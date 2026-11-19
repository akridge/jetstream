//! Speed check: what limits uploads from this computer to this bucket.
//!
//! - **Folder:** how fast the folder lists (files per second, which network shares feel
//!   most), how fast big files read (MB/s), and how many small files open and read per second.
//! - **Network:** MB/s over one connection, and over 8 at once (what parallel parts and
//!   several files at once get), each for up to ~12 seconds.
//! - **Requests:** milliseconds per request, the per-file cost of folders of small files.
//!
//! Nothing is stored in the bucket: the test data goes to resumable upload sessions that are
//! never completed (each is declared one piece larger than what is sent) and are then
//! cancelled, so no object is ever created. It only needs permission to create objects.
//! The app's speed limit doesn't apply, so the numbers show what the connection can do.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use futures::future::join_all;
use serde::Serialize;

use crate::fileops::OpProgress;
use crate::gcs::{GcsClient, GcsError, ResumableStatus, UploadExtras};
use crate::sync::{parse_destination, scan};

const PIECE: usize = 4 * 1024 * 1024;
const SINGLE_MAX: u64 = 96 * 1024 * 1024;
const STREAMS: usize = 8;
const STREAM_MAX: u64 = 32 * 1024 * 1024;
const NET_SECONDS: u64 = 12;
const REQUESTS: usize = 15;
const DISK_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const DISK_SECONDS: u64 = 12;
const SMALL_FILE: u64 = 1024 * 1024;
const SMALL_MAX: usize = 1000;
const SMALL_SECONDS: u64 = 8;
/// Steps, for progress (`done` of `total`).
pub const STEPS: u64 = 5;

#[derive(Debug, Clone, Default, Serialize)]
pub struct FolderSpeed {
    pub path: String,
    /// Files found, and how fast the folder listed.
    pub files: u64,
    pub list_seconds: f64,
    pub list_files_per_sec: f64,
    /// Reading big files (largest first).
    pub read_bytes: u64,
    pub read_mb_per_sec: Option<f64>,
    /// Opening and reading small files (under 1 MiB).
    pub small_files: u64,
    pub small_files_per_sec: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SpeedReport {
    pub destination: String,
    pub folder: Option<FolderSpeed>,
    /// MB/s (1 MB = 1,000,000 bytes) over one connection and over `streams` at once.
    pub single_mb_per_sec: f64,
    pub parallel_mb_per_sec: f64,
    pub streams: usize,
    /// Median time for one request to Cloud Storage.
    pub request_ms: f64,
    /// What limits uploads: "folder", "network", "requests" or "none".
    pub limit: String,
    /// Plain-language findings, most important first.
    pub findings: Vec<String>,
    pub elapsed_seconds: f64,
    pub cancelled: bool,
}

fn mb(bytes: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        return 0.0;
    }
    (bytes as f64 / 1e6 / seconds * 10.0).round() / 10.0
}

fn stopped(progress: &OpProgress) -> bool {
    progress.cancel.load(Ordering::Relaxed)
}

/// Read the folder: list it, read big files, then small ones (in a blocking thread).
fn folder_speed(root: &Path, progress: &OpProgress) -> Result<FolderSpeed> {
    let started = Instant::now();
    let scanned = scan(root, &[], true)?;
    let list_seconds = started.elapsed().as_secs_f64();
    let mut speed = FolderSpeed {
        path: root.display().to_string(),
        files: scanned.files.len() as u64,
        list_seconds: (list_seconds * 1000.0).round() / 1000.0,
        list_files_per_sec: if list_seconds > 0.0 { (scanned.files.len() as f64 / list_seconds).round() } else { 0.0 },
        ..Default::default()
    };
    progress.done.fetch_add(1, Ordering::Relaxed);

    let mut big: Vec<_> = scanned.files.iter().filter(|f| f.size > SMALL_FILE).collect();
    big.sort_by_key(|f| std::cmp::Reverse(f.size));
    let mut buf = vec![0u8; PIECE];
    let started = Instant::now();
    'files: for file in big {
        let Ok(mut handle) = std::fs::File::open(&file.abs) else { continue };
        loop {
            use std::io::Read;
            match handle.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    speed.read_bytes += n as u64;
                    progress.bytes.fetch_add(n as u64, Ordering::Relaxed);
                }
            }
            if speed.read_bytes >= DISK_MAX_BYTES || started.elapsed().as_secs() >= DISK_SECONDS || stopped(progress) {
                break 'files;
            }
        }
    }
    if speed.read_bytes > 0 {
        speed.read_mb_per_sec = Some(mb(speed.read_bytes, started.elapsed().as_secs_f64()));
    }

    let small: Vec<_> = scanned.files.iter().filter(|f| f.size <= SMALL_FILE).take(SMALL_MAX).collect();
    let started = Instant::now();
    for file in small {
        if std::fs::read(&file.abs).is_ok() {
            speed.small_files += 1;
        }
        if started.elapsed().as_secs() >= SMALL_SECONDS || stopped(progress) {
            break;
        }
    }
    if speed.small_files >= 20 {
        speed.small_files_per_sec = Some((speed.small_files as f64 / started.elapsed().as_secs_f64().max(0.001)).round());
    }
    progress.done.fetch_add(1, Ordering::Relaxed);
    Ok(speed)
}

/// Send up to `max` bytes (or for `seconds`) to a resumable session that never completes,
/// then cancel it. Returns the bytes the server took.
#[allow(clippy::too_many_arguments)]
async fn stream(client: &GcsClient, bucket: &str, name: &str, max: u64, seconds: u64, data: &[u8], progress: &OpProgress, extras: &UploadExtras) -> Result<u64, GcsError> {
    // Declared one piece larger than will ever be sent, so the object is never created.
    let declared = max + PIECE as u64;
    let session = client.start_resumable_extras(bucket, name, declared, None, true, &UploadExtras { content_type: Some("application/octet-stream".into()), ..Default::default() }).await?;
    let started = Instant::now();
    let mut offset = 0u64;
    let outcome = loop {
        if offset >= max || started.elapsed().as_secs() >= seconds || stopped(progress) {
            break Ok(offset);
        }
        match client.put_chunk_opt(&session, offset, data.to_vec(), declared, None, extras).await {
            Ok(ResumableStatus::Incomplete { offset: kept }) => {
                progress.bytes.fetch_add(kept.saturating_sub(offset), Ordering::Relaxed);
                offset = kept;
            }
            Ok(ResumableStatus::Complete(_)) => break Ok(offset), // can't happen (declared larger)
            Err(_) if stopped(progress) => break Ok(offset),
            Err(err) => break Err(err),
        }
    };
    client.cancel_resumable(&session).await;
    outcome
}

/// Median time to start (and then cancel) a resumable upload: one request each.
async fn request_ms(client: &GcsClient, bucket: &str, name: &str, progress: &OpProgress) -> Result<f64, GcsError> {
    let mut times = Vec::new();
    let mut sessions = Vec::new();
    for _ in 0..REQUESTS {
        if stopped(progress) {
            break;
        }
        let started = Instant::now();
        sessions.push(client.start_resumable_extras(bucket, name, 1, None, true, &UploadExtras::default()).await?);
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    join_all(sessions.iter().map(|s| client.cancel_resumable(s))).await;
    // The first request also sets up the connection; leave it out when there are others.
    if times.len() > 1 {
        times.remove(0);
    }
    times.sort_by(|a, b| a.total_cmp(b));
    Ok(times.get(times.len() / 2).map(|t| (t * 10.0).round() / 10.0).unwrap_or(0.0))
}

/// Check gs://bucket/prefix (and, with `local`, the folder uploaded to it).
pub async fn speed_check(client: &GcsClient, destination: &str, local: Option<&Path>, progress: &OpProgress) -> Result<SpeedReport> {
    let started = Instant::now();
    let (bucket, prefix) = parse_destination(destination)?;
    progress.total.store(STEPS, Ordering::Relaxed);
    let mut report = SpeedReport { destination: destination.to_string(), streams: STREAMS, ..Default::default() };

    if let Some(root) = local {
        let root = root.to_path_buf();
        // The progress counters are shared with the blocking thread through an Arc'd copy.
        let shared = Arc::new(OpProgress::default());
        let (s, root2) = (shared.clone(), root.clone());
        let mut task = tokio::task::spawn_blocking(move || folder_speed(&root2, &s));
        let folder = loop {
            tokio::select! {
                result = &mut task => break result??,
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    if stopped(progress) {
                        shared.cancel.store(true, Ordering::Relaxed);
                    }
                    progress.bytes.store(shared.bytes.load(Ordering::Relaxed), Ordering::Relaxed);
                    progress.done.store(shared.done.load(Ordering::Relaxed), Ordering::Relaxed);
                }
            }
        };
        progress.done.store(2, Ordering::Relaxed);
        report.folder = Some(folder);
    } else {
        progress.done.store(2, Ordering::Relaxed);
    }
    if stopped(progress) {
        report.cancelled = true;
        return Ok(report);
    }

    let id = format!("{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff);
    let name = |n: usize| format!("{prefix}{}speedcheck-{id}-{n}", crate::receipt::JETSTREAM_PREFIX);
    let data: Vec<u8> = (0..PIECE).map(|_| rand::random::<u8>()).collect();
    let cancel = Arc::new(AtomicBool::new(false));
    let extras = UploadExtras { cancel: Some(cancel.clone()), ..Default::default() };

    // Requests first: they also warm up the connection and sign-in.
    report.request_ms = request_ms(client, &bucket, &name(0), progress).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    progress.done.fetch_add(1, Ordering::Relaxed);

    // Stopping also interrupts a piece already on its way.
    let watch = || async {
        while !stopped(progress) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        cancel.store(true, Ordering::Relaxed);
        std::future::pending::<()>().await;
    };
    let single = async {
        let t = Instant::now();
        let sent = stream(client, &bucket, &name(1), SINGLE_MAX, NET_SECONDS, &data, progress, &extras).await?;
        progress.done.fetch_add(1, Ordering::Relaxed);
        Ok::<_, GcsError>(mb(sent, t.elapsed().as_secs_f64()))
    };
    let single = tokio::select! { r = single => r, _ = watch() => Ok(0.0) };
    report.single_mb_per_sec = single.map_err(|e| anyhow::anyhow!("{e}"))?;
    if stopped(progress) {
        report.cancelled = true;
        return Ok(report);
    }

    let names: Vec<String> = (0..STREAMS).map(|n| name(2 + n)).collect();
    let t = Instant::now();
    let parallel = join_all(names.iter().map(|n| stream(client, &bucket, n, STREAM_MAX, NET_SECONDS, &data, progress, &extras)));
    let parallel = tokio::select! { r = parallel => r, _ = watch() => Vec::new() };
    let sent: u64 = parallel.iter().filter_map(|r| r.as_ref().ok()).sum();
    if sent == 0 {
        if let Some(Err(err)) = parallel.into_iter().find(|r| r.is_err()) {
            bail!("{err}");
        }
    }
    report.parallel_mb_per_sec = mb(sent, t.elapsed().as_secs_f64());
    progress.done.fetch_add(1, Ordering::Relaxed);

    report.cancelled = stopped(progress);
    (report.limit, report.findings) = verdict(&report);
    report.elapsed_seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    Ok(report)
}

/// What limits uploads, and what to say about it.
pub fn verdict(r: &SpeedReport) -> (String, Vec<String>) {
    let mut findings = Vec::new();
    let mut limit = "none";
    let (single, parallel) = (r.single_mb_per_sec, r.parallel_mb_per_sec);
    let network = single.max(parallel);
    let folder = r.folder.as_ref();

    if let Some(read) = folder.and_then(|f| f.read_mb_per_sec) {
        if read < network * 0.8 {
            limit = "folder";
            findings.push(format!(
                "Reading the folder ({read:.0} MB/s) is slower than the network ({network:.0} MB/s), so the disk or share is the limit, not the upload."
            ));
        }
    }
    if parallel >= single * 1.5 {
        findings.push(format!(
            "One connection gets {single:.1} MB/s and {} at once get {parallel:.1} MB/s, so sending in parallel helps. JetStream does that (parallel parts for large files, several files at once); keep it on in Settings.",
            r.streams
        ));
        if limit == "none" {
            limit = "network";
        }
    } else if network > 0.0 {
        findings.push(format!("One connection already gets about all the network gives ({network:.1} MB/s); more connections won't make uploads faster."));
        if limit == "none" {
            limit = "network";
        }
    }
    if r.request_ms >= 150.0 {
        if limit == "none" || limit == "network" && parallel < single * 1.5 {
            limit = "requests";
        }
        findings.push(format!(
            "Each request to Cloud Storage takes about {:.0} ms, so folders of many small files go slowly (each file costs at least that). Zipping folders of tiny files before uploading helps most.",
            r.request_ms
        ));
    }
    if let Some(f) = folder {
        if let Some(rate) = f.small_files_per_sec.filter(|&rate| rate < 100.0) {
            findings.push(format!("Opening small files is slow here ({rate:.0} files per second), as on many network shares; this adds up for folders of many small files."));
        }
        if f.files >= 1000 && f.list_files_per_sec < 2000.0 {
            findings.push(format!(
                "Listing the folder took {:.1} s ({:.0} files per second). JetStream remembers what it uploaded, so later syncs only need this listing, not re-reading files.",
                f.list_seconds, f.list_files_per_sec
            ));
        }
    }
    if network > 0.0 {
        let gb_hour = network * 3600.0 / 1000.0;
        findings.push(format!("At {network:.1} MB/s, 1 TB takes about {:.1} hours.", 1000.0 / gb_hour));
    }
    (limit.to_string(), findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(single: f64, parallel: f64, request_ms: f64, read: Option<f64>) -> SpeedReport {
        SpeedReport {
            single_mb_per_sec: single,
            parallel_mb_per_sec: parallel,
            streams: 8,
            request_ms,
            folder: read.map(|r| FolderSpeed { read_mb_per_sec: Some(r), files: 10, ..Default::default() }),
            ..Default::default()
        }
    }

    #[test]
    fn verdicts() {
        let (limit, findings) = verdict(&report(12.0, 70.0, 40.0, Some(400.0)));
        assert_eq!(limit, "network");
        assert!(findings[0].contains("sending in parallel helps"), "{findings:?}");

        let (limit, findings) = verdict(&report(50.0, 52.0, 40.0, Some(30.0)));
        assert_eq!(limit, "folder");
        assert!(findings[0].contains("Reading the folder (30 MB/s)"));
        assert!(findings.iter().any(|f| f.contains("won't make uploads faster")));

        let (limit, findings) = verdict(&report(50.0, 55.0, 220.0, None));
        assert_eq!(limit, "requests");
        assert!(findings.iter().any(|f| f.contains("about 220 ms")));
        assert!(findings.last().unwrap().contains("1 TB takes about 5.1 hours"), "{findings:?}");

        // Slow requests, but parallel uploads are the bigger win: still "network".
        assert_eq!(verdict(&report(5.0, 40.0, 220.0, None)).0, "network");
    }

    #[test]
    fn megabytes_per_second() {
        assert_eq!(mb(10_000_000, 2.0), 5.0);
        assert_eq!(mb(10, 0.0), 0.0);
    }
}
