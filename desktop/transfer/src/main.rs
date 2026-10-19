//! `jetstream-transfer sync <folder> gs://bucket/prefix` — prints one JSON event per line.
//! `ls`, `du` and `scan` print one JSON document (the same data the desktop app shows).
//!
//! Exit codes: 0 = everything synced, 2 = finished with failed files, 3 = cancelled, 1 = error.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use regex::Regex;

use jetstream_transfer::auth::TokenProvider;
use jetstream_transfer::gcs::GcsClient;
use jetstream_transfer::manifest::{default_data_dir, Manifest};
use jetstream_transfer::sync::{parse_destination, sync, Emit, SyncOptions};
use jetstream_transfer::fileops::{self, Item, OpProgress, Plan};
use jetstream_transfer::localops;
use jetstream_transfer::throttle::{mbps_to_bytes, Throttle};
use jetstream_transfer::{audit, browse, scan};

#[derive(Parser)]
#[command(version, about = "JetStream upload engine for Google Cloud Storage")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Upload new and changed files from a local folder to gs://bucket/prefix.
    Sync {
        /// Local folder to upload.
        source: PathBuf,
        /// Destination, e.g. gs://my-bucket/surveys/2026
        destination: String,
        /// Files uploaded at the same time.
        #[arg(long, default_value_t = 32)]
        concurrency: usize,
        /// Large files (resumable uploads) at the same time; each holds one chunk in memory.
        #[arg(long, default_value_t = 4)]
        large_concurrency: usize,
        /// Never overwrite objects that already exist in the bucket.
        #[arg(long)]
        no_clobber: bool,
        /// Report what would be uploaded without uploading.
        #[arg(long)]
        dry_run: bool,
        /// Only upload files directly in the folder, not subfolders.
        #[arg(long)]
        no_recursive: bool,
        /// Re-check every file against the bucket instead of trusting the local manifest.
        #[arg(long)]
        verify: bool,
        /// Skip relative paths matching this regex (repeatable), e.g. '.*\.tmp$'.
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        /// Bandwidth limit in megabits per second (0 = no limit).
        #[arg(long, default_value_t = 0.0)]
        max_mbps: f64,
        /// Read control commands from stdin, one JSON object per line:
        /// {"cmd":"pause"}, {"cmd":"resume"}, {"cmd":"limit","mbps":20}.
        #[arg(long)]
        control_stdin: bool,
        /// Files larger than this many MiB use resumable uploads.
        #[arg(long, default_value_t = 8)]
        resumable_threshold_mib: u64,
        /// Resumable chunk size in MiB (rounded to 256 KiB).
        #[arg(long, default_value_t = 16)]
        chunk_mib: usize,
        /// Manifest database (default: <JetStream data folder>/transfer-manifest.db).
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    /// List one folder level of gs://bucket/prefix (folders and files).
    Ls {
        path: String,
        #[arg(long, default_value_t = 500)]
        max_results: u32,
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Total objects and bytes under gs://bucket/prefix.
    Du {
        path: String,
        /// Stop counting after this many objects.
        #[arg(long, default_value_t = 1_000_000)]
        limit: u64,
    },
    /// Analyze a local folder: sizes by type and subfolder, largest, old and junk files.
    Scan { folder: PathBuf },
    /// Find junk objects (temporary/system files) under gs://bucket/prefix. Read-only.
    Audit {
        path: String,
        /// Regex matched against object names (repeatable); defaults to common junk patterns.
        #[arg(long = "pattern")]
        patterns: Vec<String>,
        /// Stop after scanning this many objects (0 = no limit).
        #[arg(long, default_value_t = 0)]
        max_objects: u64,
        #[arg(long, default_value_t = 20_000)]
        max_findings: usize,
    },
    /// Compare a local folder with its cloud copy: in both, only local, only in the cloud, changed.
    Compare {
        folder: PathBuf,
        destination: String,
        /// Glob patterns skipped by the sync (e.g. "*.tmp"); repeatable.
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        /// Folder names skipped anywhere; repeatable.
        #[arg(long = "exclude-folder")]
        exclude_folders: Vec<String>,
        #[arg(long)]
        no_recursive: bool,
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    /// Download objects or folders (ending in "/") into a local folder, checked by checksum.
    /// Existing local files are never replaced.
    Download {
        #[arg(required = true, num_args = 1..)]
        sources: Vec<String>,
        #[arg(long)]
        to: PathBuf,
        #[arg(long, default_value_t = 0.0)]
        max_mbps: f64,
    },
    /// Upload individual files into gs://bucket/folder/ (existing objects are left alone).
    Upload {
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        #[arg(long)]
        to: String,
        #[arg(long, default_value_t = 0.0)]
        max_mbps: f64,
    },
    /// Bucket settings (soft delete, hierarchical namespace, Autoclass) and your permissions.
    Bucket { bucket: String },
    /// Copy objects or folders (gs://b/folder/ ends with "/") into a folder, server-side.
    /// Existing destinations are never overwritten.
    Cp {
        #[arg(required = true, num_args = 1..)]
        sources: Vec<String>,
        /// Destination folder, gs://bucket/folder/
        #[arg(long)]
        to: String,
        /// Only report what would happen.
        #[arg(long)]
        plan: bool,
    },
    /// Move objects or folders into a folder: copy, verify, then remove each original.
    Mv {
        #[arg(required = true, num_args = 1..)]
        sources: Vec<String>,
        #[arg(long)]
        to: String,
        #[arg(long)]
        plan: bool,
    },
    /// Rename an object or folder (gs://b/folder/ ends with "/") in place.
    Rename {
        path: String,
        new_name: String,
        #[arg(long)]
        plan: bool,
    },
    /// Delete objects or folders (recursively).
    Rm {
        #[arg(required = true, num_args = 1..)]
        sources: Vec<String>,
        #[arg(long)]
        plan: bool,
    },
    /// Create an empty folder NAME inside gs://bucket/parent/.
    Mkdir { parent: String, name: String },
    /// List soft-deleted objects under gs://bucket/prefix.
    Deleted {
        path: String,
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Restore soft-deleted objects, given as NAME#GENERATION (from `deleted`).
    Restore {
        bucket: String,
        #[arg(required = true, num_args = 1..)]
        objects: Vec<String>,
    },
}

/// Pause, resume and bandwidth changes from the controlling process (the JetStream server).
/// A plain thread, so a blocked read never keeps the process from exiting.
fn read_controls(throttle: Arc<Throttle>) {
    use std::io::BufRead;
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(command) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            match command["cmd"].as_str() {
                Some("pause") => throttle.set_paused(true),
                Some("resume") => throttle.set_paused(false),
                Some("limit") => throttle.set_rate(mbps_to_bytes(command["mbps"].as_f64().unwrap_or(0.0))),
                _ => {}
            }
        }
    });
}

/// "gs://bucket/a/b/" -> ("bucket", "a/b/"), keeping a trailing "/" (a folder).
fn split_gs(path: &str) -> Result<(String, String)> {
    let trimmed = path.trim().trim_start_matches("gs://");
    let (bucket, name) = trimmed.split_once('/').unwrap_or((trimmed, ""));
    if bucket.is_empty() {
        anyhow::bail!("expected gs://bucket/path, got {path}");
    }
    Ok((bucket.to_string(), name.to_string()))
}

/// Sources must all be in one bucket.
fn split_sources(sources: &[String]) -> Result<(String, Vec<Item>)> {
    let mut bucket = String::new();
    let mut items = Vec::new();
    for source in sources {
        let (b, path) = split_gs(source)?;
        if !bucket.is_empty() && b != bucket {
            anyhow::bail!("all sources must be in the same bucket");
        }
        bucket = b;
        items.push(Item { path });
    }
    Ok((bucket, items))
}

/// Print the plan, or run it and print the result (exit 2 if anything failed).
async fn plan_or_run(client: &GcsClient, plan: Plan, soft_delete_seconds: u64, only_plan: bool) -> Result<i32> {
    if only_plan {
        let mut value = serde_json::to_value(&plan.summary)?;
        value["soft_delete_seconds"] = soft_delete_seconds.into();
        return print_json(&value);
    }
    let progress = OpProgress::default();
    let result = fileops::run(client, &plan, soft_delete_seconds > 0, &progress).await;
    print_json(&result)?;
    Ok(if result.failed > 0 { 2 } else { 0 })
}

fn cloud_client() -> Result<GcsClient> {
    let http = GcsClient::http_client();
    let auth = Arc::new(TokenProvider::from_environment(http.clone())?);
    Ok(GcsClient::new(http, auth))
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<i32> {
    println!("{}", serde_json::to_string(value)?);
    Ok(0)
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(err) => {
            let event = serde_json::json!({ "event": "error", "error": format!("{err:#}") });
            println!("{event}");
            1
        }
    };
    std::process::exit(code);
}

#[tokio::main]
async fn run() -> Result<i32> {
    let command = Cli::parse().command;
    let is_move = matches!(command, Command::Mv { .. });
    match command {
        Command::Ls { path, max_results, page_token } => {
            let (bucket, prefix) = parse_destination(&path)?;
            let listing = browse::list_folder(&cloud_client()?, &bucket, &prefix, page_token.as_deref(), max_results, false).await?;
            return print_json(&listing);
        }
        Command::Du { path, limit } => {
            let (bucket, prefix) = parse_destination(&path)?;
            return print_json(&browse::folder_totals(&cloud_client()?, &bucket, &prefix, limit).await?);
        }
        Command::Compare { folder, destination, excludes, exclude_folders, no_recursive, manifest } => {
            let regexes = jetstream_transfer::compare::exclude_regexes(&excludes, &exclude_folders)?;
            let path = manifest.unwrap_or_else(|| default_data_dir().join("transfer-manifest.db"));
            let manifest = if path.exists() { Manifest::open(&path).ok() } else { None };
            let report = jetstream_transfer::compare::compare(&cloud_client()?, &folder, &destination, &regexes, !no_recursive, manifest.as_ref())
                .await?;
            return print_json(&report);
        }
        Command::Download { sources, to, max_mbps } => {
            let (bucket, items) = split_sources(&sources)?;
            let result = localops::download(&cloud_client()?, &bucket, &items, &to, &Throttle::from_mbps(max_mbps), &OpProgress::default()).await?;
            print_json(&result)?;
            return Ok(if result.failed > 0 { 2 } else { 0 });
        }
        Command::Upload { files, to, max_mbps } => {
            let (bucket, prefix) = split_gs(&to)?;
            let result =
                localops::upload_files(&cloud_client()?, &files, &bucket, &prefix, &Throttle::from_mbps(max_mbps), &OpProgress::default()).await?;
            print_json(&result)?;
            return Ok(if result.failed > 0 { 2 } else { 0 });
        }
        Command::Bucket { bucket } => {
            let bucket = bucket.trim_start_matches("gs://").trim_end_matches('/');
            return print_json(&fileops::bucket_access(&cloud_client()?, bucket).await);
        }
        Command::Cp { sources, to, plan } | Command::Mv { sources, to, plan } => {
            let delete_source = is_move;
            let client = cloud_client()?;
            let (bucket, items) = split_sources(&sources)?;
            let (dst_bucket, dst_prefix) = split_gs(&to)?;
            let source = fileops::bucket_access(&client, &bucket).await.bucket;
            let soft = source.soft_delete_seconds;
            let op = fileops::plan_transfer(&client, &source, &items, &dst_bucket, &dst_prefix, None, delete_source).await?;
            return plan_or_run(&client, op, soft, plan).await;
        }
        Command::Rename { path, new_name, plan } => {
            let client = cloud_client()?;
            let (bucket, name) = split_gs(&path)?;
            let parent = name.trim_end_matches('/').rsplit_once('/').map(|(p, _)| format!("{p}/")).unwrap_or_default();
            let source = fileops::bucket_access(&client, &bucket).await.bucket;
            let soft = source.soft_delete_seconds;
            let op = fileops::plan_transfer(&client, &source, &[Item { path: name }], &bucket, &parent, Some(&new_name), true).await?;
            return plan_or_run(&client, op, soft, plan).await;
        }
        Command::Rm { sources, plan } => {
            let client = cloud_client()?;
            let (bucket, items) = split_sources(&sources)?;
            let source = fileops::bucket_access(&client, &bucket).await.bucket;
            let soft = source.soft_delete_seconds;
            let op = fileops::plan_delete(&client, &source, &items).await?;
            return plan_or_run(&client, op, soft, plan).await;
        }
        Command::Mkdir { parent, name } => {
            let client = cloud_client()?;
            let (bucket, parent) = split_gs(&parent)?;
            let source = fileops::bucket_access(&client, &bucket).await.bucket;
            let path = fileops::new_folder(&client, &source, &parent, &name).await?;
            return print_json(&serde_json::json!({ "created": format!("gs://{bucket}/{path}") }));
        }
        Command::Deleted { path, page_token } => {
            let (bucket, prefix) = split_gs(&path)?;
            return print_json(&fileops::list_deleted(&cloud_client()?, &bucket, &prefix, page_token.as_deref()).await?);
        }
        Command::Restore { bucket, objects } => {
            let client = cloud_client()?;
            let bucket = bucket.trim_start_matches("gs://").trim_end_matches('/').to_string();
            let pairs = objects
                .iter()
                .map(|o| o.rsplit_once('#').map(|(n, g)| (n.to_string(), g.to_string())).context("expected NAME#GENERATION"))
                .collect::<Result<Vec<_>>>()?;
            return plan_or_run(&client, fileops::plan_restore(&bucket, &pairs), 1, false).await;
        }
        Command::Scan { folder } => {
            let report = tokio::task::spawn_blocking(move || {
                scan::scan_folder(&folder, &scan::ScanOptions::default(), &scan::ScanProgress::default())
            })
            .await??;
            return print_json(&report);
        }
        Command::Audit { path, patterns, max_objects, max_findings } => {
            let (bucket, prefix) = parse_destination(&path)?;
            let patterns = if patterns.is_empty() {
                audit::DEFAULT_PATTERNS.iter().map(|p| p.to_string()).collect()
            } else {
                patterns
            };
            let report = audit::audit_bucket(
                &cloud_client()?,
                &bucket,
                &prefix,
                &patterns,
                max_objects,
                max_findings,
                &audit::AuditProgress::default(),
            )
            .await?;
            return print_json(&report);
        }
        Command::Sync { .. } => {}
    }
    let Command::Sync {
        source,
        destination,
        concurrency,
        large_concurrency,
        no_clobber,
        dry_run,
        no_recursive,
        verify,
        excludes,
        max_mbps,
        control_stdin,
        resumable_threshold_mib,
        chunk_mib,
        manifest,
    } = command
    else {
        unreachable!()
    };

    let mut opts = SyncOptions::new(source, &destination)?;
    opts.concurrency = concurrency;
    opts.large_concurrency = large_concurrency;
    opts.no_clobber = no_clobber;
    opts.dry_run = dry_run;
    opts.recursive = !no_recursive;
    opts.verify = verify;
    opts.resumable_threshold = resumable_threshold_mib * 1024 * 1024;
    opts.chunk_size = chunk_mib * 1024 * 1024;
    opts.excludes = excludes
        .iter()
        .map(|pattern| Regex::new(pattern).with_context(|| format!("invalid --exclude regex: {pattern}")))
        .collect::<Result<_>>()?;
    opts.throttle = Arc::new(Throttle::from_mbps(max_mbps));
    if control_stdin {
        read_controls(opts.throttle.clone());
    }
    if std::env::var_os("JETSTREAM_TRANSFER_FAST_RETRY").is_some() {
        opts.retry_base = Duration::from_millis(50);
    }

    let manifest_path = manifest.unwrap_or_else(|| default_data_dir().join("transfer-manifest.db"));
    let manifest = Arc::new(Manifest::open(&manifest_path)?);

    let http = GcsClient::http_client();
    let auth = Arc::new(TokenProvider::from_environment(http.clone())?);
    let client = GcsClient::new(http, auth.clone());

    let stdout = Mutex::new(std::io::stdout());
    let emit: Emit = Arc::new(move |event| {
        let mut out = stdout.lock().unwrap();
        let _ = serde_json::to_writer(&mut *out, &event);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    });

    // Ctrl+C / SIGTERM: stop starting new files; large uploads keep their session for next time.
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_flag = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_flag.store(true, Ordering::SeqCst);
        }
    });

    let summary = sync(client, manifest, opts, auth.describe(), emit, cancel).await?;
    Ok(if summary.cancelled {
        3
    } else if summary.failed + summary.unreadable > 0 {
        2
    } else {
        0
    })
}
