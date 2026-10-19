//! Bucket audit: find junk objects (temporary, system and partial-upload files) under a prefix.
//! Read-only; moving findings to quarantine is done by the JetStream server.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use crate::browse::folder_prefix;
use crate::gcs::GcsClient;

/// Default junk patterns, matched case-insensitively against the full object name.
pub const DEFAULT_PATTERNS: &[&str] = &[
    r"\.gstmp$",
    r"(^|/)@eaDir/",
    r"(^|/)Thumbs\.db$",
    r"(^|/)\.DS_Store$",
    r"(^|/)desktop\.ini$",
    r"(^|/)~\$",
    r"\.(tmp|temp|crdownload|part)$",
];

#[derive(Default)]
pub struct AuditProgress {
    pub scanned: AtomicU64,
    pub found: AtomicU64,
    pub cancel: AtomicBool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub object_name: String,
    pub size: u64,
    pub updated: Option<String>,
    pub matched_pattern: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PatternTotal {
    pub pattern: String,
    pub objects: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub bucket: String,
    pub prefix: String,
    pub patterns: Vec<String>,
    pub scanned_objects: u64,
    pub scanned_bytes: u64,
    pub junk_objects: u64,
    pub junk_bytes: u64,
    pub by_pattern: Vec<PatternTotal>,
    /// Every finding up to `max_findings` (largest first after the scan).
    pub findings: Vec<Finding>,
    pub findings_truncated: bool,
    pub reached_scan_limit: bool,
    pub cancelled: bool,
    pub elapsed_seconds: f64,
}

pub fn compile(patterns: &[String]) -> Result<Vec<(String, Regex)>> {
    patterns
        .iter()
        .map(|p| {
            RegexBuilder::new(p)
                .case_insensitive(true)
                .build()
                .map(|re| (p.clone(), re))
                .with_context(|| format!("invalid pattern {p}"))
        })
        .collect()
}

pub async fn audit_bucket(
    client: &GcsClient,
    bucket: &str,
    prefix: &str,
    patterns: &[String],
    max_objects: u64,
    max_findings: usize,
    progress: &AuditProgress,
) -> Result<AuditReport> {
    let started = Instant::now();
    let compiled = compile(patterns)?;
    let prefix = folder_prefix(prefix);
    let mut report = AuditReport {
        bucket: bucket.to_string(),
        prefix: prefix.clone(),
        patterns: patterns.to_vec(),
        scanned_objects: 0,
        scanned_bytes: 0,
        junk_objects: 0,
        junk_bytes: 0,
        by_pattern: Vec::new(),
        findings: Vec::new(),
        findings_truncated: false,
        reached_scan_limit: false,
        cancelled: false,
        elapsed_seconds: 0.0,
    };
    let mut totals: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut token: Option<String> = None;

    'pages: loop {
        if progress.cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let page = client.list_flat(bucket, &prefix, token.as_deref()).await?;
        for object in page.objects {
            report.scanned_objects += 1;
            report.scanned_bytes += object.size;
            progress.scanned.fetch_add(1, Ordering::Relaxed);
            if let Some((pattern, _)) = compiled.iter().find(|(_, re)| re.is_match(&object.name)) {
                report.junk_objects += 1;
                report.junk_bytes += object.size;
                progress.found.fetch_add(1, Ordering::Relaxed);
                let entry = totals.entry(pattern.clone()).or_default();
                entry.0 += 1;
                entry.1 += object.size;
                if report.findings.len() < max_findings {
                    report.findings.push(Finding {
                        object_name: object.name,
                        size: object.size,
                        updated: object.updated,
                        matched_pattern: pattern.clone(),
                    });
                } else {
                    report.findings_truncated = true;
                }
            }
            if max_objects > 0 && report.scanned_objects >= max_objects {
                report.reached_scan_limit = page.next_page_token.is_some() || report.scanned_objects > max_objects;
                break 'pages;
            }
        }
        match page.next_page_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }

    report.by_pattern = totals
        .into_iter()
        .map(|(pattern, (objects, bytes))| PatternTotal { pattern, objects, bytes })
        .collect();
    report.by_pattern.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    report.findings.sort_by(|a, b| b.size.cmp(&a.size));
    report.elapsed_seconds = (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_patterns_match_common_junk_and_nothing_else() {
        let patterns: Vec<String> = DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();
        let compiled = compile(&patterns).unwrap();
        let is_junk = |name: &str| compiled.iter().any(|(_, re)| re.is_match(name));
        for junk in [
            "a/photo.jpg.gstmp",
            "nas/@eaDir/x.jpg",
            "a/THUMBS.DB",
            ".DS_Store",
            "d/desktop.ini",
            "docs/~$report.docx",
            "x/file.tmp",
            "dl/video.mp4.crdownload",
        ] {
            assert!(is_junk(junk), "{junk} should be junk");
        }
        for keep in ["a/photo.jpg", "notThumbs.dbx", "docs/report~.docx", "data/temperature.csv", "a/eaDir/x"] {
            assert!(!is_junk(keep), "{keep} should not be junk");
        }
    }

    #[test]
    fn invalid_patterns_are_reported() {
        assert!(compile(&["(unclosed".to_string()]).is_err());
    }
}
