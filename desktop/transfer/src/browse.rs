//! Browsing Cloud Storage like a file system: one folder level at a time, and folder totals.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use crate::gcs::GcsClient;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Name relative to the listed folder ("photos/" for a folder, "a.jpg" for a file).
    pub name: String,
    /// Full object name or prefix.
    pub path: String,
    pub is_folder: bool,
    pub size: Option<u64>,
    pub updated: Option<String>,
    pub storage_class: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub bucket: String,
    pub prefix: String,
    pub entries: Vec<Entry>,
    pub next_page_token: Option<String>,
}

/// "a/b" -> "a/b/", "" stays "".
pub fn folder_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim().trim_start_matches('/');
    if trimmed.is_empty() || trimmed.ends_with('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    }
}

/// Folders first, then files, directly under `prefix`.
pub async fn list_folder(
    client: &GcsClient,
    bucket: &str,
    prefix: &str,
    page_token: Option<&str>,
    max_results: u32,
    include_folders: bool,
) -> Result<Listing> {
    let prefix = folder_prefix(prefix);
    let page = client.list_dir(bucket, &prefix, page_token, max_results, include_folders).await?;
    let mut entries: Vec<Entry> = page
        .prefixes
        .into_iter()
        .map(|p| Entry {
            name: p.strip_prefix(&prefix).unwrap_or(&p).to_string(),
            path: p,
            is_folder: true,
            size: None,
            updated: None,
            storage_class: None,
            content_type: None,
        })
        .collect();
    entries.extend(
        page.objects
            .into_iter()
            // The zero-byte "folder marker" some tools create for the folder itself.
            .filter(|o| o.name != prefix)
            .map(|o| Entry {
                name: o.name.strip_prefix(&prefix).unwrap_or(&o.name).to_string(),
                path: o.name,
                is_folder: false,
                size: Some(o.size),
                updated: o.updated,
                storage_class: o.storage_class,
                content_type: o.content_type,
            }),
    );
    Ok(Listing { bucket: bucket.to_string(), prefix, entries, next_page_token: page.next_page_token })
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FolderTotals {
    pub objects: u64,
    pub bytes: u64,
    /// Bytes per storage class (STANDARD, NEARLINE, ...).
    pub by_storage_class: BTreeMap<String, u64>,
    pub newest: Option<String>,
    /// Stopped after `limit` objects.
    pub truncated: bool,
}

/// Count every object under a prefix (recursively). Stops after the page (of up to 1000
/// objects) that reaches `limit`, and reports `truncated` if more remain.
pub async fn folder_totals(client: &GcsClient, bucket: &str, prefix: &str, limit: u64) -> Result<FolderTotals> {
    let prefix = folder_prefix(prefix);
    let mut totals = FolderTotals::default();
    let mut token: Option<String> = None;
    loop {
        let page = client.list_flat(bucket, &prefix, token.as_deref()).await?;
        for object in page.objects {
            totals.objects += 1;
            totals.bytes += object.size;
            *totals.by_storage_class.entry(object.storage_class.unwrap_or_else(|| "STANDARD".into())).or_default() += object.size;
            if object.updated > totals.newest {
                totals.newest = object.updated;
            }
        }
        if totals.objects >= limit {
            totals.truncated = page.next_page_token.is_some();
            break;
        }
        match page.next_page_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    Ok(totals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_prefix_normalizes() {
        assert_eq!(folder_prefix(""), "");
        assert_eq!(folder_prefix("/a/b"), "a/b/");
        assert_eq!(folder_prefix("a/b/"), "a/b/");
    }
}
