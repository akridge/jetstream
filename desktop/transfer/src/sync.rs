//! Incremental folder → Cloud Storage sync.
//!
//! 1. Walk the folder (size + mtime only).
//! 2. Files whose size and mtime match the manifest are unchanged: no hashing, no requests.
//! 3. Everything else is hashed (crc32c). On the first sync to a destination, or with
//!    `verify`, the bucket prefix is listed once so files already there aren't re-sent.
//! 4. Uploads run in parallel: small files in one request, large files as resumable
//!    sessions whose URI is saved, so an interrupted file continues where it stopped.
//!    Every object's crc32c is checked by Cloud Storage and again on the response.

use std::collections::HashMap;
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

use crate::gcs::{GcsClient, GcsError, ObjectInfo, ResumableStatus, CHUNK_ALIGN};
use crate::manifest::{FileRecord, Manifest};
use crate::throttle::Throttle;

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
    pub uploaded: u64,
    pub uploaded_bytes: u64,
    pub resumed: u64,
    pub would_upload: u64,
    pub would_upload_bytes: u64,
    pub skipped_exists: u64,
    pub failed: u64,
    pub unreadable: u64,
    pub listed_objects: Option<u64>,
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
    Plan {
        local_files: u64,
        local_bytes: u64,
        excluded: u64,
        unchanged: u64,
        to_check: u64,
        to_check_bytes: u64,
        listed_objects: Option<u64>,
    },
    /// A large (resumable) upload started; small files only report when done.
    FileStarted { path: String, bytes: u64 },
    FileUploaded { path: String, bytes: u64, generation: Option<String>, resumed_from: Option<u64> },
    FileWouldUpload { path: String, bytes: u64 },
    FileSkipped { path: String, reason: String },
    FileFailed { path: String, error: String },
    Progress { files_done: u64, files_total: u64, bytes_done: u64, bytes_total: u64, paused: bool },
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

struct Candidate {
    file: LocalFile,
    known_crc: Option<u32>,
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
    bytes_done: AtomicU64,
    files_done: AtomicU64,
}

pub(crate) fn scan(source: &Path, excludes: &[Regex], recursive: bool) -> Result<(Vec<LocalFile>, u64, u64)> {
    if !source.is_dir() {
        bail!("{} is not a folder", source.display());
    }
    let mut files = Vec::new();
    let mut excluded = 0;
    let mut unreadable = 0;
    let walker = walkdir::WalkDir::new(source).follow_links(false).max_depth(if recursive { usize::MAX } else { 1 });
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                unreadable += 1;
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
            excluded += 1;
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            unreadable += 1;
            continue;
        };
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        files.push(LocalFile { abs: entry.into_path(), rel, size: meta.len(), mtime_ns });
    }
    Ok((files, excluded, unreadable))
}

fn crc32c_file(path: &Path) -> std::io::Result<u32> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1024 * 1024];
    let mut crc = 0u32;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(crc);
        }
        crc = crc32c::crc32c_append(crc, &buf[..n]);
    }
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

    let source = opts.source.clone();
    let excludes = opts.excludes.clone();
    let recursive = opts.recursive;
    let (files, excluded, unreadable) = tokio::task::spawn_blocking(move || scan(&source, &excludes, recursive)).await??;

    let records = manifest.load(&dest)?;
    let mut summary = Summary {
        local_files: files.len() as u64,
        local_bytes: files.iter().map(|f| f.size).sum(),
        excluded,
        unreadable,
        ..Default::default()
    };

    // One listing on the first sync to this destination (or when verifying), so files that
    // are already in the bucket, e.g. from an earlier gcloud rsync, aren't uploaded again.
    let remote = if opts.verify || records.is_empty() {
        let objects = client.list(&opts.bucket, &opts.prefix).await?;
        summary.listed_objects = Some(objects.len() as u64);
        Some(
            objects
                .into_iter()
                .filter_map(|o| o.name.strip_prefix(&opts.prefix).map(str::to_string).map(|rel| (rel, o)))
                .collect::<HashMap<_, _>>(),
        )
    } else {
        None
    };

    let mut candidates = Vec::new();
    for file in files {
        match records.get(&file.rel) {
            Some(rec) if rec.size == file.size && rec.mtime_ns == file.mtime_ns => {
                let in_cloud = remote.as_ref().map_or(true, |remote| {
                    remote.get(&file.rel).is_some_and(|o| {
                        o.size == rec.size && o.crc32c.as_deref().and_then(crate::gcs::parse_crc32c_b64) == Some(rec.crc32c)
                    })
                });
                if in_cloud {
                    summary.unchanged += 1;
                } else {
                    candidates.push(Candidate { known_crc: Some(rec.crc32c), file });
                }
            }
            _ => candidates.push(Candidate { file, known_crc: None }),
        }
    }
    let to_check_bytes: u64 = candidates.iter().map(|c| c.file.size).sum();
    emit(Event::Plan {
        local_files: summary.local_files,
        local_bytes: summary.local_bytes,
        excluded,
        unchanged: summary.unchanged,
        to_check: candidates.len() as u64,
        to_check_bytes,
        listed_objects: summary.listed_objects,
    });

    let ctx = Arc::new(Ctx {
        client,
        emit: emit.clone(),
        manifest,
        large_slots: Semaphore::new(opts.large_concurrency.max(1)),
        opts,
        dest,
        records,
        remote,
        cancel,
        bytes_done: AtomicU64::new(0),
        files_done: AtomicU64::new(0),
    });

    // Progress ticker.
    let files_total = candidates.len() as u64;
    let ticker_ctx = ctx.clone();
    let ticker_emit = emit.clone();
    let ticker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.tick().await;
        loop {
            interval.tick().await;
            ticker_emit(Event::Progress {
                files_done: ticker_ctx.files_done.load(Ordering::Relaxed),
                files_total,
                bytes_done: ticker_ctx.bytes_done.load(Ordering::Relaxed),
                bytes_total: to_check_bytes,
                paused: ticker_ctx.opts.throttle.is_paused(),
            });
        }
    });

    let concurrency = ctx.opts.concurrency.max(1);
    let mut results = stream::iter(candidates)
        .map(|candidate| {
            let ctx = ctx.clone();
            async move {
                let rel = candidate.file.rel.clone();
                let size = candidate.file.size;
                let outcome = process(&ctx, candidate).await;
                ctx.files_done.fetch_add(1, Ordering::Relaxed);
                (rel, size, outcome)
            }
        })
        .buffer_unordered(concurrency);

    while let Some((path, bytes, outcome)) = results.next().await {
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
    ticker.abort();
    emit(Event::Progress {
        files_done: ctx.files_done.load(Ordering::Relaxed),
        files_total,
        bytes_done: ctx.bytes_done.load(Ordering::Relaxed),
        bytes_total: to_check_bytes,
        paused: false,
    });

    summary.elapsed_seconds = (started.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    emit(Event::Done { summary: summary.clone() });
    Ok(summary)
}

async fn process(ctx: &Ctx, candidate: Candidate) -> Outcome {
    if ctx.cancel.load(Ordering::Relaxed) {
        return Outcome::Cancelled;
    }
    let file = candidate.file;
    let crc = match candidate.known_crc {
        Some(crc) => crc,
        None => {
            let abs = file.abs.clone();
            match tokio::task::spawn_blocking(move || crc32c_file(&abs)).await {
                Ok(Ok(crc)) => crc,
                Ok(Err(err)) => return Outcome::Unreadable(format!("could not read file: {err}")),
                Err(err) => return Outcome::Failed(err.to_string()),
            }
        }
    };
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
            if !ctx.opts.dry_run {
                let _ = ctx.manifest.record(&ctx.dest, &file.rel, &record, object.generation.as_deref());
            }
            return Outcome::AlreadyInCloud;
        }
    }
    if ctx.opts.dry_run {
        return Outcome::WouldUpload;
    }

    let name = format!("{}{}", ctx.opts.prefix, file.rel);
    let result = if file.size > ctx.opts.resumable_threshold {
        let _slot = ctx.large_slots.acquire().await.expect("semaphore");
        (ctx.emit)(Event::FileStarted { path: file.rel.clone(), bytes: file.size });
        upload_resumable(ctx, &file, &record, &name).await
    } else {
        upload_small(ctx, &file, &record, &name).await.map(|info| (info, None))
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

async fn upload_small(ctx: &Ctx, file: &LocalFile, record: &FileRecord, name: &str) -> Result<ObjectInfo, UploadError> {
    let mut attempt = 0;
    loop {
        let data = tokio::fs::read(&file.abs).await.map_err(|e| UploadError::Io(format!("could not read file: {e}")))?;
        if data.len() as u64 != record.size || crc32c::crc32c(&data) != record.crc32c {
            return Err(UploadError::Io("file changed while it was being uploaded; it will be retried on the next sync".into()));
        }
        ctx.opts.throttle.take(record.size, || ctx.cancel.load(Ordering::Relaxed)).await;
        match ctx.client.upload_multipart(&ctx.opts.bucket, name, data, record.crc32c, ctx.opts.no_clobber).await {
            Ok(info) => {
                ctx.bytes_done.fetch_add(record.size, Ordering::Relaxed);
                return Ok(info);
            }
            Err(GcsError::Retryable(_)) if attempt + 1 < ctx.opts.max_attempts => {
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
    let chunk = (ctx.opts.chunk_size / CHUNK_ALIGN).max(1) * CHUNK_ALIGN;
    let mut resumed_from = None;
    let mut session: Option<String> = None;
    let mut offset = 0u64;

    if let Ok(Some(saved)) = ctx.manifest.session(&ctx.dest, &file.rel) {
        if saved.size == record.size && saved.mtime_ns == record.mtime_ns && saved.crc32c == record.crc32c {
            match ctx.client.query_resumable(&saved.session_uri, record.size, record.crc32c).await {
                Ok(ResumableStatus::Incomplete { offset: have }) => {
                    session = Some(saved.session_uri);
                    offset = have;
                    resumed_from = Some(have);
                }
                Ok(ResumableStatus::Complete(info)) => {
                    let _ = ctx.manifest.clear_session(&ctx.dest, &file.rel);
                    ctx.bytes_done.fetch_add(record.size, Ordering::Relaxed);
                    return Ok((info, Some(record.size)));
                }
                Err(_) => {}
            }
        }
        if session.is_none() {
            let _ = ctx.manifest.clear_session(&ctx.dest, &file.rel);
        }
    }

    let mut attempt = 0;
    let mut restarts = 0;
    loop {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(UploadError::Cancelled);
        }
        let uri = match &session {
            Some(uri) => uri.clone(),
            None => match ctx
                .client
                .start_resumable(&ctx.opts.bucket, name, record.size, record.crc32c, ctx.opts.no_clobber)
                .await
            {
                Ok(uri) => {
                    let _ = ctx.manifest.save_session(&ctx.dest, &file.rel, record, &uri);
                    session = Some(uri.clone());
                    offset = 0;
                    uri
                }
                Err(GcsError::Retryable(_)) if attempt + 1 < ctx.opts.max_attempts => {
                    tokio::time::sleep(backoff(ctx, attempt)).await;
                    attempt += 1;
                    continue;
                }
                Err(err) => return Err(err.into()),
            },
        };

        let len = chunk.min((record.size - offset) as usize);
        let data = read_at(&file.abs, offset, len).await?;
        ctx.opts.throttle.take(data.len() as u64, || ctx.cancel.load(Ordering::Relaxed)).await;
        match ctx.client.put_chunk(&uri, offset, data, record.size, record.crc32c).await {
            Ok(ResumableStatus::Incomplete { offset: have }) => {
                ctx.bytes_done.fetch_add(have.saturating_sub(offset), Ordering::Relaxed);
                offset = have;
                attempt = 0;
            }
            Ok(ResumableStatus::Complete(info)) => {
                ctx.bytes_done.fetch_add(record.size - offset, Ordering::Relaxed);
                let _ = ctx.manifest.clear_session(&ctx.dest, &file.rel);
                return Ok((info, resumed_from));
            }
            Err(GcsError::SessionExpired) if restarts < 2 => {
                let _ = ctx.manifest.clear_session(&ctx.dest, &file.rel);
                ctx.bytes_done.fetch_sub(offset.min(ctx.bytes_done.load(Ordering::Relaxed)), Ordering::Relaxed);
                session = None;
                restarts += 1;
            }
            Err(GcsError::Retryable(_)) if attempt + 1 < ctx.opts.max_attempts => {
                tokio::time::sleep(backoff(ctx, attempt)).await;
                attempt += 1;
                // Ask the server how much it kept before sending more.
                if let Ok(ResumableStatus::Incomplete { offset: have }) =
                    ctx.client.query_resumable(&uri, record.size, record.crc32c).await
                {
                    offset = have;
                }
            }
            Err(err) => {
                if matches!(err, GcsError::Fatal(_) | GcsError::AlreadyExists) {
                    let _ = ctx.manifest.clear_session(&ctx.dest, &file.rel);
                }
                return Err(err.into());
            }
        }
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
        let (files, excluded, unreadable) = scan(dir.path(), &excludes, true).unwrap();
        let mut rels: Vec<_> = files.iter().map(|f| f.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, ["a.jpg", "sub/b.jpg"]);
        assert_eq!((excluded, unreadable), (2, 0));

        let (top_level, _, _) = scan(dir.path(), &excludes, false).unwrap();
        assert_eq!(top_level.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(), ["a.jpg"]);
    }
}
