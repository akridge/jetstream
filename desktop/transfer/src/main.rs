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
#[command(version = jetstream_transfer::VERSION_TEXT, about = "JetStream upload engine for Google Cloud Storage")]
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
        /// {"cmd":"pause"}, {"cmd":"resume"}, {"cmd":"limit","mbps":20}, {"cmd":"stop"}.
        #[arg(long)]
        control_stdin: bool,
        /// Keep --concurrency and --chunk-mib as given (by default they rise for many small
        /// files or very large ones).
        #[arg(long)]
        no_auto_tune: bool,
        /// Files larger than this many MiB use resumable uploads.
        #[arg(long, default_value_t = 8)]
        resumable_threshold_mib: u64,
        /// Resumable chunk size in MiB (rounded to 256 KiB).
        #[arg(long, default_value_t = 16)]
        chunk_mib: usize,
        /// Upload large files as one stream instead of parallel parts joined in the bucket
        /// (joined objects have a CRC32C checksum but no MD5).
        #[arg(long)]
        no_composite: bool,
        /// Files at least this many MiB use parallel parts (when the bucket allows it).
        #[arg(long, default_value_t = 256)]
        composite_threshold_mib: u64,
        /// Target size of each part in MiB (2 to 32 parts per file).
        #[arg(long, default_value_t = 32)]
        composite_part_mib: u64,
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
    /// Check a folder against its cloud copy file by file and write a receipt (CSV + JSON).
    Receipt {
        folder: PathBuf,
        destination: String,
        /// Folder to write the receipt files to.
        #[arg(long)]
        out: PathBuf,
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        #[arg(long = "exclude-folder")]
        exclude_folders: Vec<String>,
        #[arg(long)]
        no_recursive: bool,
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    /// Print a saved receipt as the app shows it: summary and problems, not the verified files.
    #[command(hide = true)]
    ReceiptView { json: PathBuf },
    /// Add missing file types (and, with --local, the original files' dates) to objects already
    /// in gs://bucket/prefix by updating their metadata. A plan unless --apply.
    FixDetails {
        destination: String,
        /// The folder synced to it (for the original dates).
        #[arg(long)]
        local: Option<PathBuf>,
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        #[arg(long = "exclude-folder")]
        exclude_folders: Vec<String>,
        #[arg(long)]
        no_recursive: bool,
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Make the changes (otherwise only report what would change).
        #[arg(long)]
        apply: bool,
    },
    /// Measure what limits uploads: the folder (list and read speed), the network (one
    /// connection and 8 at once) and request time. Nothing is stored in the bucket.
    /// Build a catalog of files under gs://bucket/prefix (the Cloud Catalog Tool): images by
    /// default, grouped by filename token. Prints the OASIS JSON unless --format says otherwise.
    Catalog {
        /// gs://bucket/prefix
        prefix: String,
        /// The full request as JSON (or @file.json), as the catalog tool sends it; the options
        /// below override it.
        #[arg(long)]
        request: Option<String>,
        /// Kinds of files: images, raw, video, science, tables, documents, audio, all.
        #[arg(long, value_delimiter = ',')]
        kinds: Vec<String>,
        /// Extra extensions (".nc,.csv").
        #[arg(long = "ext", value_delimiter = ',')]
        extensions: Vec<String>,
        /// none, filename_token, folder_segment, regex, manual.
        #[arg(long)]
        group_by: Option<String>,
        /// oasis (default), detailed-csv, detailed-json, catalog (the full catalog as the app saves it).
        #[arg(long, default_value = "oasis")]
        format: String,
        /// Dataset information: an InPort item number, an ISO 19115 record URL, or an XML file.
        #[arg(long)]
        dataset: Option<String>,
        /// Write here instead of printing.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Use these object names (one per line) instead of listing the bucket (for testing).
        #[arg(long, hide = true)]
        names: Option<PathBuf>,
        /// Keep the listing in this file: list once, then regroup from it (for testing the app).
        #[arg(long, hide = true)]
        listing: Option<PathBuf>,
        /// Dataset information as JSON (as the app passes it).
        #[arg(long, hide = true)]
        dataset_json: Option<String>,
        /// With --format group-files: the group (all files when absent), offset and count.
        #[arg(long, hide = true)]
        group: Option<String>,
        #[arg(long, hide = true, default_value_t = 0)]
        offset: usize,
        #[arg(long, hide = true, default_value_t = 120)]
        limit: usize,
    },
    /// Read dataset information from NOAA InPort (an item number) or an ISO 19115 record (URL or file).
    DatasetInfo { source: String },
    SpeedCheck {
        /// gs://bucket/prefix to test against.
        destination: String,
        /// The folder you upload from (to test reading it).
        #[arg(long)]
        local: Option<PathBuf>,
    },
    /// Make a thumbnail of an image in a bucket (cached), as the cloud browser does.
    Thumbnail {
        /// gs://bucket/path/to/image.jpg
        object: String,
        /// Thumbnail cache folder.
        #[arg(long)]
        cache: PathBuf,
        /// Write the thumbnail (JPEG) here.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Preview Nearline/Coldline/Archive objects too (retrieval is charged).
        #[arg(long)]
        allow_cold: bool,
        /// Don't download images bigger than this many MiB.
        #[arg(long, default_value_t = 20)]
        max_mib: u64,
        /// Cache size limit in MiB.
        #[arg(long, default_value_t = 250)]
        cache_mib: u64,
        /// Longest side in pixels (256 for tiles, 1024 for the preview pane).
        #[arg(long, default_value_t = jetstream_transfer::thumbs::EDGE)]
        edge: u32,
    },
    /// Print an object's details (its metadata as Cloud Storage returns it).
    Stat { object: String },
    /// Print the start of a text file in a bucket (64 KB), as the preview pane shows it.
    Head {
        object: String,
        #[arg(long)]
        allow_cold: bool,
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

/// Pause, resume, bandwidth and stop from the controlling process (the JetStream server).
/// "stop" finishes or saves what's in flight, writes the manifest and exits (code 3).
/// A plain thread, so a blocked read never keeps the process from exiting.
fn read_controls(throttle: Arc<Throttle>, cancel: Arc<AtomicBool>) {
    use std::io::BufRead;
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(command) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            match command["cmd"].as_str() {
                Some("pause") => throttle.set_paused(true),
                Some("resume") => throttle.set_paused(false),
                Some("limit") => throttle.set_rate(mbps_to_bytes(command["mbps"].as_f64().unwrap_or(0.0))),
                Some("stop") => {
                    cancel.store(true, Ordering::SeqCst);
                    // A paused sync must notice the stop too.
                    throttle.set_paused(false);
                }
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

/// An InPort item number or ISO record URL (fetched), or an XML file.
async fn dataset_info(source: &str) -> Result<jetstream_transfer::catalog_meta::DatasetInfo> {
    let path = std::path::Path::new(source);
    if path.is_file() {
        return jetstream_transfer::catalog_meta::read_file(path);
    }
    jetstream_transfer::catalog_meta::fetch(source).await
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
        Command::Receipt { folder, destination, out, excludes, exclude_folders, no_recursive, manifest } => {
            let regexes = jetstream_transfer::compare::exclude_regexes(&excludes, &exclude_folders)?;
            let path = manifest.unwrap_or_else(|| default_data_dir().join("transfer-manifest.db"));
            let manifest = Manifest::open(&path).ok();
            let http = GcsClient::http_client();
            let auth = Arc::new(TokenProvider::from_environment(http.clone())?);
            let client = GcsClient::new(http, auth.clone());
            let receipt = jetstream_transfer::receipt::make_receipt(
                &client, &folder, &destination, &regexes, !no_recursive, manifest.as_ref(), auth.describe(), &OpProgress::default(),
            )
            .await?;
            let (csv, json) = jetstream_transfer::receipt::write_receipt(&receipt, &out)?;
            print_json(&serde_json::json!({
                "result": receipt.result, "totals": receipt.totals, "csv": csv, "json": json,
                "finished_at": receipt.finished_at, "destination": receipt.destination,
            }))?;
            return Ok(if receipt.result == "complete" { 0 } else { 2 });
        }
        Command::ReceiptView { json } => {
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&json)?)?;
            return print_json(&jetstream_transfer::receipt::view(&value, jetstream_transfer::receipt::VIEW_PROBLEMS_PER_KIND));
        }
        Command::FixDetails { destination, local, excludes, exclude_folders, no_recursive, manifest, apply } => {
            let regexes = jetstream_transfer::compare::exclude_regexes(&excludes, &exclude_folders)?;
            let path = manifest.unwrap_or_else(|| default_data_dir().join("transfer-manifest.db"));
            let manifest = if path.exists() { Manifest::open(&path).ok() } else { None };
            let client = cloud_client()?;
            let local = local.as_deref().map(|dir| (dir, regexes.as_slice(), !no_recursive));
            let plan = jetstream_transfer::details::plan_fix(&client, &destination, local, manifest.as_ref(), &OpProgress::default()).await?;
            if !apply {
                return print_json(&plan);
            }
            let result = jetstream_transfer::details::apply_fix(&client, &plan, &OpProgress::default()).await;
            print_json(&serde_json::json!({ "plan": plan, "result": result }))?;
            return Ok(if result.failed > 0 { 2 } else { 0 });
        }
        Command::Catalog { prefix, request, kinds, extensions, group_by, format, dataset, out, names, listing: listing_file, dataset_json, group, offset, limit } => {
            use jetstream_transfer::catalog;
            let mut value: serde_json::Value = match request.as_deref() {
                Some(text) if text.starts_with('@') => serde_json::from_str(&std::fs::read_to_string(&text[1..])?)?,
                Some(text) => serde_json::from_str(text)?,
                None => serde_json::json!({}),
            };
            value["gcs_prefix"] = prefix.clone().into();
            if !kinds.is_empty() {
                value["kinds"] = serde_json::json!(kinds);
            }
            if !extensions.is_empty() {
                value["extensions"] = serde_json::json!(extensions);
            }
            if let Some(mode) = group_by {
                value["group_by_mode"] = mode.replace('-', "_").into();
            }
            let request: catalog::CatalogRequest = serde_json::from_value(value)?;
            let listing = match (names, &listing_file) {
                (Some(file), _) => catalog::listing_from_names(&request, &std::fs::read_to_string(file)?.lines().collect::<Vec<_>>())?,
                (None, Some(path)) if path.exists() => serde_json::from_str::<catalog::Listing>(&std::fs::read_to_string(path)?)?,
                (None, _) => catalog::scan(&cloud_client()?, &request, &OpProgress::default()).await?,
            };
            if let Some(path) = &listing_file {
                if !path.exists() {
                    std::fs::write(path, serde_json::to_string(&listing)?)?;
                }
            }
            let now = catalog::iso_utc(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64);
            let (mut built, full) = catalog::build_with_table(&listing, &request.grouping, &now)?;
            for warning in &built.warnings {
                eprintln!("warning: {warning}");
            }
            let info = match (dataset, dataset_json) {
                (Some(source), _) => Some(serde_json::to_value(dataset_info(&source).await?)?),
                (None, Some(text)) => serde_json::from_str::<Option<serde_json::Value>>(&text)?,
                (None, None) => None,
            };
            built.metadata.jetstream_request = Some(request.clone());
            built.metadata.dataset = info.clone();
            let text = match format.as_str() {
                "oasis" => catalog::oasis_json(&built),
                "detailed-csv" => catalog::detailed_csv(&listing, &built),
                "detailed-json" => serde_json::to_string_pretty(&catalog::detailed_json(&listing, &built, info.as_ref()))?,
                "catalog" => serde_json::to_string_pretty(&built)?,
                "summary" => serde_json::to_string(&catalog::summary(&listing, &built, full.as_ref()))?,
                "group-files" => {
                    let (total, entries) = catalog::group_page(&listing, &built, group.as_deref(), offset, limit)?;
                    serde_json::to_string(&serde_json::json!({ "total": total, "entries": entries }))?
                }
                other => anyhow::bail!("unknown format {other:?}: use oasis, detailed-csv, detailed-json or catalog"),
            };
            match out {
                Some(path) => std::fs::write(path, text)?,
                None => print!("{text}"),
            }
            return Ok(0);
        }
        Command::DatasetInfo { source } => {
            return print_json(&dataset_info(&source).await?);
        }
        Command::SpeedCheck { destination, local } => {
            let client = cloud_client()?;
            let report = jetstream_transfer::speedcheck::speed_check(&client, &destination, local.as_deref(), &OpProgress::default()).await?;
            return print_json(&report);
        }
        Command::Stat { object } => {
            let (bucket, name) = split_gs(&object)?;
            let raw = cloud_client()?.get_object_raw(&bucket, &name).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            return print_json(&raw.ok_or_else(|| anyhow::anyhow!("{object} not found"))?);
        }
        Command::Head { object, allow_cold } => {
            let (bucket, name) = split_gs(&object)?;
            let client = cloud_client()?;
            let info = client.get_object(&bucket, &name).await.map_err(|e| anyhow::anyhow!("{e}"))?.ok_or_else(|| anyhow::anyhow!("{object} not found"))?;
            let preview = jetstream_transfer::preview::text(&client, &bucket, &name, info.generation.as_deref(), info.size, info.storage_class.as_deref(), allow_cold).await;
            return print_json(&preview);
        }
        Command::Thumbnail { object, cache, out, allow_cold, max_mib, cache_mib, edge } => {
            use jetstream_transfer::thumbs;
            let (bucket, name) = split_gs(&object)?;
            let client = cloud_client()?;
            let info = client.get_object(&bucket, &name).await.map_err(|e| anyhow::anyhow!("{e}"))?.ok_or_else(|| anyhow::anyhow!("{object} not found"))?;
            let object = thumbs::ObjectRef {
                bucket,
                name,
                generation: info.generation,
                size: info.size,
                content_type: info.content_type,
                storage_class: info.storage_class,
            };
            let cache = thumbs::ThumbCache::open(cache, cache_mib * 1024 * 1024)?;
            let opts = thumbs::ThumbOptions { max_bytes: max_mib * 1024 * 1024, allow_cold, edge };
            return match thumbs::thumbnail(&client, &cache, &object, &opts).await {
                Ok(thumb) => {
                    if let Some(out) = out {
                        std::fs::write(&out, &thumb.jpeg)?;
                    }
                    print_json(&serde_json::json!({ "bytes": thumb.jpeg.len(), "source": thumb.source }))
                }
                Err(skip) => print_json(&skip).map(|_| 2),
            };
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
        no_auto_tune,
        resumable_threshold_mib,
        chunk_mib,
        no_composite,
        composite_threshold_mib,
        composite_part_mib,
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
    opts.composite = !no_composite;
    opts.composite_threshold = composite_threshold_mib * 1024 * 1024;
    opts.composite_part_size = composite_part_mib.max(1) * 1024 * 1024;
    opts.excludes = excludes
        .iter()
        .map(|pattern| Regex::new(pattern).with_context(|| format!("invalid --exclude regex: {pattern}")))
        .collect::<Result<_>>()?;
    opts.auto_tune = !no_auto_tune;
    opts.throttle = Arc::new(Throttle::from_mbps(max_mbps));
    let cancel = Arc::new(AtomicBool::new(false));
    if control_stdin {
        read_controls(opts.throttle.clone(), cancel.clone());
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
