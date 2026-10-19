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
    match command {
        Command::Ls { path, max_results, page_token } => {
            let (bucket, prefix) = parse_destination(&path)?;
            let listing = browse::list_folder(&cloud_client()?, &bucket, &prefix, page_token.as_deref(), max_results).await?;
            return print_json(&listing);
        }
        Command::Du { path, limit } => {
            let (bucket, prefix) = parse_destination(&path)?;
            return print_json(&browse::folder_totals(&cloud_client()?, &bucket, &prefix, limit).await?);
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
