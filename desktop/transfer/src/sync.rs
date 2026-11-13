//! Incremental folder → Cloud Storage sync.
//!
//! 1. Walk the folder (size + mtime only). Online-only cloud placeholders (OneDrive "files on
//!    demand") are skipped with an explanation instead of silently downloading.
//! 2. Files whose size and mtime match the manifest are unchanged: no hashing, no requests.
//!    An early estimate is reported straight away, so progress starts from what's synced.
//! 3. If any file is unknown (or with `verify`), the bucket is listed, in parallel by
//!    subfolder and only where unknown files are. An object with the same size and
//!    `goog-reserved-file-mtime` (as `gcloud storage rsync` records) counts as synced without
//!    reading the local file.
//! 4. The rest is hashed (crc32c) on its own stage, which feeds the uploads: small files in one
//!    request (from the bytes already read for hashing), large files as resumable sessions whose
//!    URI is saved, so an interrupted file continues where it stopped. Every object's crc32c is
//!    checked by Cloud Storage and again on the response.
//! 5. A request whose body stops moving is abandoned and retried; files locked by another
//!    program are tried once more at the end.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{bail, Result};
use futures::stream::{self, StreamExt};
use regex::Regex;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::Semaphore;

use crate::gcs::{GcsClient, GcsError, ObjectInfo, ResumableStatus, UploadExtras, CHUNK_ALIGN};
use crate::manifest::{FileRecord, Manifest};
use crate::onedrive;
use crate::throttle::Throttle;

/// Small files are kept in memory after hashing and uploaded from the same bytes.
const KEEP_IN_MEMORY: u64 = 1024 * 1024;
/// Files at least this big report when they start uploading (smaller ones finish quickly).
const REPORT_START: u64 = 4 * 1024 * 1024;
/// Online-only placeholders reported one by one (the count covers all of them).
const MAX_REPORTED_PLACEHOLDERS: usize = 200;
const LIST_CONCURRENCY: usize = 8;

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub source: PathBuf,
    pub bucket: String,
    /// Object name prefix, empty or ending in '/'.
    pub prefix: String,
    pub concurrency: usize,
    /// Large files uploading at once (each holds one chunk in memory).
    pub large_concurrency: usize,
    pub no_clobber: bool,
    pub dry_run: bool,
    /// Include subfolders (otherwise only files directly in `source`).
    pub recursive: bool,
    /// Compare every file against a fresh bucket listing instead of trusting the manifest.
    pub verify: bool,
    /// Regexes matched against the relative path (with '/' separators); matches are skipped.
    pub excludes: Vec<Regex>,
    /// Files larger than this use resumable uploads.
    pub resumable_threshold: u64,
    pub chunk_size: usize,
    pub max_attempts: u32,
    pub retry_base: Duration,
    /// Bandwidth limit and pause, shared with whoever controls the sync.
    pub throttle: Arc<Throttle>,
    /// Raise concurrency for many small files and the chunk size for very large ones.
    pub auto_tune: bool,
    /// Files hashed at once.
    pub hash_concurrency: usize,
    /// Upload large files as parts in parallel, then join them in the bucket (when the bucket
    /// allows it: Standard storage class or Autoclass, no retention policy, delete permission).
    pub composite: bool,
    /// Files at least this big use parallel parts.
    pub composite_threshold: u64,
    /// Aim for parts of about this size (2 to 32 parts per file).
    pub composite_part_size: u64,
    /// Parts uploading at once across the sync.
    pub composite_concurrency: usize,
}

impl SyncOptions {
    pub fn new(source: PathBuf, destination: &str) -> Result<Self> {
        let (bucket, prefix) = parse_destination(destination)?;
        Ok(Self {
            source,
            bucket,
            prefix,
            concurrency: 32,
            large_concurrency: 4,
            no_clobber: false,
            dry_run: false,
            recursive: true,
            verify: false,
            excludes: Vec::new(),
            resumable_threshold: 8 * 1024 * 1024,
            chunk_size: 16 * 1024 * 1024,
            max_attempts: 6,
            retry_base: Duration::from_secs(1),
            throttle: Arc::new(Throttle::default()),
            auto_tune: true,
            hash_concurrency: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8),
            composite: true,
            composite_threshold: 256 * 1024 * 1024,
            composite_part_size: 32 * 1024 * 1024,
            composite_concurrency: 8,
        })
    }

    pub fn destination(&self) -> String {
        format!("gs://{}/{}", self.bucket, self.prefix)
    }
}

/// "gs://bucket/some/prefix" or "bucket/some/prefix" -> ("bucket", "some/prefix/").
pub fn parse_destination(destination: &str) -> Result<(String, String)> {
    let trimmed = destination.trim().trim_start_matches("gs://").trim_matches('/');
    let (bucket, prefix) = trimmed.split_once('/').unwrap_or((trimmed, ""));
    if bucket.is_empty() {
        bail!("destination must look like gs://bucket/path");
    }
    let prefix = prefix.trim_matches('/');
    Ok((bucket.to_string(), if prefix.is_empty() { String::new() } else { format!("{prefix}/") }))
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub local_files: u64,
    pub local_bytes: u64,
    pub excluded: u64,
    pub unchanged: u64,
    pub already_in_cloud: u64,
    /// Of `already_in_cloud`, recognised by size and modified time without reading the file.
    pub matched_by_time: u64,
    pub uploaded: u64,
    pub uploaded_bytes: u64,
    pub resumed: u64,
    pub would_upload: u64,
    pub would_upload_bytes: u64,
    pub skipped_exists: u64,
    pub failed: u64,
    pub unreadable: u64,
    /// Online-only cloud placeholders (OneDrive "files on demand"), skipped.
    pub online_only: u64,
    pub hashed_files: u64,
    pub hashed_bytes: u64,
    pub listed_objects: Option<u64>,
    /// Files uploaded as parallel parts joined in the bucket.
    pub composite_files: u64,
    /// Temporary parts that couldn't be deleted after joining (under `_jetstream/parts/`).
    pub parts_left: u64,
    pub cancelled: bool,
    pub elapsed_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Start {
        source: String,
        destination: String,
        auth: String,
        concurrency: usize,
        dry_run: bool,
        verify: bool,
        no_clobber: bool,
    },
    /// "scanning", "listing" or "uploading".
    Phase { phase: &'static str },
    Plan {
        local_files: u64,
        local_bytes: u64,
        excluded: u64,
        unchanged: u64,
        to_check: u64,
        to_check_bytes: u64,
        listed_objects: Option<u64>,
        /// From the manifest alone, before the bucket listing refines it.
        estimate: bool,
    },
    /// Concurrency and chunk size chosen for this sync.
    Tuned { concurrency: usize, large_chunk_mib: usize },
    /// A large file started uploading; small files only report when done.
    FileStarted {
        path: String,
        bytes: u64,
        /// Uploading as this many parallel parts.
        #[serde(skip_serializing_if = "Option::is_none")]
        parts: Option<u32>,
    },
    /// Large files go up as one stream in this sync, and why (bucket settings, permissions).
    CompositeOff { reason: String },
    /// A request failed (or stalled) and is being tried again.
    FileRetrying { path: String, attempt: u32, max_attempts: u32, error: String },
    /// Cloud Storage can't be reached (the internet connection dropped): every upload waits,
    /// without using up its retries, and carries on by itself when the connection is back.
    Offline { error: String },
    /// The connection is back after `offline_seconds`; uploads continue where they were.
    Online { offline_seconds: u64 },
    FileUploaded { path: String, bytes: u64, generation: Option<String>, resumed_from: Option<u64> },
    FileWouldUpload { path: String, bytes: u64 },
    FileSkipped { path: String, reason: String },
    FileFailed { path: String, error: String },
    Progress {
        phase: &'static str,
        files_done: u64,
        files_total: u64,
        bytes_done: u64,
        bytes_total: u64,
        hashed_files: u64,
        hashed_bytes: u64,
        listed_objects: u64,
        paused: bool,
        /// Waiting for the internet connection to come back.
        offline: bool,
    },
    Done {
        #[serde(flatten)]
        summary: Summary,
    },
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Debug, Clone)]
pub(crate) struct LocalFile {
    pub(crate) abs: PathBuf,
    pub(crate) rel: String,
    pub(crate) size: u64,
    pub(crate) mtime_ns: i64,
}

impl LocalFile {
    fn mtime_secs(&self) -> i64 {
        self.mtime_ns.div_euclid(1_000_000_000)
    }
}

pub(crate) struct ScanResult {
    pub(crate) files: Vec<LocalFile>,
    pub(crate) excluded: u64,
    pub(crate) unreadable: u64,
    /// Relative paths of online-only placeholders (not in `files`).
    pub(crate) online_only: Vec<String>,
}

struct Candidate {
    file: LocalFile,
    known_crc: Option<u32>,
}

/// A candidate after hashing: its checksum, and its bytes when small enough to keep.
struct Hashed {
    file: LocalFile,
    crc: u32,
    data: Option<Vec<u8>>,
}

enum Outcome {
    Unchanged,
    AlreadyInCloud,
    Uploaded { generation: Option<String>, resumed_from: Option<u64> },
    WouldUpload,
    SkippedExists,
    Failed(String),
    Unreadable(String),
    Cancelled,
}

struct Ctx {
    client: GcsClient,
    emit: Emit,
    manifest: Arc<Manifest>,
    opts: SyncOptions,
    dest: String,
    records: HashMap<String, FileRecord>,
    remote: Option<HashMap<String, ObjectInfo>>,
    cancel: Arc<AtomicBool>,
    large_slots: Semaphore,
    chunk_size: usize,
    bytes_done: AtomicU64,
    /// Bytes of requests still in flight (moves smoothly within a large chunk).
    live_bytes: Arc<AtomicU64>,
    files_done: AtomicU64,
    hashed_files: AtomicU64,
    /// Files a preview settled without reading them (see `hash`).
    decided_files: AtomicU64,
    hashed_bytes: AtomicU64,
    listed: Arc<AtomicU64>,
    phase: std::sync::Mutex<&'static str>,
    net: Net,
    /// Why parallel parts aren't used in this sync (None: they are, for big files).
    composite_off: std::sync::Mutex<Option<String>>,
    part_slots: Semaphore,
    composite_files: AtomicU64,
    parts_left: AtomicU64,
}

/// Whether Cloud Storage is reachable, shared by every upload of a sync.
#[derive(Default)]
struct Net {
    offline: AtomicBool,
    /// Bumped each time an outage ends, so uploads that failed during it don't each probe again.
    outages: AtomicU64,
    /// One upload checks the connection at a time; the rest wait for its answer.
    checking: tokio::sync::Mutex<()>,
}

impl Ctx {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub(crate) fn scan(source: &Path, excludes: &[Regex], recursive: bool) -> Result<ScanResult> {
    if !source.is_dir() {
        bail!("{} is not a folder", source.display());
    }
    let mut result = ScanResult { files: Vec::new(), excluded: 0, unreadable: 0, online_only: Vec::new() };
    let walker = walkdir::WalkDir::new(source).follow_links(false).max_depth(if recursive { usize::MAX } else { 1 });
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                result.unreadable += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(source)
            .unwrap_or(entry.path())
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if excludes.iter().any(|re| re.is_match(&rel)) {
            result.excluded += 1;
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            result.unreadable += 1;
            continue;
        };
        if onedrive::is_online_only(&meta) {
            result.online_only.push(rel);
            continue;
        }
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        result.files.push(LocalFile { abs: entry.into_path(), rel, size: meta.len(), mtime_ns });
    }
    Ok(result)
}

pub(crate) fn crc32c_file(path: &Path, progress: Option<&AtomicU64>, cancel: Option<&AtomicBool>) -> std::io::Result<u32> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1024 * 1024];
    let mut crc = 0u32;
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "stopped"));
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(crc);
        }
        crc = crc32c::crc32c_append(crc, &buf[..n]);
        if let Some(p) = progress {
            p.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
}

/// The first path segment ("" for files directly in the folder).
fn top_segment(rel: &str) -> &str {
    rel.split_once('/').map(|(first, _)| first).unwrap_or("")
}

const LIST_FIELDS: &str = "name,size,crc32c,md5Hash,generation,updated,metadata";

/// List objects under gs://bucket/prefix, keyed by name relative to the prefix. With `only`,
/// just those top-level subfolders ("" = objects directly under the prefix). Subfolders are
/// listed in parallel.
/// One page of a bucket listing, tried again (for about a minute) when the connection drops or
/// Cloud Storage is briefly unavailable, so a blip doesn't fail the whole sync.
async fn list_page_retrying(
    client: &GcsClient,
    bucket: &str,
    prefix: &str,
    token: Option<&str>,
) -> Result<(Vec<serde_json::Value>, Option<String>)> {
    let mut attempt = 0u32;
    loop {
        match client.list_raw(bucket, prefix, token, LIST_FIELDS, false).await {
            Ok(page) => return Ok(page),
            Err(err) if attempt < 6 && is_transient(&format!("{err:#}")) => {
                tokio::time::sleep(Duration::from_secs(1 << attempt.min(4))).await;
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Network trouble or a temporary Cloud Storage error (not permissions or a missing bucket).
fn is_transient(message: &str) -> bool {
    ["network error", "access token", "stalled", "408", "429", "500 ", "502 ", "503 ", "504 "].iter().any(|m| message.contains(m))
}

pub(crate) async fn list_remote(
    client: &GcsClient,
    bucket: &str,
    prefix: &str,
    only: Option<&HashSet<String>>,
    listed: &AtomicU64,
) -> Result<HashMap<String, ObjectInfo>> {
    let mut out = HashMap::new();
    let add = |raw: serde_json::Value, out: &mut HashMap<String, ObjectInfo>| {
        if let Ok(info) = serde_json::from_value::<ObjectInfo>(raw) {
            if let Some(rel) = info.name.strip_prefix(prefix) {
                if !rel.is_empty() {
                    out.insert(rel.to_string(), info.clone());
                }
            }
        }
    };

    // One level first: objects directly under the prefix, and its subfolders.
    let mut subfolders = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (items, prefixes, next) = client.list_level(bucket, prefix, token.as_deref(), LIST_FIELDS).await?;
        listed.fetch_add(items.len() as u64, Ordering::Relaxed);
        if only.is_none_or(|o| o.contains("")) {
            for raw in items {
                add(raw, &mut out);
            }
        }
        subfolders.extend(prefixes);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    let wanted: Vec<String> = subfolders
        .into_iter()
        .filter(|p| only.is_none_or(|o| o.contains(p[prefix.len()..].trim_end_matches('/'))))
        .collect();

    let pages = stream::iter(wanted)
        .map(|sub| async move {
            let mut found = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let (items, next) = list_page_retrying(client, bucket, &sub, token.as_deref()).await?;
                listed.fetch_add(items.len() as u64, Ordering::Relaxed);
                found.extend(items);
                match next {
                    Some(next) => token = Some(next),
                    None => return anyhow::Ok(found),
                }
            }
        })
        .buffer_unordered(LIST_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    for page in pages {
        for raw in page? {
            add(raw, &mut out);
        }
    }
    Ok(out)
}

/// Whether an error message is Windows' "file is being used by another process" / lock error.
fn is_locked_error(message: &str) -> bool {
    message.contains("(os error 32)") || message.contains("(os error 33)")
}

pub async fn sync(
    client: GcsClient,
    manifest: Arc<Manifest>,
    opts: SyncOptions,
    auth_description: &str,
    emit: Emit,
    cancel: Arc<AtomicBool>,
) -> Result<Summary> {
    let started = Instant::now();
    let dest = opts.destination();
    emit(Event::Start {
        source: opts.source.display().to_string(),
        destination: dest.clone(),
        auth: auth_description.to_string(),
        concurrency: opts.concurrency,
        dry_run: opts.dry_run,
        verify: opts.verify,
        no_clobber: opts.no_clobber,
    });
    emit(Event::Phase { phase: "scanning" });

    let source = opts.source.clone();
    let excludes = opts.excludes.clone();
    let recursive = opts.recursive;
    let scanned = tokio::task::spawn_blocking(move || scan(&source, &excludes, recursive)).await??;
    for rel in scanned.online_only.iter().take(MAX_REPORTED_PLACEHOLDERS) {
        emit(Event::FileSkipped { path: rel.clone(), reason: onedrive::ONLINE_ONLY_REASON.into() });
    }

    let records = manifest.load_tree(&dest)?;
    let mut summary = Summary {
        local_files: scanned.files.len() as u64,
        local_bytes: scanned.files.iter().map(|f| f.size).sum(),
        excluded: scanned.excluded,
        unreadable: scanned.unreadable,
        online_only: scanned.online_only.len() as u64,
        ..Default::default()
    };

    let (known, unknown): (Vec<LocalFile>, Vec<LocalFile>) = scanned
        .files
        .into_iter()
        .partition(|f| records.get(&f.rel).is_some_and(|rec| rec.size == f.size && rec.mtime_ns == f.mtime_ns));
    let plan = |unchanged: u64, to_check: &[&LocalFile], listed: Option<u64>, estimate: bool| Event::Plan {
        local_files: summary.local_files,
        local_bytes: summary.local_bytes,
        excluded: summary.excluded,
        unchanged,
        to_check: to_check.len() as u64,
        to_check_bytes: to_check.iter().map(|f| f.size).sum(),
        listed_objects: listed,
        estimate,
    };
    // What's already synced, before any request.
    emit(plan(known.len() as u64, &unknown.iter().collect::<Vec<_>>(), None, true));

    // List the bucket where it can tell us something: everywhere with `verify`, otherwise only
    // the subfolders holding files the manifest doesn't know (e.g. after a stop, or files first
    // uploaded with gcloud), so they're never sent twice.
    let listed = Arc::new(AtomicU64::new(0));
    let remote = if opts.verify || !unknown.is_empty() {
        emit(Event::Phase { phase: "listing" });
        let only: Option<HashSet<String>> =
            if opts.verify { None } else { Some(unknown.iter().map(|f| top_segment(&f.rel).to_string()).collect()) };
        let ticker = {
            let (emit, listed) = (emit.clone(), listed.clone());
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    emit(Event::Progress {
                        phase: "listing",
                        files_done: 0,
                        files_total: 0,
                        bytes_done: 0,
                        bytes_total: 0,
                        hashed_files: 0,
                        hashed_bytes: 0,
                        listed_objects: listed.load(Ordering::Relaxed),
                        paused: false,
                        offline: false,
                    });
                }
            })
        };
        let result = list_remote(&client, &opts.bucket, &opts.prefix, only.as_ref(), &listed).await;
        ticker.abort();
        let objects = result?;
        summary.listed_objects = Some(listed.load(Ordering::Relaxed));
        Some(objects)
    } else {
        None
    };

    let mut candidates = Vec::new();
    for file in known {
        let rec = &records[&file.rel];
        let in_cloud = match (&remote, opts.verify) {
            (Some(remote), true) => remote.get(&file.rel).is_some_and(|o| {
                o.size == rec.size && o.crc32c.as_deref().and_then(crate::gcs::parse_crc32c_b64) == Some(rec.crc32c)
            }),
            _ => true,
        };
        if in_cloud {
            summary.unchanged += 1;
        } else {
            candidates.push(Candidate { known_crc: Some(rec.crc32c), file });
        }
    }
    for file in unknown {
        // Same size and recorded modified time: gcloud's rule, so no need to read the file. Only
        // for files the manifest knows nothing about: if it has a record, the file has changed
        // since (and the cloud keeps whole seconds, so a quick same-size edit could look equal).
        let by_time = remote.as_ref().filter(|_| !records.contains_key(&file.rel)).and_then(|r| r.get(&file.rel)).and_then(|o| {
            let crc = o.crc32c.as_deref().and_then(crate::gcs::parse_crc32c_b64)?;
            (o.size == file.size && o.file_mtime() == Some(file.mtime_secs())).then(|| (crc, o.generation.clone()))
        });
        match by_time {
            Some((crc, generation)) if !opts.verify => {
                // Previews record it too (see AlreadyInCloud).
                let record = FileRecord { size: file.size, mtime_ns: file.mtime_ns, crc32c: crc };
                let _ = manifest.record_by_time(&dest, &file.rel, &record, generation.as_deref());
                summary.already_in_cloud += 1;
                summary.matched_by_time += 1;
            }
            _ => candidates.push(Candidate { file, known_crc: None }),
        }
    }
    let to_check: Vec<&LocalFile> = candidates.iter().map(|c| &c.file).collect();
    emit(plan(summary.unchanged + summary.already_in_cloud, &to_check, summary.listed_objects, false));
    let to_check_bytes: u64 = to_check.iter().map(|f| f.size).sum();
    let files_total = candidates.len() as u64;

    // Many small files: more requests in flight. Very large files: bigger chunks.
    let mut concurrency = opts.concurrency.max(1);
    let mut chunk_size = opts.chunk_size;
    if opts.auto_tune && !candidates.is_empty() {
        let average = to_check_bytes / files_total;
        if files_total >= 200 && average < 512 * 1024 {
            concurrency = concurrency.max(64);
        }
        let largest = candidates.iter().map(|c| c.file.size).max().unwrap_or(0);
        if largest >= 8 << 30 {
            chunk_size = chunk_size.max(64 << 20);
        } else if largest >= 1 << 30 {
            chunk_size = chunk_size.max(32 << 20);
        }
        emit(Event::Tuned { concurrency, large_chunk_mib: chunk_size >> 20 });
    }

    // Parallel parts for big files, if the bucket allows it (checked once, only when needed).
    let composite_concurrency = opts.composite_concurrency.max(1);
    let composite_off = if opts.dry_run || !candidates.iter().any(|c| c.file.size >= opts.composite_threshold) {
        Some(String::new())
    } else if !opts.composite {
        Some("turned off".to_string())
    } else {
        let reason = composite_check(&client, &opts.bucket).await;
        if let Some(reason) = &reason {
            emit(Event::CompositeOff { reason: reason.clone() });
        }
        reason
    };

    let ctx = Arc::new(Ctx {
        client,
        emit: emit.clone(),
        manifest,
        large_slots: Semaphore::new(opts.large_concurrency.max(1)),
        chunk_size: (chunk_size / CHUNK_ALIGN).max(1) * CHUNK_ALIGN,
        opts,
        dest,
        records,
        remote,
        cancel,
        bytes_done: AtomicU64::new(0),
        live_bytes: Arc::new(AtomicU64::new(0)),
        files_done: AtomicU64::new(0),
        hashed_files: AtomicU64::new(0),
        decided_files: AtomicU64::new(0),
        hashed_bytes: AtomicU64::new(0),
        listed,
        phase: std::sync::Mutex::new("uploading"),
        net: Net::default(),
        composite_off: std::sync::Mutex::new(composite_off),
        part_slots: Semaphore::new(composite_concurrency),
        composite_files: AtomicU64::new(0),
        parts_left: AtomicU64::new(0),
    });
    emit(Event::Phase { phase: "uploading" });

    let progress = move |ctx: &Ctx, paused: bool| Event::Progress {
        phase: *ctx.phase.lock().unwrap(),
        files_done: ctx.files_done.load(Ordering::Relaxed),
        files_total,
        bytes_done: (ctx.bytes_done.load(Ordering::Relaxed) + ctx.live_bytes.load(Ordering::Relaxed)).min(to_check_bytes),
        bytes_total: to_check_bytes,
        hashed_files: ctx.hashed_files.load(Ordering::Relaxed) + ctx.decided_files.load(Ordering::Relaxed),
        hashed_bytes: ctx.hashed_bytes.load(Ordering::Relaxed),
        listed_objects: ctx.listed.load(Ordering::Relaxed),
        paused,
        offline: ctx.net.offline.load(Ordering::Relaxed),
    };
    let ticker = {
        let (ctx, emit) = (ctx.clone(), emit.clone());
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.tick().await;
            loop {
                interval.tick().await;
                emit(progress(&ctx, ctx.opts.throttle.is_paused()));
            }
        })
    };

    // Hashing feeds uploading, so upload slots never wait on reading files.
    let hash_concurrency = ctx.opts.hash_concurrency.max(1);
    let mut locked = Vec::new();
    {
        let mut results = stream::iter(candidates)
            .map(|candidate| {
                let ctx = ctx.clone();
                async move { (candidate.file.clone(), hash(&ctx, candidate).await) }
            })
            .buffer_unordered(hash_concurrency)
            .map(|(file, hashed)| {
                let ctx = ctx.clone();
                async move {
                    let outcome = match hashed {
                        Ok(hashed) => upload(&ctx, hashed).await,
                        Err(outcome) => outcome,
                    };
                    (file, outcome)
                }
            })
            .buffer_unordered(concurrency);
        while let Some((file, outcome)) = results.next().await {
            if let Outcome::Unreadable(error) = &outcome {
                if is_locked_error(error) && !ctx.cancelled() {
                    // In use by another program (e.g. open in Excel): try again at the end.
                    locked.push(file);
                    continue;
                }
            }
            ctx.files_done.fetch_add(1, Ordering::Relaxed);
            record_outcome(&mut summary, &emit, file, outcome);
        }
    }
    for file in locked {
        let outcome = if ctx.cancelled() {
            Outcome::Cancelled
        } else {
            match hash(&ctx, Candidate { file: file.clone(), known_crc: None }).await {
                Ok(hashed) => upload(&ctx, hashed).await,
                Err(outcome) => outcome,
            }
        };
        let outcome = match outcome {
            Outcome::Unreadable(error) if is_locked_error(&error) => Outcome::Unreadable(format!(
                "in use by another program (close it and sync again): {error}"
            )),
            other => other,
        };
        ctx.files_done.fetch_add(1, Ordering::Relaxed);
        record_outcome(&mut summary, &emit, file, outcome);
    }
    ticker.abort();
    emit(progress(&ctx, false));

    summary.hashed_files = ctx.hashed_files.load(Ordering::Relaxed);
    summary.composite_files = ctx.composite_files.load(Ordering::Relaxed);
    summary.parts_left = ctx.parts_left.load(Ordering::Relaxed);
    summary.hashed_bytes = ctx.hashed_bytes.load(Ordering::Relaxed);
    summary.cancelled |= ctx.cancelled();
    summary.elapsed_seconds = (started.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    emit(Event::Done { summary: summary.clone() });
    Ok(summary)
}

fn record_outcome(summary: &mut Summary, emit: &Emit, file: LocalFile, outcome: Outcome) {
    let (path, bytes) = (file.rel, file.size);
    match outcome {
        Outcome::Unchanged => summary.unchanged += 1,
        Outcome::AlreadyInCloud => summary.already_in_cloud += 1,
        Outcome::Uploaded { generation, resumed_from } => {
            summary.uploaded += 1;
            summary.uploaded_bytes += bytes;
            if resumed_from.is_some() {
                summary.resumed += 1;
            }
            emit(Event::FileUploaded { path, bytes, generation, resumed_from });
        }
        Outcome::WouldUpload => {
            summary.would_upload += 1;
            summary.would_upload_bytes += bytes;
            emit(Event::FileWouldUpload { path, bytes });
        }
        Outcome::SkippedExists => {
            summary.skipped_exists += 1;
            emit(Event::FileSkipped { path, reason: "already exists in bucket (no-clobber)".into() });
        }
        Outcome::Failed(error) => {
            summary.failed += 1;
            emit(Event::FileFailed { path, error });
        }
        Outcome::Unreadable(error) => {
            summary.unreadable += 1;
            emit(Event::FileFailed { path, error });
        }
        Outcome::Cancelled => summary.cancelled = true,
    }
}

/// Checksum a candidate (unless already known), keeping small files' bytes for the upload.
async fn hash(ctx: &Arc<Ctx>, candidate: Candidate) -> Result<Hashed, Outcome> {
    if ctx.cancelled() {
        return Err(Outcome::Cancelled);
    }
    let file = candidate.file;
    if let Some(crc) = candidate.known_crc {
        return Ok(Hashed { file, crc, data: None });
    }
    // A preview needn't read a file that uploads whatever its content: nothing by that name in
    // the cloud, or a different size. (A real sync reads it anyway, to upload it.)
    if ctx.opts.dry_run && ctx.remote.as_ref().is_some_and(|r| r.get(&file.rel).is_none_or(|o| o.size != file.size)) {
        ctx.decided_files.fetch_add(1, Ordering::Relaxed);
        return Err(Outcome::WouldUpload);
    }
    let abs = file.abs.clone();
    let size = file.size;
    let worker = ctx.clone();
    let result = tokio::task::spawn_blocking(move || -> std::io::Result<(u32, Option<Vec<u8>>)> {
        if size <= KEEP_IN_MEMORY {
            let data = std::fs::read(&abs)?;
            worker.hashed_bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
            Ok((crc32c::crc32c(&data), Some(data)))
        } else {
            Ok((crc32c_file(&abs, Some(&worker.hashed_bytes), Some(&worker.cancel))?, None))
        }
    })
    .await;
    ctx.hashed_files.fetch_add(1, Ordering::Relaxed);
    match result {
        Ok(Ok((crc, data))) => Ok(Hashed { file, crc, data }),
        Ok(Err(err)) if ctx.cancelled() => Err(if err.kind() == std::io::ErrorKind::Interrupted { Outcome::Cancelled } else { Outcome::Unreadable(format!("could not read file: {err}")) }),
        Ok(Err(err)) => Err(Outcome::Unreadable(format!("could not read file: {err}"))),
        Err(err) => Err(Outcome::Failed(err.to_string())),
    }
}

async fn upload(ctx: &Ctx, hashed: Hashed) -> Outcome {
    if ctx.cancelled() {
        return Outcome::Cancelled;
    }
    let Hashed { file, crc, data } = hashed;
    let record = FileRecord { size: file.size, mtime_ns: file.mtime_ns, crc32c: crc };

    // Only the timestamp changed (copied, touched): the content is already uploaded.
    if !ctx.opts.verify {
        if let Some(previous) = ctx.records.get(&file.rel) {
            if previous.size == record.size && previous.crc32c == record.crc32c {
                let _ = ctx.manifest.record(&ctx.dest, &file.rel, &record, None);
                return Outcome::Unchanged;
            }
        }
    }
    if let Some(object) = ctx.remote.as_ref().and_then(|remote| remote.get(&file.rel)) {
        if object.size == record.size && object.crc32c.as_deref().and_then(crate::gcs::parse_crc32c_b64) == Some(crc) {
            // Recorded by previews too: it's true either way, and the upload after a preview
            // then doesn't read the file again.
            let _ = ctx.manifest.record(&ctx.dest, &file.rel, &record, object.generation.as_deref());
            return Outcome::AlreadyInCloud;
        }
    }
    if ctx.opts.dry_run {
        return Outcome::WouldUpload;
    }

    let name = format!("{}{}", ctx.opts.prefix, file.rel);
    let result = if file.size > ctx.opts.resumable_threshold {
        let _slot = ctx.large_slots.acquire().await.expect("semaphore");
        if ctx.cancelled() {
            return Outcome::Cancelled;
        }
        let composite = if file.size >= ctx.opts.composite_threshold && ctx.composite_off.lock().unwrap().is_none() {
            upload_composite(ctx, &file, &record, &name).await
        } else {
            Ok(None)
        };
        match composite {
            Ok(Some(done)) => Ok(done),
            Ok(None) => {
                (ctx.emit)(Event::FileStarted { path: file.rel.clone(), bytes: file.size, parts: None });
                upload_resumable(ctx, &file, &record, &name).await
            }
            Err(err) => Err(err),
        }
    } else {
        if file.size >= REPORT_START {
            (ctx.emit)(Event::FileStarted { path: file.rel.clone(), bytes: file.size, parts: None });
        }
        upload_small(ctx, &file, &record, &name, data).await.map(|info| (info, None))
    };
    match result {
        Ok((info, resumed_from)) => {
            if let Err(err) = ctx.manifest.record(&ctx.dest, &file.rel, &record, info.generation.as_deref()) {
                return Outcome::Failed(format!("uploaded, but could not update manifest: {err}"));
            }
            Outcome::Uploaded { generation: info.generation, resumed_from }
        }
        Err(UploadError::Gcs(GcsError::AlreadyExists)) => Outcome::SkippedExists,
        Err(UploadError::Cancelled) => Outcome::Cancelled,
        Err(UploadError::Gcs(err)) => Outcome::Failed(err.to_string()),
        Err(UploadError::Io(err)) => Outcome::Unreadable(err),
    }
}

enum UploadError {
    Gcs(GcsError),
    Io(String),
    Cancelled,
}

impl From<GcsError> for UploadError {
    fn from(err: GcsError) -> Self {
        UploadError::Gcs(err)
    }
}

fn backoff(ctx: &Ctx, attempt: u32) -> Duration {
    let base = ctx.opts.retry_base.as_millis() as u64;
    let delay = base.saturating_mul(1 << attempt.min(5));
    Duration::from_millis(delay + rand::random_range(0..=base / 2 + 1))
}

fn retrying(ctx: &Ctx, file: &LocalFile, attempt: u32, error: &str) {
    (ctx.emit)(Event::FileRetrying {
        path: file.rel.clone(),
        attempt: attempt + 1,
        max_attempts: ctx.opts.max_attempts,
        error: error.chars().take(300).collect(),
    });
}

fn extras(ctx: &Ctx, file: &LocalFile) -> UploadExtras {
    UploadExtras {
        mtime_secs: Some(file.mtime_secs()),
        live: Some(ctx.live_bytes.clone()),
        throttle: Some(ctx.opts.throttle.clone()),
        cancel: Some(ctx.cancel.clone()),
    }
}

/// Wait while the sync is paused or the internet connection is down (or until it's stopped).
async fn wait_ready(ctx: &Ctx) {
    loop {
        ctx.opts.throttle.wait_while_paused(|| ctx.cancelled()).await;
        if ctx.cancelled() || !ctx.net.offline.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// After a network error: if Cloud Storage can't be reached at all, wait (checking every few
/// seconds, up to every 10 s) until it can, and return true so the failed attempt isn't counted.
/// Returns false when the connection is fine (a one-off failure, retried as usual).
async fn wait_for_network(ctx: &Ctx, error: &str) -> bool {
    let outages = ctx.net.outages.load(Ordering::Relaxed);
    let _checking = ctx.net.checking.lock().await;
    if ctx.net.outages.load(Ordering::Relaxed) != outages {
        return true; // failed during an outage another upload has just waited out
    }
    if ctx.client.reachable().await {
        return false;
    }
    ctx.net.offline.store(true, Ordering::Relaxed);
    (ctx.emit)(Event::Offline { error: error.chars().take(300).collect() });
    let since = Instant::now();
    let mut wait = Duration::from_secs(2);
    loop {
        let until = Instant::now() + wait;
        while Instant::now() < until && !ctx.cancelled() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if ctx.cancelled() || ctx.client.reachable().await {
            break;
        }
        wait = (wait * 2).min(Duration::from_secs(10));
    }
    ctx.net.offline.store(false, Ordering::Relaxed);
    ctx.net.outages.fetch_add(1, Ordering::Relaxed);
    (ctx.emit)(Event::Online { offline_seconds: since.elapsed().as_secs() });
    true
}

async fn upload_small(
    ctx: &Ctx,
    file: &LocalFile,
    record: &FileRecord,
    name: &str,
    mut kept: Option<Vec<u8>>,
) -> Result<ObjectInfo, UploadError> {
    let mut attempt = 0;
    loop {
        wait_ready(ctx).await;
        if ctx.cancelled() {
            return Err(UploadError::Cancelled);
        }
        let data = match kept.take() {
            Some(data) => data,
            None => tokio::fs::read(&file.abs).await.map_err(|e| UploadError::Io(format!("could not read file: {e}")))?,
        };
        if data.len() as u64 != record.size || crc32c::crc32c(&data) != record.crc32c {
            return Err(UploadError::Io("file changed while it was being uploaded; it will be retried on the next sync".into()));
        }
        match ctx.client.upload_multipart_with(&ctx.opts.bucket, name, data, record.crc32c, ctx.opts.no_clobber, &extras(ctx, file)).await {
            Ok(info) => {
                ctx.bytes_done.fetch_add(record.size, Ordering::Relaxed);
                return Ok(info);
            }
            // Paused or stopped mid-request: send it again once resumed (a stop ends above).
            Err(GcsError::Interrupted) => {}
            Err(GcsError::Network(error)) => {
                if !wait_for_network(ctx, &error).await {
                    if attempt + 1 >= ctx.opts.max_attempts {
                        return Err(GcsError::Network(error).into());
                    }
                    retrying(ctx, file, attempt, &error);
                    tokio::time::sleep(backoff(ctx, attempt)).await;
                    attempt += 1;
                }
            }
            Err(GcsError::Retryable(error)) if attempt + 1 < ctx.opts.max_attempts => {
                retrying(ctx, file, attempt, &error);
                tokio::time::sleep(backoff(ctx, attempt)).await;
                attempt += 1;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

async fn read_at(path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, UploadError> {
    let io = |e: std::io::Error| UploadError::Io(format!("could not read file: {e}"));
    let mut file = tokio::fs::File::open(path).await.map_err(io)?;
    file.seek(std::io::SeekFrom::Start(offset)).await.map_err(io)?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).await.map_err(io)?;
    Ok(buf)
}

/// Upload a large file through a resumable session, continuing a saved session if one matches.
async fn upload_resumable(
    ctx: &Ctx,
    file: &LocalFile,
    record: &FileRecord,
    name: &str,
) -> Result<(ObjectInfo, Option<u64>), UploadError> {
    let range = Range {
        name: name.to_string(),
        start: 0,
        len: record.size,
        crc: Some(record.crc32c),
        key: file.rel.clone(),
        no_clobber: ctx.opts.no_clobber,
        mtime_secs: Some(file.mtime_secs()),
        chunk: ctx.chunk_size,
    };
    upload_range(ctx, file, record, &range).await
}

/// A chunk being read from disk in the background.
type ReadAhead = tokio::task::JoinHandle<Result<Vec<u8>, UploadError>>;

/// Part of a file to upload as one object (the whole file, or one part of a composite upload).
struct Range {
    name: String,
    start: u64,
    len: u64,
    /// Checked by Cloud Storage when known (parts are checked as a whole after joining).
    crc: Option<u32>,
    /// Where its resumable session is saved in the manifest.
    key: String,
    no_clobber: bool,
    mtime_secs: Option<i64>,
    chunk: usize,
}

/// Upload `range` of `file` through a resumable session (continuing a saved one), reading the
/// next chunk from disk while the current one is being sent.
async fn upload_range(ctx: &Ctx, file: &LocalFile, record: &FileRecord, range: &Range) -> Result<(ObjectInfo, Option<u64>), UploadError> {
    let chunk = range.chunk;
    let size = range.len;
    let session_record = FileRecord { size, mtime_ns: record.mtime_ns, crc32c: record.crc32c };
    let mut resumed_from = None;
    let mut session: Option<String> = None;
    let mut offset = 0u64;

    if let Ok(Some(saved)) = ctx.manifest.session(&ctx.dest, &range.key) {
        if saved.size == size && saved.mtime_ns == record.mtime_ns && saved.crc32c == record.crc32c {
            match ctx.client.query_resumable_opt(&saved.session_uri, size, range.crc).await {
                Ok(ResumableStatus::Incomplete { offset: have }) => {
                    session = Some(saved.session_uri);
                    offset = have;
                    resumed_from = Some(have);
                    ctx.bytes_done.fetch_add(have, Ordering::Relaxed);
                }
                Ok(ResumableStatus::Complete(info)) => {
                    let _ = ctx.manifest.clear_session(&ctx.dest, &range.key);
                    ctx.bytes_done.fetch_add(size, Ordering::Relaxed);
                    return Ok((info, Some(size)));
                }
                Err(_) => {}
            }
        }
        if session.is_none() {
            let _ = ctx.manifest.clear_session(&ctx.dest, &range.key);
        }
    }

    let mut attempt = 0;
    let mut restarts = 0;
    // The next chunk, being read while the current one is sent.
    let mut ahead: Option<(u64, ReadAhead)> = None;
    loop {
        wait_ready(ctx).await;
        if ctx.cancelled() {
            // The session stays saved, so the next sync continues from `offset`.
            return Err(UploadError::Cancelled);
        }
        let uri = match &session {
            Some(uri) => uri.clone(),
            None => match ctx.client.start_resumable_opt(&ctx.opts.bucket, &range.name, size, range.crc, range.no_clobber, range.mtime_secs).await {
                Ok(uri) => {
                    let _ = ctx.manifest.save_session(&ctx.dest, &range.key, &session_record, &uri);
                    session = Some(uri.clone());
                    offset = 0;
                    uri
                }
                Err(GcsError::Network(error)) if wait_for_network(ctx, &error).await => continue,
                Err(GcsError::Retryable(error) | GcsError::Network(error)) if attempt + 1 < ctx.opts.max_attempts => {
                    retrying(ctx, file, attempt, &error);
                    tokio::time::sleep(backoff(ctx, attempt)).await;
                    attempt += 1;
                    continue;
                }
                Err(err) => return Err(err.into()),
            },
        };

        let len = chunk.min((size - offset) as usize);
        let data = match ahead.take() {
            Some((at, handle)) if at == offset => handle.await.map_err(|e| UploadError::Io(format!("could not read file: {e}")))??,
            other => {
                if let Some((_, handle)) = other {
                    handle.abort();
                }
                read_at(&file.abs, range.start + offset, len).await?
            }
        };
        let next = offset + len as u64;
        if next < size {
            let (path, at, next_len) = (file.abs.clone(), range.start + next, chunk.min((size - next) as usize));
            ahead = Some((next, tokio::spawn(async move { read_at(&path, at, next_len).await })));
        }
        match ctx.client.put_chunk_opt(&uri, offset, data, size, range.crc, &extras(ctx, file)).await {
            Ok(ResumableStatus::Incomplete { offset: have }) => {
                ctx.bytes_done.fetch_add(have.saturating_sub(offset), Ordering::Relaxed);
                offset = have;
                attempt = 0;
            }
            Ok(ResumableStatus::Complete(info)) => {
                ctx.bytes_done.fetch_add(size - offset, Ordering::Relaxed);
                let _ = ctx.manifest.clear_session(&ctx.dest, &range.key);
                return Ok((info, resumed_from));
            }
            Err(GcsError::SessionExpired) if restarts < 2 => {
                let _ = ctx.manifest.clear_session(&ctx.dest, &range.key);
                ctx.bytes_done.fetch_sub(offset.min(ctx.bytes_done.load(Ordering::Relaxed)), Ordering::Relaxed);
                session = None;
                restarts += 1;
            }
            // Paused or stopped mid-chunk, or the connection dropped and is back: ask Cloud
            // Storage how much it kept and continue from there (a stop ends at the top).
            Err(GcsError::Interrupted) => {
                wait_ready(ctx).await;
                if !ctx.cancelled() {
                    resync_offset(ctx, &uri, size, range.crc, &mut offset).await;
                }
            }
            Err(GcsError::Network(error)) if wait_for_network(ctx, &error).await => {
                resync_offset(ctx, &uri, size, range.crc, &mut offset).await;
            }
            Err(GcsError::Retryable(error) | GcsError::Network(error)) if attempt + 1 < ctx.opts.max_attempts => {
                retrying(ctx, file, attempt, &error);
                tokio::time::sleep(backoff(ctx, attempt)).await;
                attempt += 1;
                // Ask the server how much it kept before sending more.
                resync_offset(ctx, &uri, size, range.crc, &mut offset).await;
            }
            Err(err) => {
                if matches!(err, GcsError::Fatal(_) | GcsError::AlreadyExists) {
                    let _ = ctx.manifest.clear_session(&ctx.dest, &range.key);
                }
                return Err(err.into());
            }
        }
    }
}

/// Whether the bucket allows parallel parts; Some(reason) when it doesn't.
async fn composite_check(client: &GcsClient, bucket: &str) -> Option<String> {
    let info = match client.get_bucket(bucket).await {
        Ok(info) => info,
        Err(err) => return Some(format!("the bucket's settings couldn't be read ({err:#})")),
    };
    // A clear "no" turns it off; a check that can't run (e.g. an emulator) doesn't.
    let granted = client.test_permissions(bucket, &["storage.objects.delete"]).await.ok();
    composite_reason(&info, granted.as_deref())
}

/// The rules for parallel parts, given the bucket and the permissions granted (if known).
pub(crate) fn composite_reason(info: &crate::gcs::BucketInfo, granted: Option<&[String]>) -> Option<String> {
    if info.retention_seconds > 0 {
        return Some("the bucket has a retention policy, so the temporary parts couldn't be deleted".into());
    }
    let class = info.storage_class.as_deref().unwrap_or("STANDARD").to_ascii_uppercase();
    if !info.autoclass && !matches!(class.as_str(), "STANDARD" | "MULTI_REGIONAL" | "REGIONAL" | "") {
        return Some(format!("the bucket's storage class is {class}, where the temporary parts would be charged for early deletion"));
    }
    if granted.is_some_and(|g| !g.iter().any(|p| p == "storage.objects.delete")) {
        return Some("your account can't delete objects in this bucket, which removing the temporary parts needs".into());
    }
    None
}

/// Byte ranges of the parts: about `target` bytes each, 2 to 32 of them, aligned to 256 KiB.
pub(crate) fn part_layout(size: u64, target: u64) -> Vec<(u64, u64)> {
    let n = size.div_ceil(target.max(1)).clamp(2, 32);
    let align = CHUNK_ALIGN as u64;
    let part = size.div_ceil(n).div_ceil(align).max(1) * align;
    (0..n).map(|i| i * part).take_while(|start| *start < size).map(|start| (start, part.min(size - start))).collect()
}

fn composite_off(ctx: &Ctx, reason: String) {
    let mut off = ctx.composite_off.lock().unwrap();
    if off.is_none() {
        (ctx.emit)(Event::CompositeOff { reason: reason.clone() });
        *off = Some(reason);
    }
}

/// Upload a big file as parts in parallel, join them in the bucket and check the result's
/// checksum against the file's. Ok(None): use a single stream instead (not allowed here, or
/// the joined object didn't match). Parts already uploaded by an earlier run are reused.
async fn upload_composite(ctx: &Ctx, file: &LocalFile, record: &FileRecord, name: &str) -> Result<Option<(ObjectInfo, Option<u64>)>, UploadError> {
    let parts = part_layout(file.size, ctx.opts.composite_part_size);
    let dir = format!(
        "{}{}parts/{:08x}-{}-{:08x}/",
        ctx.opts.prefix,
        crate::receipt::JETSTREAM_PREFIX,
        crc32c::crc32c(name.as_bytes()),
        file.size,
        record.crc32c
    );
    let names: Vec<String> = (0..parts.len()).map(|i| format!("{dir}{i:02}")).collect();
    // Never overwriting, and the bucket listing shows it's there: nothing to send.
    if ctx.opts.no_clobber && ctx.remote.as_ref().is_some_and(|r| r.contains_key(&file.rel)) {
        return Err(GcsError::AlreadyExists.into());
    }
    // Parts an earlier run finished (same file: the folder name has its size and checksum).
    let existing: HashMap<String, u64> = match ctx.client.list(&ctx.opts.bucket, &dir).await {
        Ok(objects) => objects.into_iter().map(|o| (o.name, o.size)).collect(),
        Err(_) => HashMap::new(),
    };
    (ctx.emit)(Event::FileStarted { path: file.rel.clone(), bytes: file.size, parts: Some(parts.len() as u32) });
    let chunk = ctx.chunk_size.min(16 << 20);

    // All parts at once; `part_slots` limits how many send at the same time.
    let mut tasks = Vec::with_capacity(parts.len());
    for (i, (&(start, len), part_name)) in parts.iter().zip(&names).enumerate() {
        let reused = existing.get(part_name) == Some(&len);
        tasks.push(async move {
                if reused {
                    ctx.bytes_done.fetch_add(len, Ordering::Relaxed);
                    return Ok(len);
                }
                let _slot = ctx.part_slots.acquire().await.expect("semaphore");
                let range = Range {
                    name: part_name.clone(),
                    start,
                    len,
                    crc: None,
                    key: format!("{}#part{i}", file.rel),
                    no_clobber: false,
                    mtime_secs: None,
                    chunk,
                };
                upload_range(ctx, file, record, &range).await.map(|_| 0)
        });
    }
    let results: Vec<Result<u64, UploadError>> = futures::future::join_all(tasks).await;
    let mut reused = 0;
    for result in results {
        match result {
            Ok(n) => reused += n,
            // A part the bucket won't take (e.g. no permission for _jetstream/): one stream.
            Err(UploadError::Gcs(GcsError::Fatal(error))) => {
                composite_off(ctx, format!("a part couldn't be uploaded ({error})"));
                ctx.bytes_done.fetch_sub(file.size.min(ctx.bytes_done.load(Ordering::Relaxed)), Ordering::Relaxed);
                return Ok(None);
            }
            // Stopped, unreadable, or out of retries: the parts that finished are reused next time.
            Err(err) => return Err(err),
        }
    }

    // Join them (retrying a dropped connection), then check the whole file's checksum.
    let mut attempt = 0;
    let info = loop {
        wait_ready(ctx).await;
        if ctx.cancelled() {
            return Err(UploadError::Cancelled);
        }
        match ctx.client.compose(&ctx.opts.bucket, name, &names, ctx.opts.no_clobber, Some(file.mtime_secs())).await {
            Ok(info) => break info,
            Err(GcsError::AlreadyExists) => {
                delete_parts(ctx, &names).await;
                return Err(GcsError::AlreadyExists.into());
            }
            Err(GcsError::Network(error)) if wait_for_network(ctx, &error).await => {}
            Err(GcsError::Retryable(error) | GcsError::Network(error)) if attempt + 1 < ctx.opts.max_attempts => {
                retrying(ctx, file, attempt, &error);
                tokio::time::sleep(backoff(ctx, attempt)).await;
                attempt += 1;
            }
            Err(GcsError::Fatal(error)) => {
                composite_off(ctx, format!("joining the parts failed ({error})"));
                ctx.bytes_done.fetch_sub(file.size.min(ctx.bytes_done.load(Ordering::Relaxed)), Ordering::Relaxed);
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        }
    };
    let matches = info.size == file.size && info.crc32c.as_deref().and_then(crate::gcs::parse_crc32c_b64) == Some(record.crc32c);
    delete_parts(ctx, &names).await;
    if !matches {
        // Never keep a joined object that isn't exactly the file: upload it again in one stream.
        let _ = ctx.client.delete_object(&ctx.opts.bucket, name, info.generation.as_deref()).await;
        (ctx.emit)(Event::FileRetrying {
            path: file.rel.clone(),
            attempt: 1,
            max_attempts: ctx.opts.max_attempts,
            error: "the joined parts didn't match the file's checksum; uploading it again as one stream".into(),
        });
        ctx.bytes_done.fetch_sub(file.size.min(ctx.bytes_done.load(Ordering::Relaxed)), Ordering::Relaxed);
        return Ok(None);
    }
    ctx.composite_files.fetch_add(1, Ordering::Relaxed);
    Ok(Some((info, (reused > 0).then_some(reused))))
}

/// Remove a composite upload's temporary parts (best effort; leftovers are counted).
async fn delete_parts(ctx: &Ctx, names: &[String]) {
    let deletes = names.iter().map(|name| ctx.client.delete_object(&ctx.opts.bucket, name, None));
    let failed = futures::future::join_all(deletes).await.iter().filter(|r| r.is_err()).count();
    ctx.parts_left.fetch_add(failed as u64, Ordering::Relaxed);
}

/// Move `offset` to what Cloud Storage has kept of a resumable upload.
async fn resync_offset(ctx: &Ctx, uri: &str, size: u64, crc: Option<u32>, offset: &mut u64) {
    if let Ok(ResumableStatus::Incomplete { offset: have }) = ctx.client.query_resumable_opt(uri, size, crc).await {
        if have >= *offset {
            ctx.bytes_done.fetch_add(have - *offset, Ordering::Relaxed);
        } else {
            ctx.bytes_done.fetch_sub((*offset - have).min(ctx.bytes_done.load(Ordering::Relaxed)), Ordering::Relaxed);
        }
        *offset = have;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_destinations() {
        assert_eq!(parse_destination("gs://bucket/a/b/").unwrap(), ("bucket".into(), "a/b/".into()));
        assert_eq!(parse_destination("bucket/a").unwrap(), ("bucket".into(), "a/".into()));
        assert_eq!(parse_destination("gs://bucket").unwrap(), ("bucket".into(), "".into()));
        assert!(parse_destination("gs://").is_err());
    }

    #[test]
    fn scan_uses_forward_slashes_and_excludes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub/_archive")).unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"1").unwrap();
        std::fs::write(dir.path().join("sub/b.jpg"), b"22").unwrap();
        std::fs::write(dir.path().join("sub/c.tmp"), b"3").unwrap();
        std::fs::write(dir.path().join("sub/_archive/d.jpg"), b"4").unwrap();
        let excludes = vec![Regex::new(r".*\.tmp$").unwrap(), Regex::new(r"(^|/)_archive/").unwrap()];
        let scanned = scan(dir.path(), &excludes, true).unwrap();
        let mut rels: Vec<_> = scanned.files.iter().map(|f| f.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, ["a.jpg", "sub/b.jpg"]);
        assert_eq!((scanned.excluded, scanned.unreadable, scanned.online_only.len()), (2, 0, 0));

        let top_level = scan(dir.path(), &excludes, false).unwrap();
        assert_eq!(top_level.files.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(), ["a.jpg"]);
    }

    #[test]
    fn part_layout_covers_the_file_in_aligned_parts() {
        let mib = 1024 * 1024;
        for (size, target) in [(300 * mib, 32 * mib), (256 * mib, 32 * mib), (50 * 1024 * mib, 32 * mib), (6 * mib + 123, mib), (3 * mib, 32 * mib)] {
            let parts = part_layout(size, target);
            assert!((2..=32).contains(&parts.len()), "{size}: {} parts", parts.len());
            assert_eq!(parts[0].0, 0);
            for pair in parts.windows(2) {
                assert_eq!(pair[0].0 + pair[0].1, pair[1].0, "contiguous");
                assert_eq!(pair[0].1 % CHUNK_ALIGN as u64, 0, "aligned");
            }
            let last = parts.last().unwrap();
            assert_eq!(last.0 + last.1, size, "covers the whole file");
        }
        assert_eq!(part_layout(300 * mib, 32 * mib).len(), 10);
        assert_eq!(part_layout(50 * 1024 * mib, 32 * mib).len(), 32, "at most 32 (one compose)");
    }

    #[test]
    fn parallel_parts_only_where_the_bucket_allows_them() {
        use crate::gcs::BucketInfo;
        let standard = BucketInfo { storage_class: Some("STANDARD".into()), ..Default::default() };
        assert_eq!(composite_reason(&standard, None), None);
        assert_eq!(composite_reason(&standard, Some(&["storage.objects.delete".to_string()])), None);
        assert!(composite_reason(&standard, Some(&[])).unwrap().contains("can't delete"));
        let cold = BucketInfo { storage_class: Some("NEARLINE".into()), ..Default::default() };
        assert!(composite_reason(&cold, None).unwrap().contains("NEARLINE"));
        assert_eq!(composite_reason(&BucketInfo { autoclass: true, ..cold }, None), None, "Autoclass charges no early deletion");
        let locked = BucketInfo { retention_seconds: 86_400, ..standard };
        assert!(composite_reason(&locked, None).unwrap().contains("retention"));
    }

    #[test]
    fn helpers() {
        assert_eq!(top_segment("photos/a/b.jpg"), "photos");
        assert_eq!(top_segment("top.txt"), "");
        assert!(is_locked_error("could not read file: The process cannot access the file because it is being used by another process. (os error 32)"));
        assert!(!is_locked_error("could not read file: permission denied (os error 13)"));
        let f = LocalFile { abs: PathBuf::new(), rel: String::new(), size: 0, mtime_ns: 1_700_000_000_999_999_999 };
        assert_eq!(f.mtime_secs(), 1_700_000_000);
    }
}
