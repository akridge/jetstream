//! Upload receipts: a file-by-file record that a folder is in the cloud intact.
//!
//! Every local file is matched to its cloud object and their crc32c checksums compared. The
//! local checksum comes from the upload manifest when JetStream read that exact file before
//! (same size and modified time); otherwise the file is read now, and the manifest learns it,
//! so the next receipt reads only what changed. Files the sync only recognised by size and
//! date are read here before being called verified.
//!
//! The receipt is written as CSV (people, spreadsheets) and JSON (tools), with who made it,
//! when, and whether the folder is complete.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use futures::stream::{self, StreamExt};
use regex::Regex;
use serde::Serialize;

use crate::fileops::OpProgress;
use crate::gcs::{crc32c_b64, parse_crc32c_b64, GcsClient, ObjectInfo};
use crate::manifest::{FileRecord, Manifest};
use crate::sync::{crc32c_file, list_remote, parse_destination, scan};

/// Objects JetStream keeps next to the data (e.g. cloud copies of receipts) aren't data.
pub const JETSTREAM_PREFIX: &str = "_jetstream/";
const HASH_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// In the cloud with the same size and checksum.
    Verified,
    /// In the cloud, but the content differs.
    Mismatch,
    /// On this computer, not in the cloud.
    MissingInCloud,
    /// In the cloud only (not counted against completeness).
    CloudOnly,
    /// Couldn't be read (locked, permissions, online-only placeholder).
    Unreadable,
}

impl FileStatus {
    fn label(self) -> &'static str {
        match self {
            FileStatus::Verified => "verified",
            FileStatus::Mismatch => "mismatch",
            FileStatus::MissingInCloud => "missing_in_cloud",
            FileStatus::CloudOnly => "cloud_only",
            FileStatus::Unreadable => "unreadable",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceiptFile {
    pub path: String,
    pub status: FileStatus,
    pub size: u64,
    /// The cloud object's crc32c (base64, as Google reports it).
    pub crc32c: Option<String>,
    /// The cloud object's MD5 (base64), when Google has one (not for composite objects).
    pub md5: Option<String>,
    pub generation: Option<String>,
    pub cloud_updated: Option<String>,
    /// Why a file isn't verified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    pub local_files: u64,
    pub local_bytes: u64,
    pub verified: u64,
    pub verified_bytes: u64,
    pub mismatch: u64,
    pub missing_in_cloud: u64,
    pub unreadable: u64,
    pub online_only: u64,
    pub cloud_only: u64,
    /// Files read to compute checksums for this receipt (the rest came from the manifest).
    pub files_read: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Receipt {
    pub generator: String,
    pub account: String,
    pub local_root: String,
    pub destination: String,
    pub started_at: String,
    pub finished_at: String,
    /// "complete" when every local file is verified; otherwise "incomplete".
    pub result: &'static str,
    pub totals: Totals,
    pub elapsed_seconds: f64,
    pub files: Vec<ReceiptFile>,
}

/// RFC 3339 UTC time for seconds since the epoch.
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Check `local_root` against `destination` and build a receipt. `progress.total` is the number
/// of local files and `done` counts those checked.
#[allow(clippy::too_many_arguments)]
pub async fn make_receipt(
    client: &GcsClient,
    local_root: &Path,
    destination: &str,
    excludes: &[Regex],
    recursive: bool,
    manifest: Option<&Manifest>,
    account: &str,
    progress: &OpProgress,
) -> Result<Receipt> {
    let started = Instant::now();
    let started_at = rfc3339(now_secs());
    let (bucket, prefix) = parse_destination(destination)?;
    let dest = format!("gs://{bucket}/{prefix}");

    let root = local_root.to_path_buf();
    let excludes_owned = excludes.to_vec();
    let scanned = tokio::task::spawn_blocking(move || scan(&root, &excludes_owned, recursive)).await??;
    progress.total.store(scanned.files.len() as u64, Ordering::Relaxed);

    let listed = std::sync::atomic::AtomicU64::new(0);
    let mut cloud: HashMap<String, ObjectInfo> = list_remote(client, &bucket, &prefix, None, &listed)
        .await?
        .into_iter()
        .filter(|(rel, _)| !rel.ends_with('/') && !rel.starts_with(JETSTREAM_PREFIX) && (recursive || !rel.contains('/')))
        .collect();
    let (records, by_time) = match manifest {
        Some(m) => (m.load_tree(&dest).unwrap_or_default(), m.by_time_paths(&dest).unwrap_or_default()),
        None => Default::default(),
    };

    let mut totals = Totals {
        local_files: scanned.files.len() as u64,
        local_bytes: scanned.files.iter().map(|f| f.size).sum(),
        online_only: scanned.online_only.len() as u64,
        unreadable: scanned.unreadable,
        ..Default::default()
    };
    let mut rows = Vec::new();
    for rel in &scanned.online_only {
        let object = cloud.remove(rel);
        rows.push(ReceiptFile {
            path: rel.clone(),
            status: FileStatus::Unreadable,
            size: object.as_ref().map(|o| o.size).unwrap_or(0),
            crc32c: object.as_ref().and_then(|o| o.crc32c.clone()),
            md5: object.as_ref().and_then(|o| o.md5_hash.clone()),
            generation: object.as_ref().and_then(|o| o.generation.clone()),
            cloud_updated: object.and_then(|o| o.updated),
            note: Some("online-only file (OneDrive): not on this computer, so it couldn't be checked".into()),
        });
    }

    // Pair local files with their objects; work out which need reading.
    let mut pending = Vec::new();
    for file in scanned.files {
        let object = cloud.remove(&file.rel);
        let known = records
            .get(&file.rel)
            .filter(|r| r.size == file.size && r.mtime_ns == file.mtime_ns && !by_time.contains(&file.rel))
            .map(|r| r.crc32c);
        pending.push((file, object, known));
    }

    let checked: Vec<_> = stream::iter(pending)
        .map(|(file, object, known)| async move {
            let local_crc = match (known, &object) {
                (_, None) => None,
                (Some(crc), _) => Some(Ok(crc)),
                (None, Some(_)) => {
                    let abs: PathBuf = file.abs.clone();
                    Some(match tokio::task::spawn_blocking(move || crc32c_file(&abs, None, None)).await {
                        Ok(result) => result.map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    })
                }
            };
            progress.done.fetch_add(1, Ordering::Relaxed);
            (file, object, known.is_none(), local_crc)
        })
        .buffer_unordered(HASH_CONCURRENCY)
        .collect()
        .await;

    for (file, object, read, local_crc) in checked {
        let row = |status, object: Option<&ObjectInfo>, note: Option<String>| ReceiptFile {
            path: file.rel.clone(),
            status,
            size: file.size,
            crc32c: object.and_then(|o| o.crc32c.clone()),
            md5: object.and_then(|o| o.md5_hash.clone()),
            generation: object.and_then(|o| o.generation.clone()),
            cloud_updated: object.and_then(|o| o.updated.clone()),
            note,
        };
        let Some(object) = object else {
            totals.missing_in_cloud += 1;
            rows.push(row(FileStatus::MissingInCloud, None, Some("not in the cloud".into())));
            continue;
        };
        match local_crc {
            Some(Ok(crc)) => {
                if read {
                    totals.files_read += 1;
                    if let Some(m) = manifest {
                        let record = FileRecord { size: file.size, mtime_ns: file.mtime_ns, crc32c: crc };
                        let _ = m.record(&dest, &file.rel, &record, object.generation.as_deref());
                    }
                }
                let cloud_crc = object.crc32c.as_deref().and_then(parse_crc32c_b64);
                if object.size == file.size && cloud_crc == Some(crc) {
                    totals.verified += 1;
                    totals.verified_bytes += file.size;
                    rows.push(row(FileStatus::Verified, Some(&object), None));
                } else {
                    totals.mismatch += 1;
                    let note = format!(
                        "this computer: {} bytes, crc32c {}; cloud: {} bytes, crc32c {}",
                        file.size,
                        crc32c_b64(crc),
                        object.size,
                        object.crc32c.as_deref().unwrap_or("?")
                    );
                    rows.push(row(FileStatus::Mismatch, Some(&object), Some(note)));
                }
            }
            Some(Err(error)) => {
                totals.unreadable += 1;
                rows.push(row(FileStatus::Unreadable, Some(&object), Some(format!("couldn't read the file: {error}"))));
            }
            None => unreachable!("files with a cloud object are always checked"),
        }
    }
    for (rel, object) in cloud {
        totals.cloud_only += 1;
        rows.push(ReceiptFile {
            path: rel,
            status: FileStatus::CloudOnly,
            size: object.size,
            crc32c: object.crc32c,
            md5: object.md5_hash,
            generation: object.generation,
            cloud_updated: object.updated,
            note: Some("in the cloud only".into()),
        });
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));

    let complete = totals.mismatch == 0 && totals.missing_in_cloud == 0 && totals.unreadable == 0 && totals.online_only == 0;
    Ok(Receipt {
        generator: format!("JetStream {}", env!("CARGO_PKG_VERSION")),
        account: account.to_string(),
        local_root: local_root.display().to_string(),
        destination: dest,
        started_at,
        finished_at: rfc3339(now_secs()),
        result: if complete { "complete" } else { "incomplete" },
        totals,
        elapsed_seconds: (started.elapsed().as_secs_f64() * 100.0).round() / 100.0,
        files: rows,
    })
}

/// One CSV field, quoted when needed.
pub fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) || value.starts_with(' ') || value.ends_with(' ') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// CSV: a commented summary, then one row per file.
pub fn to_csv(receipt: &Receipt) -> String {
    let t = &receipt.totals;
    let mut out = String::new();
    for (key, value) in [
        ("JetStream upload receipt", receipt.result.to_string()),
        ("Folder", receipt.local_root.clone()),
        ("Destination", receipt.destination.clone()),
        ("Checked by", receipt.account.clone()),
        ("Started", receipt.started_at.clone()),
        ("Finished", receipt.finished_at.clone()),
        ("Generated by", receipt.generator.clone()),
        (
            "Totals",
            format!(
                "{} of {} files verified; {} different; {} missing in the cloud; {} unreadable; {} online-only; {} in the cloud only",
                t.verified, t.local_files, t.mismatch, t.missing_in_cloud, t.unreadable, t.online_only, t.cloud_only
            ),
        ),
    ] {
        out.push_str(&format!("# {key}: {}\n", value.replace('\n', " ")));
    }
    out.push_str("path,status,size,crc32c,md5,generation,cloud_updated,note\n");
    for f in &receipt.files {
        let fields = [
            csv_field(&f.path),
            f.status.label().to_string(),
            f.size.to_string(),
            csv_field(f.crc32c.as_deref().unwrap_or("")),
            csv_field(f.md5.as_deref().unwrap_or("")),
            f.generation.clone().unwrap_or_default(),
            f.cloud_updated.clone().unwrap_or_default(),
            csv_field(f.note.as_deref().unwrap_or("")),
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// Problem rows per kind kept by `view` (the counts cover all of them).
pub const VIEW_PROBLEMS_PER_KIND: usize = 500;
/// File rows in the printable receipt (the CSV has them all).
const HTML_MAX_ROWS: usize = 20_000;

/// What the app's receipt sheet shows, from a receipt's JSON: everything but the verified
/// file list, with up to `per_kind` problem rows of each kind.
pub fn view(receipt: &serde_json::Value, per_kind: usize) -> serde_json::Value {
    use serde_json::json;
    let mut groups: Vec<(String, Vec<serde_json::Value>, u64)> =
        ["missing_in_cloud", "mismatch", "unreadable"].iter().map(|k| (k.to_string(), Vec::new(), 0)).collect();
    let mut online_only = (Vec::new(), 0u64);
    for file in receipt["files"].as_array().into_iter().flatten() {
        let status = file["status"].as_str().unwrap_or("");
        let online = status == "unreadable" && file["note"].as_str().is_some_and(|n| n.contains("online-only"));
        if online {
            online_only.1 += 1;
            if online_only.0.len() < per_kind {
                online_only.0.push(file.clone());
            }
            continue;
        }
        if let Some(group) = groups.iter_mut().find(|(k, ..)| k == status) {
            group.2 += 1;
            if group.1.len() < per_kind {
                group.1.push(file.clone());
            }
        }
    }
    let mut problems: Vec<serde_json::Value> = groups.into_iter().filter(|g| g.2 > 0).map(|(kind, files, count)| json!({ "kind": kind, "count": count, "files": files })).collect();
    if online_only.1 > 0 {
        problems.push(json!({ "kind": "online_only", "count": online_only.1, "files": online_only.0 }));
    }
    let mut out = receipt.clone();
    if let Some(map) = out.as_object_mut() {
        map.remove("files");
        map.insert("problems".into(), json!(problems));
    }
    out
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn human_bytes(n: u64) -> String {
    let units = ["bytes", "KB", "MB", "GB", "TB", "PB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} bytes")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A printable, self-contained receipt (open it in a browser to print or save as PDF).
pub fn to_html(receipt: &serde_json::Value) -> String {
    let s = |key: &str| receipt[key].as_str().unwrap_or("").to_string();
    let t = &receipt["totals"];
    let n = |key: &str| t[key].as_u64().unwrap_or(0);
    let complete = s("result") == "complete";
    let not_verified = n("mismatch") + n("missing_in_cloud") + n("unreadable") + n("online_only");
    let headline = if complete {
        format!("All {} files verified", thousands(n("verified")))
    } else {
        format!("{} of {} files not verified", thousands(not_verified), thousands(n("local_files")))
    };
    let files: Vec<&serde_json::Value> = receipt["files"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let label = |status: &str, note: &str| match status {
        "verified" => "Verified",
        "mismatch" => "Different",
        "missing_in_cloud" => "Not in the cloud",
        "cloud_only" => "Cloud only",
        "unreadable" if note.contains("online-only") => "Online-only",
        "unreadable" => "Couldn't read",
        _ => "?",
    };
    let mut rows = String::new();
    let mut problems = String::new();
    for f in &files {
        let status = f["status"].as_str().unwrap_or("");
        let note = f["note"].as_str().unwrap_or("");
        let cells = format!(
            "<td class=\"path\">{}</td><td class=\"st {status}\">{}</td><td class=\"num\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td>",
            html_escape(f["path"].as_str().unwrap_or("")),
            label(status, note),
            human_bytes(f["size"].as_u64().unwrap_or(0)),
            html_escape(f["crc32c"].as_str().unwrap_or("")),
            html_escape(f["md5"].as_str().unwrap_or("")),
        );
        if !matches!(status, "verified" | "cloud_only") {
            problems.push_str(&format!("<tr>{cells}<td>{}</td></tr>\n", html_escape(note)));
        }
    }
    for f in files.iter().take(HTML_MAX_ROWS) {
        let status = f["status"].as_str().unwrap_or("");
        rows.push_str(&format!(
            "<tr><td class=\"path\">{}</td><td class=\"st {status}\">{}</td><td class=\"num\">{}</td><td class=\"mono\">{}</td></tr>\n",
            html_escape(f["path"].as_str().unwrap_or("")),
            label(status, f["note"].as_str().unwrap_or("")),
            human_bytes(f["size"].as_u64().unwrap_or(0)),
            html_escape(f["crc32c"].as_str().unwrap_or("")),
        ));
    }
    let more = if files.len() > HTML_MAX_ROWS {
        format!("<p class=\"note\">Showing the first {} of {} files. The CSV next to this receipt lists them all.</p>", thousands(HTML_MAX_ROWS as u64), thousands(files.len() as u64))
    } else {
        String::new()
    };
    let fact = |label: &str, value: String| format!("<div><dt>{label}</dt><dd>{}</dd></div>", html_escape(&value));
    let facts = [
        fact("Folder", s("local_root")),
        fact("Cloud", s("destination")),
        fact("Checked by", s("account")),
        fact("Started", s("started_at")),
        fact("Finished", s("finished_at")),
        fact("Checksums", format!("{} read now, {} from earlier uploads", thousands(n("files_read")), thousands(n("local_files").saturating_sub(n("files_read") + n("missing_in_cloud"))))),
        fact("Generated by", s("generator")),
    ]
    .join("");
    let stat = |value: String, label: &str| format!("<div class=\"stat\"><b>{value}</b><span>{label}</span></div>");
    let stats = [
        stat(thousands(n("verified")), "verified"),
        stat(human_bytes(n("verified_bytes")), "verified data"),
        stat(thousands(n("missing_in_cloud")), "not in the cloud"),
        stat(thousands(n("mismatch")), "different"),
        stat(thousands(n("unreadable")), "couldn't read"),
        stat(thousands(n("cloud_only")), "in the cloud only"),
    ]
    .join("");
    let problem_section = if problems.is_empty() {
        String::new()
    } else {
        format!(
            "<h2>Not verified</h2><table><thead><tr><th>File</th><th>Status</th><th class=\"num\">Size</th><th>CRC32C (cloud)</th><th>MD5 (cloud)</th><th>Why</th></tr></thead><tbody>{problems}</tbody></table>"
        )
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Upload receipt · {title}</title>
<style>
:root {{ color-scheme: light; --ink:#14202e; --muted:#5d6b7c; --line:#dde3ea; --ok:#127a46; --bad:#b4590a; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; font: 14px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, Roboto, sans-serif; color: var(--ink); background: #f4f6f9; }}
main {{ max-width: 960px; margin: 32px auto; background: #fff; border: 1px solid var(--line); border-radius: 14px; padding: 36px 40px; }}
header {{ display: flex; gap: 18px; align-items: center; border-bottom: 1px solid var(--line); padding-bottom: 22px; }}
.seal {{ width: 56px; height: 56px; border-radius: 50%; display: grid; place-items: center; font-size: 28px; color: #fff; background: var(--ok); flex: none; }}
.seal.bad {{ background: var(--bad); }}
h1 {{ font-size: 22px; margin: 0; }}
.kicker {{ text-transform: uppercase; letter-spacing: .08em; font-size: 11px; color: var(--muted); }}
dl {{ display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px 28px; margin: 22px 0; }}
dl div {{ min-width: 0; }} dt {{ color: var(--muted); font-size: 12px; }} dd {{ margin: 0; word-break: break-all; }}
.stats {{ display: grid; grid-template-columns: repeat(6, 1fr); gap: 10px; margin: 8px 0 26px; }}
.stat {{ border: 1px solid var(--line); border-radius: 10px; padding: 10px 12px; }}
.stat b {{ display: block; font-size: 18px; font-variant-numeric: tabular-nums; }} .stat span {{ color: var(--muted); font-size: 12px; }}
h2 {{ font-size: 15px; margin: 26px 0 8px; }}
table {{ width: 100%; border-collapse: collapse; font-size: 12px; }}
th, td {{ text-align: left; padding: 5px 8px; border-bottom: 1px solid var(--line); vertical-align: top; }}
th {{ color: var(--muted); font-weight: 600; }}
.num {{ text-align: right; white-space: nowrap; }} .mono {{ font-family: ui-monospace, Menlo, Consolas, monospace; font-size: 11px; }}
.path {{ word-break: break-all; }}
.st.verified {{ color: var(--ok); }} .st.mismatch, .st.missing_in_cloud, .st.unreadable {{ color: var(--bad); font-weight: 600; }} .st.cloud_only {{ color: var(--muted); }}
.note {{ color: var(--muted); font-size: 12px; }}
footer {{ margin-top: 26px; color: var(--muted); font-size: 12px; }}
@media print {{ body {{ background: #fff; }} main {{ margin: 0; border: none; padding: 0; }} tr {{ break-inside: avoid; }} }}
</style></head>
<body><main>
<header><div class="seal{seal_class}">{seal}</div><div><div class="kicker">JetStream upload receipt</div><h1>{headline}</h1></div></header>
<dl>{facts}</dl>
<div class="stats">{stats}</div>
{problem_section}
<h2>Files</h2>
<table><thead><tr><th>File</th><th>Status</th><th class="num">Size</th><th>CRC32C (cloud)</th></tr></thead><tbody>
{rows}</tbody></table>
{more}
<footer>Each file on this computer was matched with its copy in Google Cloud Storage and their CRC32C checksums compared. "Verified" means the same size and checksum. Files only in the cloud aren't counted against this folder.</footer>
</main></body></html>
"#,
        title = html_escape(&s("finished_at")),
        seal_class = if complete { "" } else { " bad" },
        seal = if complete { "✓" } else { "!" },
        headline = html_escape(&headline),
    )
}

/// Write `<dir>/<UTC time>.csv` and `.json`; returns both paths.
pub fn write_receipt(receipt: &Receipt, dir: &Path) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    // Never replace an earlier receipt, even one finished in the same second.
    let base = receipt.finished_at.replace(':', "-");
    let mut stem = base.clone();
    let mut n = 1;
    while dir.join(format!("{stem}.json")).exists() || dir.join(format!("{stem}.csv")).exists() {
        n += 1;
        stem = format!("{base}-{n}");
    }
    let csv = dir.join(format!("{stem}.csv"));
    let json = dir.join(format!("{stem}.json"));
    std::fs::write(&csv, to_csv(receipt))?;
    std::fs::write(&json, serde_json::to_vec_pretty(receipt)?)?;
    std::fs::write(json.with_extension("html"), to_html(&serde_json::to_value(receipt)?))?;
    Ok((csv, json))
}

/// A folder-name-safe slug for a synced folder's receipts directory.
pub fn slug(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    out.truncate(80);
    if out.is_empty() {
        "folder".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_825_600), "2000-02-29T12:00:00Z");
        assert_eq!(rfc3339(1_767_323_045), "2026-01-02T03:04:05Z");
    }

    #[test]
    fn csv_fields_are_quoted_when_needed() {
        assert_eq!(csv_field("plain.jpg"), "plain.jpg");
        assert_eq!(csv_field("a,b.jpg"), "\"a,b.jpg\"");
        assert_eq!(csv_field("say \"hi\".txt"), "\"say \"\"hi\"\".txt\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn receipts_in_the_same_second_get_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let receipt = Receipt {
            generator: "test".into(),
            account: "a".into(),
            local_root: "/x".into(),
            destination: "gs://b/".into(),
            started_at: rfc3339(0),
            finished_at: rfc3339(0),
            result: "complete",
            totals: Totals::default(),
            elapsed_seconds: 0.0,
            files: Vec::new(),
        };
        let (first, _) = write_receipt(&receipt, dir.path()).unwrap();
        let (second, _) = write_receipt(&receipt, dir.path()).unwrap();
        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
        assert!(first.with_extension("html").exists(), "printable receipt written too");
    }

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "generator": "JetStream test", "account": "diver@example.gov", "local_root": "C:\\Reef <2024>", "destination": "gs://b/reef/",
            "started_at": "2026-01-01T00:00:00Z", "finished_at": "2026-01-01T00:01:00Z", "result": "incomplete",
            "totals": { "local_files": 4, "verified": 1, "verified_bytes": 2048, "missing_in_cloud": 1, "mismatch": 1, "unreadable": 0, "online_only": 1, "cloud_only": 1, "files_read": 1 },
            "files": [
                { "path": "a.jpg", "status": "verified", "size": 2048, "crc32c": "AAAAAA==" },
                { "path": "b<script>.jpg", "status": "missing_in_cloud", "size": 10, "note": "not in the cloud" },
                { "path": "c.jpg", "status": "mismatch", "size": 10, "note": "this computer: 10 bytes" },
                { "path": "d.jpg", "status": "unreadable", "size": 0, "note": "online-only file (OneDrive): not on this computer" },
                { "path": "e.jpg", "status": "cloud_only", "size": 5, "note": "in the cloud only" }
            ]
        })
    }

    #[test]
    fn the_app_view_groups_problems_and_leaves_out_verified_files() {
        let view = view(&sample(), 500);
        assert!(view.get("files").is_none());
        let kinds: Vec<(&str, u64)> = view["problems"].as_array().unwrap().iter().map(|p| (p["kind"].as_str().unwrap(), p["count"].as_u64().unwrap())).collect();
        assert_eq!(kinds, vec![("missing_in_cloud", 1), ("mismatch", 1), ("online_only", 1)]);
        let capped = super::view(&sample(), 0);
        assert_eq!(capped["problems"][0]["files"].as_array().unwrap().len(), 0);
        assert_eq!(capped["problems"][0]["count"], 1);
    }

    #[test]
    fn the_printable_receipt_escapes_names_and_lists_problems() {
        let html = to_html(&sample());
        assert!(html.contains("3 of 4 files not verified"), "{html}");
        assert!(html.contains("b&lt;script&gt;.jpg") && !html.contains("b<script>"));
        assert!(html.contains("C:\\Reef &lt;2024&gt;"));
        assert!(html.contains("<h2>Not verified</h2>"));
        assert!(html.contains("Online-only") && html.contains("Not in the cloud") && html.contains("2.0 KB"));
        let complete = to_html(&serde_json::json!({ "result": "complete", "totals": { "verified": 1234, "local_files": 1234 }, "files": [] }));
        assert!(complete.contains("All 1,234 files verified") && !complete.contains("Not verified</h2>"));
    }

    #[test]
    fn slugs_folder_names() {
        assert_eq!(slug("Kona survey photos (2026)"), "Kona-survey-photos-2026");
        assert_eq!(slug("..//"), "folder");
    }
}
