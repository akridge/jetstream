//! "Fix file details": add what older uploads are missing (a proper content type, and the
//! original file's modified and created dates) to objects already in the cloud, by updating
//! their metadata in place. Nothing is re-uploaded.
//!
//! Plan first (read-only): the type comes from the name; dates come from the synced folder on
//! this computer, only for files that are the same as their cloud copy (same size and checksum,
//! the checksum taken from the upload manifest or by reading the file). Then apply: each object
//! is patched only if it hasn't changed since it was planned (ifMetagenerationMatch).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use regex::Regex;
use serde::Serialize;

use crate::fileops::{Failure, OpProgress, OpResult};
use crate::gcs::{guess_content_type, parse_crc32c_b64, GcsClient, GcsError, ObjectInfo, FILE_CREATED_KEY, FILE_MTIME_KEY};
use crate::manifest::Manifest;
use crate::sync::{crc32c_file, parse_destination, scan};

/// Examples listed in the plan (the counts cover everything).
const MAX_EXAMPLES: usize = 200;
const PATCHES_AT_ONCE: usize = 16;
const BINARY: &[&str] = &["application/octet-stream", "binary/octet-stream"];

/// What one object gets.
#[derive(Debug, Clone, Serialize)]
pub struct FixItem {
    pub name: String,
    #[serde(skip)]
    pub metageneration: Option<String>,
    /// New content type (None: leave it).
    pub content_type: Option<String>,
    /// Original modified / created time to record (None: leave it).
    pub modified: Option<i64>,
    pub created: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FixPlan {
    pub bucket: String,
    pub prefix: String,
    /// Objects looked at.
    pub objects: u64,
    /// Objects that get a content type / original dates.
    pub types: u64,
    pub dates: u64,
    /// New content types and how many files get each.
    pub by_type: BTreeMap<String, u64>,
    /// Cloud files with no matching file on this computer (no dates for those).
    pub not_local: u64,
    /// On both, but different content (no dates: they aren't the same file).
    pub different: u64,
    /// Files read to check they're the same (no checksum known for them).
    pub read_files: u64,
    pub examples: Vec<FixItem>,
    #[serde(skip)]
    pub items: Vec<FixItem>,
}

/// What a fix would change, given the object and (if it's the same file) the local file's dates.
fn needs(object: &ObjectInfo, local: Option<(i64, Option<i64>)>) -> Option<FixItem> {
    let current = object.content_type.as_deref().unwrap_or("");
    let guessed = guess_content_type(&object.name);
    let content_type = ((current.is_empty() || BINARY.contains(&current)) && !BINARY.contains(&guessed.as_str())).then_some(guessed);
    let (mut modified, mut created) = (None, None);
    if let Some((mtime, ctime)) = local {
        if object.file_mtime() != Some(mtime) {
            modified = Some(mtime);
        }
        if let Some(ctime) = ctime {
            if object.file_created().is_none() {
                created = Some(ctime);
            }
        }
    }
    (content_type.is_some() || modified.is_some() || created.is_some()).then(|| FixItem {
        name: object.name.clone(),
        metageneration: object.metageneration.clone(),
        content_type,
        modified,
        created,
    })
}

/// Work out what objects under gs://bucket/prefix need. With `local`, the folder synced to it
/// provides the original dates. `progress.done` counts objects looked at.
pub async fn plan_fix(
    client: &GcsClient,
    destination: &str,
    local: Option<(&Path, &[Regex], bool)>,
    manifest: Option<&Manifest>,
    progress: &OpProgress,
) -> Result<FixPlan> {
    let (bucket, prefix) = parse_destination(destination)?;
    let mut objects: Vec<ObjectInfo> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = client
            .list_raw(&bucket, &prefix, token.as_deref(), "name,size,crc32c,contentType,metadata,metageneration", false)
            .await?;
        for raw in page {
            let Ok(info) = serde_json::from_value::<ObjectInfo>(raw) else { continue };
            let rel = &info.name[prefix.len()..];
            if rel.is_empty() || rel.ends_with('/') || rel.starts_with(crate::receipt::JETSTREAM_PREFIX) {
                continue;
            }
            objects.push(info);
        }
        progress.total.store(objects.len() as u64, Ordering::Relaxed);
        if progress.cancel.load(Ordering::Relaxed) {
            anyhow::bail!("stopped");
        }
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }

    // The local folder: which cloud files are the same file, so their dates can be recorded.
    let mut same: HashMap<String, (i64, Option<i64>)> = HashMap::new();
    let mut plan = FixPlan { bucket: bucket.clone(), prefix: prefix.clone(), objects: objects.len() as u64, ..Default::default() };
    if let Some((root, excludes, recursive)) = local {
        let root_owned = root.to_path_buf();
        let excludes_owned = excludes.to_vec();
        let scanned = tokio::task::spawn_blocking(move || scan(&root_owned, &excludes_owned, recursive)).await??;
        let by_rel: HashMap<&str, _> = scanned.files.iter().map(|f| (f.rel.as_str(), f)).collect();
        let dest_key = format!("gs://{bucket}/{prefix}");
        let known = manifest.map(|m| m.load_tree(&dest_key).unwrap_or_default()).unwrap_or_default();
        let mut to_read = Vec::new();
        for object in &objects {
            let rel = &object.name[prefix.len()..];
            let Some(file) = by_rel.get(rel) else {
                plan.not_local += 1;
                continue;
            };
            let cloud_crc = object.crc32c.as_deref().and_then(parse_crc32c_b64);
            if file.size != object.size || cloud_crc.is_none() {
                plan.different += 1;
                continue;
            }
            let dates = (file.mtime_ns.div_euclid(1_000_000_000), file.created_secs);
            match known.get(rel).filter(|r| r.size == file.size && r.mtime_ns == file.mtime_ns) {
                Some(record) if Some(record.crc32c) == cloud_crc => {
                    same.insert(rel.to_string(), dates);
                }
                Some(_) => plan.different += 1,
                // No checksum on record: read the file to be sure, but only if a date is missing.
                None if needs(object, Some(dates)).is_some_and(|n| n.modified.is_some() || n.created.is_some()) => {
                    to_read.push((rel.to_string(), file.abs.clone(), cloud_crc, dates));
                }
                None => {}
            }
        }
        plan.read_files = to_read.len() as u64;
        let read: Vec<_> = stream::iter(to_read)
            .map(|(rel, path, cloud_crc, dates)| async move {
                let crc = tokio::task::spawn_blocking(move || crc32c_file(&path, None, None)).await.ok().and_then(|r| r.ok());
                (rel, crc == cloud_crc, dates)
            })
            .buffer_unordered(4)
            .collect()
            .await;
        for (rel, matches, dates) in read {
            if matches {
                same.insert(rel, dates);
            } else {
                plan.different += 1;
            }
        }
    }

    for object in &objects {
        let rel = &object.name[prefix.len()..];
        if let Some(item) = needs(object, same.get(rel).copied()) {
            if let Some(t) = &item.content_type {
                plan.types += 1;
                *plan.by_type.entry(t.clone()).or_default() += 1;
            }
            if item.modified.is_some() || item.created.is_some() {
                plan.dates += 1;
            }
            if plan.examples.len() < MAX_EXAMPLES {
                plan.examples.push(item.clone());
            }
            plan.items.push(item);
        }
    }
    progress.done.store(objects.len() as u64, Ordering::Relaxed);
    Ok(plan)
}

/// Patch the planned objects (16 at a time). Objects changed since planning are skipped.
pub async fn apply_fix(client: &GcsClient, plan: &FixPlan, progress: &OpProgress) -> OpResult {
    let started = Instant::now();
    progress.total.store(plan.items.len() as u64, Ordering::Relaxed);
    let failures = std::sync::Mutex::new(Vec::new());
    stream::iter(&plan.items)
        .for_each_concurrent(PATCHES_AT_ONCE, |item| {
            let failures = &failures;
            async move {
                if progress.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let mut body = serde_json::json!({});
                if let Some(t) = &item.content_type {
                    body["contentType"] = t.clone().into();
                }
                let mut custom = serde_json::Map::new();
                if let Some(m) = item.modified {
                    custom.insert(FILE_MTIME_KEY.into(), m.to_string().into());
                }
                if let Some(c) = item.created {
                    custom.insert(FILE_CREATED_KEY.into(), c.to_string().into());
                }
                if !custom.is_empty() {
                    body["metadata"] = serde_json::Value::Object(custom);
                }
                let mut attempt = 0;
                loop {
                    match client.patch_object(&plan.bucket, &item.name, item.metageneration.as_deref(), &body).await {
                        Ok(_) => {
                            progress.done.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                        // Changed since it was planned (or deleted): leave it.
                        Err(GcsError::AlreadyExists) => {
                            progress.skipped.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                        Err(GcsError::Retryable(_) | GcsError::Network(_)) if attempt < 4 => {
                            attempt += 1;
                            tokio::time::sleep(std::time::Duration::from_millis(500 * 2u64.pow(attempt))).await;
                        }
                        Err(err) => {
                            progress.failed.fetch_add(1, Ordering::Relaxed);
                            failures.lock().unwrap().push(Failure { path: format!("gs://{}/{}", plan.bucket, item.name), error: err.to_string() });
                            return;
                        }
                    }
                }
            }
        })
        .await;
    let failures = failures.into_inner().unwrap();
    OpResult {
        kind: None,
        total: plan.items.len() as u64,
        done: progress.done.load(Ordering::Relaxed),
        skipped: progress.skipped.load(Ordering::Relaxed),
        failed: failures.len() as u64,
        failures: failures.into_iter().take(50).collect(),
        cancelled: progress.cancel.load(Ordering::Relaxed),
        undoable: false,
        elapsed_seconds: started.elapsed().as_secs_f64(),
        undo: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(name: &str, content_type: Option<&str>, meta: &[(&str, &str)]) -> ObjectInfo {
        ObjectInfo {
            name: name.into(),
            content_type: content_type.map(str::to_string),
            metadata: (!meta.is_empty()).then(|| meta.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn what_needs_fixing() {
        // Binary-typed JPEG: gets its type.
        let item = needs(&object("a/IMG.JPG", Some("application/octet-stream"), &[]), None).unwrap();
        assert_eq!((item.content_type.as_deref(), item.modified), (Some("image/jpeg"), None));
        // A proper type stays; an unknown extension stays binary: nothing to do.
        assert!(needs(&object("a.jpg", Some("image/jpeg"), &[]), None).is_none());
        assert!(needs(&object("a.zzz", Some("application/octet-stream"), &[]), None).is_none());
        // With the same local file: dates are added where missing or different.
        let item = needs(&object("a.jpg", Some("image/jpeg"), &[]), Some((1_600_000_000, Some(1_500_000_000)))).unwrap();
        assert_eq!((item.content_type, item.modified, item.created), (None, Some(1_600_000_000), Some(1_500_000_000)));
        let done = object("a.jpg", Some("image/jpeg"), &[(FILE_MTIME_KEY, "1600000000"), (FILE_CREATED_KEY, "1500000000")]);
        assert!(needs(&done, Some((1_600_000_000, Some(1_500_000_000)))).is_none(), "already right");
        // A recorded created time is kept.
        let has_created = object("a.jpg", Some("image/jpeg"), &[(FILE_MTIME_KEY, "1600000000"), (FILE_CREATED_KEY, "1400000000")]);
        assert!(needs(&has_created, Some((1_600_000_000, Some(1_500_000_000)))).is_none());
    }
}
