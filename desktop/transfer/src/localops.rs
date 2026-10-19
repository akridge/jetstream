//! Moving data between the cloud and this computer outside of folder syncs: downloading
//! files or folders, and uploading individual files (e.g. dropped onto a cloud folder).
//!
//! Downloads are written to a temporary ".jetstream-part" file, checked against the cloud
//! checksum and size, and only then renamed into place; existing local files are never
//! replaced. Uploads never overwrite objects already in the cloud.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{bail, Result};
use futures::stream::{self, StreamExt};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::browse::folder_prefix;
use crate::fileops::{parse_rfc3339, retry, Failure, Item, OpKind, OpProgress, OpResult};
use crate::gcs::{crc32c_b64, GcsClient, GcsError, ObjectInfo, ResumableStatus, CHUNK_ALIGN};
use crate::throttle::Throttle;

const CONCURRENCY: usize = 8;
const MAX_REPORTED_FAILURES: usize = 200;
/// Files up to this size upload in one request; larger ones in resumable chunks.
const SINGLE_REQUEST_LIMIT: u64 = 8 * 1024 * 1024;
const UPLOAD_CHUNK: usize = 32 * CHUNK_ALIGN; // 8 MiB
const PART_SUFFIX: &str = ".jetstream-part";

struct Report<'a> {
    progress: &'a OpProgress,
    failures: Mutex<Vec<Failure>>,
}

impl<'a> Report<'a> {
    fn new(progress: &'a OpProgress) -> Self {
        Self { progress, failures: Mutex::new(Vec::new()) }
    }
    fn done(&self, bytes: u64) {
        self.progress.done.fetch_add(1, Ordering::Relaxed);
        self.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    fn skipped(&self) {
        self.progress.skipped.fetch_add(1, Ordering::Relaxed);
    }
    fn failed(&self, path: String, error: String) {
        self.progress.failed.fetch_add(1, Ordering::Relaxed);
        let mut list = self.failures.lock().unwrap();
        if list.len() < MAX_REPORTED_FAILURES {
            list.push(Failure { path, error });
        }
    }
    fn finish(self, kind: OpKind, started: Instant) -> OpResult {
        let p = self.progress;
        OpResult {
            kind: Some(kind),
            total: p.total.load(Ordering::Relaxed),
            done: p.done.load(Ordering::Relaxed),
            skipped: p.skipped.load(Ordering::Relaxed),
            failed: p.failed.load(Ordering::Relaxed),
            failures: self.failures.into_inner().unwrap(),
            cancelled: p.cancel.load(Ordering::Relaxed),
            undoable: false,
            elapsed_seconds: started.elapsed().as_secs_f64(),
            undo: None,
        }
    }
}

// ----- download ---------------------------------------------------------------------------

/// One path component of an object name, made safe as a local file or folder name.
/// `None` for components that must never be used ("", ".", "..").
pub fn safe_component(part: &str) -> Option<String> {
    if part.is_empty() || part == "." || part == ".." {
        return None;
    }
    let windows = cfg!(windows);
    let mut out: String = part
        .chars()
        .map(|c| if c.is_control() || c == '\\' || (windows && "<>:\"|?*".contains(c)) { '_' } else { c })
        .collect();
    if windows && (out.ends_with('.') || out.ends_with(' ')) {
        // Windows drops trailing dots and spaces, which would merge different names.
        out.push('_');
    }
    Some(out)
}

/// Where an object goes under `dest`: `rel` is its name relative to what was selected.
/// Fails for names that would escape the destination.
pub fn local_path(dest: &Path, rel: &str) -> Result<PathBuf> {
    let mut path = dest.to_path_buf();
    let mut parts = 0;
    for part in rel.split('/') {
        if part.is_empty() {
            continue;
        }
        match safe_component(part) {
            Some(safe) => path.push(safe),
            None => bail!("unsafe name (contains \"..\")"),
        }
        parts += 1;
    }
    if parts == 0 {
        bail!("empty name");
    }
    Ok(path)
}

struct DownloadTask {
    object: ObjectInfo,
    gzip: bool,
    local: PathBuf,
}

/// Download objects and folders (recursively) from `bucket` into `dest_dir`.
pub async fn download(
    client: &GcsClient,
    bucket: &str,
    items: &[Item],
    dest_dir: &Path,
    throttle: &Throttle,
    progress: &OpProgress,
) -> Result<OpResult> {
    let started = Instant::now();
    if !dest_dir.is_dir() {
        bail!("{} is not a folder", dest_dir.display());
    }
    let report = Report::new(progress);
    let mut tasks = Vec::new();
    let mut folders = Vec::new();
    for item in items {
        let base = item.path.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
        if item.path.ends_with('/') {
            let prefix = folder_prefix(&item.path);
            let mut token: Option<String> = None;
            loop {
                let (page, next) = client
                    .list_raw(bucket, &prefix, token.as_deref(), "name,size,crc32c,generation,updated,contentEncoding", false)
                    .await?;
                for raw in page {
                    let gzip = raw["contentEncoding"].as_str() == Some("gzip");
                    let Ok(object) = serde_json::from_value::<ObjectInfo>(raw) else { continue };
                    let rel = format!("{base}/{}", &object.name[prefix.len()..]);
                    match local_path(dest_dir, &rel) {
                        // Folder placeholder objects become (empty) folders.
                        Ok(local) if object.name.ends_with('/') => folders.push(local),
                        Ok(local) => tasks.push(DownloadTask { object, gzip, local }),
                        Err(err) => report.failed(format!("gs://{bucket}/{}", object.name), err.to_string()),
                    }
                }
                match next {
                    Some(next) => token = Some(next),
                    None => break,
                }
            }
            folders.push(dest_dir.join(safe_component(&base).unwrap_or_else(|| "download".into())));
        } else {
            let raw = client.get_object_raw(bucket, &item.path).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            let Some(raw) = raw else {
                report.failed(format!("gs://{bucket}/{}", item.path), "no longer exists".into());
                continue;
            };
            let gzip = raw["contentEncoding"].as_str() == Some("gzip");
            let object: ObjectInfo = serde_json::from_value(raw)?;
            match local_path(dest_dir, &base) {
                Ok(local) => tasks.push(DownloadTask { object, gzip, local }),
                Err(err) => report.failed(format!("gs://{bucket}/{}", item.path), err.to_string()),
            }
        }
    }
    for folder in folders {
        let _ = tokio::fs::create_dir_all(folder).await;
    }
    progress.total.store(tasks.len() as u64 + progress.failed.load(Ordering::Relaxed), Ordering::Relaxed);

    stream::iter(tasks.iter())
        .for_each_concurrent(CONCURRENCY, |task| {
            let report = &report;
            async move {
                if progress.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let path = format!("gs://{bucket}/{}", task.object.name);
                match download_one(client, bucket, task, throttle, progress).await {
                    Ok(Some(bytes)) => report.done(bytes),
                    Ok(None) => report.skipped(),
                    Err(error) => report.failed(path, error),
                }
            }
        })
        .await;
    Ok(report.finish(OpKind::Download, started))
}

/// Ok(Some(bytes)) downloaded, Ok(None) already there, Err(reason) failed.
async fn download_one(
    client: &GcsClient,
    bucket: &str,
    task: &DownloadTask,
    throttle: &Throttle,
    progress: &OpProgress,
) -> Result<Option<u64>, String> {
    let object = &task.object;
    if let Ok(meta) = tokio::fs::metadata(&task.local).await {
        if meta.is_file() && meta.len() == object.size {
            return Ok(None);
        }
        return Err(format!("{} already exists here and differs; it was left unchanged", task.local.display()));
    }
    if let Some(parent) = task.local.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| format!("can't create {}: {e}", parent.display()))?;
    }
    let part = PathBuf::from(format!("{}{PART_SUFFIX}", task.local.display()));
    let mut attempt = 0;
    loop {
        match fetch(client, bucket, object, &part, throttle, progress).await {
            Ok(crc) => {
                // Objects stored gzip-compressed arrive decompressed, so their checksum differs.
                let expected = object.crc32c.as_deref().filter(|_| !task.gzip);
                if let Some(expected) = expected {
                    if crc32c_b64(crc) != expected {
                        let _ = tokio::fs::remove_file(&part).await;
                        if attempt < 2 {
                            attempt += 1;
                            continue;
                        }
                        return Err("downloaded data didn't match the cloud checksum".into());
                    }
                }
                tokio::fs::rename(&part, &task.local).await.map_err(|e| format!("can't save {}: {e}", task.local.display()))?;
                if let Some(updated) = object.updated.as_deref().and_then(parse_rfc3339) {
                    // Keep the cloud's modified time, as gsutil/gcloud do.
                    let time = UNIX_EPOCH + Duration::from_secs(updated.max(0) as u64);
                    if let Ok(file) = std::fs::File::options().write(true).open(&task.local) {
                        let _ = file.set_modified(time);
                    }
                }
                return Ok(Some(object.size));
            }
            Err(FetchError::Retryable(msg)) if attempt < 4 && !progress.cancel.load(Ordering::Relaxed) => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
                let _ = msg;
            }
            Err(FetchError::Retryable(msg) | FetchError::Fatal(msg)) => {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(msg);
            }
        }
    }
}

enum FetchError {
    Retryable(String),
    Fatal(String),
}

async fn fetch(
    client: &GcsClient,
    bucket: &str,
    object: &ObjectInfo,
    part: &Path,
    throttle: &Throttle,
    progress: &OpProgress,
) -> Result<u32, FetchError> {
    let cancelled = || progress.cancel.load(Ordering::Relaxed);
    let mut response = match client.download(bucket, &object.name, object.generation.as_deref()).await {
        Ok(r) => r,
        Err(GcsError::Retryable(m)) => return Err(FetchError::Retryable(m)),
        Err(e) => return Err(FetchError::Fatal(e.to_string())),
    };
    let mut file = tokio::fs::File::create(part).await.map_err(|e| FetchError::Fatal(format!("can't write {}: {e}", part.display())))?;
    let mut crc = 0u32;
    let mut written = 0u64;
    loop {
        if cancelled() {
            drop(file);
            let _ = tokio::fs::remove_file(part).await;
            return Err(FetchError::Fatal("stopped".into()));
        }
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => return Err(FetchError::Retryable(format!("network error: {e}"))),
        };
        throttle.take(chunk.len() as u64, cancelled).await;
        crc = crc32c::crc32c_append(crc, &chunk);
        written += chunk.len() as u64;
        file.write_all(&chunk).await.map_err(|e| FetchError::Fatal(format!("can't write {}: {e}", part.display())))?;
    }
    file.flush().await.map_err(|e| FetchError::Fatal(e.to_string()))?;
    file.sync_all().await.map_err(|e| FetchError::Fatal(e.to_string()))?;
    if written != object.size {
        return Err(FetchError::Retryable(format!("got {written} of {} bytes", object.size)));
    }
    Ok(crc)
}

// ----- upload files -----------------------------------------------------------------------

/// Upload individual local files into gs://bucket/prefix/ under their own names. Objects
/// that already exist are left as they are (counted as skipped).
pub async fn upload_files(
    client: &GcsClient,
    files: &[PathBuf],
    bucket: &str,
    prefix: &str,
    throttle: &Throttle,
    progress: &OpProgress,
) -> Result<OpResult> {
    let started = Instant::now();
    let prefix = folder_prefix(prefix);
    let report = Report::new(progress);
    progress.total.store(files.len() as u64, Ordering::Relaxed);
    stream::iter(files.iter())
        .for_each_concurrent(CONCURRENCY, |file| {
            let (report, prefix) = (&report, &prefix);
            async move {
                if progress.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let Some(name) = file.file_name().map(|n| n.to_string_lossy().to_string()) else {
                    report.failed(file.display().to_string(), "not a file".into());
                    return;
                };
                let object = format!("{prefix}{name}");
                match upload_one(client, file, bucket, &object, throttle, progress).await {
                    Ok(Some(bytes)) => report.done(bytes),
                    Ok(None) => report.skipped(),
                    Err(error) => report.failed(file.display().to_string(), error),
                }
            }
        })
        .await;
    Ok(report.finish(OpKind::Upload, started))
}

fn file_crc(path: &Path) -> std::io::Result<u32> {
    use std::io::Read;
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

async fn upload_one(
    client: &GcsClient,
    path: &Path,
    bucket: &str,
    name: &str,
    throttle: &Throttle,
    progress: &OpProgress,
) -> Result<Option<u64>, String> {
    let cancelled = || progress.cancel.load(Ordering::Relaxed);
    let meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("folders are uploaded as a sync job, not here".into());
    }
    let size = meta.len();
    if size <= SINGLE_REQUEST_LIMIT {
        let data = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
        let crc = crc32c::crc32c(&data);
        throttle.take(size, cancelled).await;
        return match retry(|| client.upload_multipart(bucket, name, data.clone(), crc, true)).await {
            Ok(_) => Ok(Some(size)),
            Err(GcsError::AlreadyExists) => Ok(None),
            Err(e) => Err(e.to_string()),
        };
    }

    let owned = path.to_path_buf();
    let crc = tokio::task::spawn_blocking(move || file_crc(&owned)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let session = match retry(|| client.start_resumable(bucket, name, size, crc, true)).await {
        Ok(uri) => uri,
        Err(GcsError::AlreadyExists) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    let mut offset = 0u64;
    let mut attempts = 0;
    while offset < size {
        if cancelled() {
            return Err("stopped".into());
        }
        let len = UPLOAD_CHUNK.min((size - offset) as usize);
        let mut data = vec![0u8; len];
        file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| e.to_string())?;
        file.read_exact(&mut data).await.map_err(|e| e.to_string())?;
        throttle.take(len as u64, cancelled).await;
        match client.put_chunk(&session, offset, data, size, crc).await {
            Ok(ResumableStatus::Complete(_)) => return Ok(Some(size)),
            Ok(ResumableStatus::Incomplete { offset: have }) => {
                offset = have;
                attempts = 0;
            }
            Err(GcsError::Retryable(msg)) if attempts < 5 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempts))).await;
                match client.query_resumable(&session, size, crc).await {
                    Ok(ResumableStatus::Incomplete { offset: have }) => offset = have,
                    Ok(ResumableStatus::Complete(_)) => return Ok(Some(size)),
                    Err(_) => {
                        let _ = msg;
                    }
                }
            }
            Err(GcsError::AlreadyExists) => return Ok(None),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(Some(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_stay_inside_the_destination() {
        let dest = Path::new("/data/downloads");
        assert_eq!(local_path(dest, "photos/a.jpg").unwrap(), dest.join("photos").join("a.jpg"));
        assert_eq!(local_path(dest, "photos//a.jpg").unwrap(), dest.join("photos").join("a.jpg"));
        assert!(local_path(dest, "photos/../../etc/passwd").is_err());
        assert!(local_path(dest, "./x").is_err());
        assert!(local_path(dest, "/").is_err());
        assert_eq!(safe_component("a\\b").unwrap(), "a_b");
        assert_eq!(safe_component("tab\there").unwrap(), "tab_here");
    }
}
