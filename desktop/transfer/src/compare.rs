//! Compare a local folder with its cloud copy: what is in both, only on this computer (will
//! upload on the next sync), only in the cloud, or different. Read-only.
//!
//! Sizes are compared for every file; where the upload manifest knows a file's checksum
//! (it was uploaded by the JetStream engine and hasn't changed since), the checksum is
//! compared too, so "same" is confirmed rather than assumed.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use regex::Regex;
use serde::Serialize;

use crate::gcs::{parse_crc32c_b64, GcsClient, ObjectInfo};
use crate::manifest::Manifest;
use crate::sync::{parse_destination, scan};

/// Files listed per group (the counts cover all of them).
const MAX_LISTED: usize = 5000;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Diff {
    /// Path relative to the folder, with "/" separators.
    pub path: String,
    pub local_size: Option<u64>,
    pub cloud_size: Option<u64>,
    /// Seconds since the epoch.
    pub local_modified: Option<i64>,
    pub cloud_updated: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Group {
    pub files: u64,
    pub bytes: u64,
    /// Largest first, up to a limit.
    pub list: Vec<Diff>,
    pub truncated: bool,
}

impl Group {
    fn add(&mut self, diff: Diff, bytes: u64) {
        self.files += 1;
        self.bytes += bytes;
        self.list.push(diff);
    }

    fn finish(&mut self) {
        let size = |d: &Diff| d.local_size.or(d.cloud_size).unwrap_or(0);
        self.list.sort_by_key(|d| std::cmp::Reverse(size(d)));
        self.truncated = self.list.len() > MAX_LISTED;
        self.list.truncate(MAX_LISTED);
    }
}

/// Counts per top-level subfolder.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FolderCounts {
    /// First path segment ("" for files directly in the folder).
    pub folder: String,
    pub same: u64,
    pub different: u64,
    pub local_only: u64,
    pub cloud_only: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CompareReport {
    pub local_root: String,
    pub destination: String,
    pub local_files: u64,
    pub local_bytes: u64,
    pub cloud_files: u64,
    pub cloud_bytes: u64,
    /// Skipped by the folder's filters (not expected in the cloud).
    pub excluded: u64,
    pub unreadable: u64,
    pub same: Group,
    /// Of `same`, how many were confirmed by checksum.
    pub verified: u64,
    pub different: Group,
    pub local_only: Group,
    pub cloud_only: Group,
    pub folders: Vec<FolderCounts>,
    pub elapsed_seconds: f64,
}

/// Turn the glob patterns synced folders use ("*.tmp", "Thumbs.db", "~*") into regexes,
/// with the same rules as the JetStream server, plus folder names skipped anywhere.
pub fn exclude_regexes(patterns: &[String], folders: &[String]) -> Result<Vec<Regex>> {
    let mut out = Vec::new();
    for pattern in patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let looks_like_regex = ["|", "(", ")", "^", "$", "\\", "{", "}", "[", "]", "+"].iter().any(|c| pattern.contains(c));
        let regex = if looks_like_regex {
            pattern.to_string()
        } else if let Some(ext) = pattern.strip_prefix("*.") {
            format!(r".*\.{}$", regex::escape(ext))
        } else if let Some(rest) = pattern.strip_prefix('*') {
            format!(".*{}$", regex::escape(rest))
        } else if let Some(start) = pattern.strip_suffix('*') {
            format!("^{}.*", regex::escape(start))
        } else if pattern.contains('*') {
            regex::escape(pattern).replace(r"\*", ".*")
        } else {
            // An exact file name, anywhere in the tree.
            format!("(^|/){}$", regex::escape(pattern))
        };
        out.push(Regex::new(&format!("(?i){regex}"))?);
    }
    for folder in folders.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
        out.push(Regex::new(&format!("(?i)(^|/){}/", regex::escape(folder)))?);
    }
    Ok(out)
}

fn top_folder(path: &str) -> &str {
    path.split_once('/').map(|(first, _)| first).unwrap_or("")
}

/// Compare `local_root` with gs://bucket/prefix. `manifest` adds checksum confirmation.
pub async fn compare(
    client: &GcsClient,
    local_root: &Path,
    destination: &str,
    excludes: &[Regex],
    recursive: bool,
    manifest: Option<&Manifest>,
) -> Result<CompareReport> {
    let started = Instant::now();
    let (bucket, prefix) = parse_destination(destination)?;
    let dest_key = format!("gs://{bucket}/{prefix}");

    let root = local_root.to_path_buf();
    let excludes_owned = excludes.to_vec();
    let local = tokio::task::spawn_blocking(move || scan(&root, &excludes_owned, recursive));

    let mut cloud: HashMap<String, ObjectInfo> = HashMap::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = client.list_raw(&bucket, &prefix, token.as_deref(), "name,size,crc32c,updated", false).await?;
        for raw in page {
            let Ok(info) = serde_json::from_value::<ObjectInfo>(raw) else { continue };
            let rel = info.name[prefix.len()..].to_string();
            // Folder placeholder objects aren't files; without recursion, only the top level counts.
            if rel.is_empty() || rel.ends_with('/') || (!recursive && rel.contains('/')) {
                continue;
            }
            cloud.insert(rel, info);
        }
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    let (files, excluded, unreadable) = local.await??;
    let known = match manifest {
        Some(m) => m.load(&dest_key).unwrap_or_default(),
        None => HashMap::new(),
    };

    let mut report = CompareReport {
        local_root: local_root.display().to_string(),
        destination: dest_key,
        excluded,
        unreadable,
        cloud_files: cloud.len() as u64,
        cloud_bytes: cloud.values().map(|o| o.size).sum(),
        ..Default::default()
    };
    let mut folders: BTreeMap<String, FolderCounts> = BTreeMap::new();
    let mut bump = |path: &str, f: fn(&mut FolderCounts)| {
        let key = top_folder(path).to_string();
        f(folders.entry(key.clone()).or_insert_with(|| FolderCounts { folder: key, ..Default::default() }));
    };

    for file in files {
        report.local_files += 1;
        report.local_bytes += file.size;
        let modified = Some(file.mtime_ns / 1_000_000_000);
        match cloud.remove(&file.rel) {
            None => {
                bump(&file.rel, |c| c.local_only += 1);
                report.local_only.add(Diff { path: file.rel, local_size: Some(file.size), local_modified: modified, ..Default::default() }, file.size);
            }
            Some(object) => {
                let mut same = object.size == file.size;
                let mut verified = false;
                if same {
                    // A checksum recorded for this exact file (same size and modified time).
                    let recorded = known.get(&file.rel).filter(|r| r.size == file.size && r.mtime_ns == file.mtime_ns).map(|r| r.crc32c);
                    if let (Some(local_crc), Some(cloud_crc)) = (recorded, object.crc32c.as_deref().and_then(parse_crc32c_b64)) {
                        same = local_crc == cloud_crc;
                        verified = same;
                    }
                }
                let diff = Diff {
                    path: file.rel.clone(),
                    local_size: Some(file.size),
                    cloud_size: Some(object.size),
                    local_modified: modified,
                    cloud_updated: object.updated,
                };
                if same {
                    bump(&file.rel, |c| c.same += 1);
                    report.verified += u64::from(verified);
                    report.same.add(diff, file.size);
                } else {
                    bump(&file.rel, |c| c.different += 1);
                    report.different.add(diff, file.size);
                }
            }
        }
    }
    for (rel, object) in cloud {
        bump(&rel, |c| c.cloud_only += 1);
        report.cloud_only.add(Diff { path: rel, cloud_size: Some(object.size), cloud_updated: object.updated, ..Default::default() }, object.size);
    }
    for group in [&mut report.same, &mut report.different, &mut report.local_only, &mut report.cloud_only] {
        group.finish();
    }
    report.folders = folders.into_values().collect();
    report.elapsed_seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_become_regexes_like_the_server() {
        let re = exclude_regexes(&["*.tmp".into(), "Thumbs.db".into(), "~*".into()], &["_archive".into()]).unwrap();
        let hit = |p: &str| re.iter().any(|r| r.is_match(p));
        assert!(hit("a/b/x.TMP"));
        assert!(hit("photos/Thumbs.db"));
        assert!(hit("~draft.docx"));
        assert!(hit("_archive/old.jpg") && hit("x/_archive/old.jpg"));
        assert!(!hit("photos/a.jpg") && !hit("x_archive/a.jpg") && !hit("notThumbs.db"));
    }
}
