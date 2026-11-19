//! Catalogs of files in a bucket, ported from the Python "Cloud Catalog Tool"
//! (`jetstream/services_dataset.py`) so the desktop app can build them natively.
//!
//! - **Same results:** filters, grouping, site instances and the OASIS export
//!   (`instances` / `instances_grouped` / `site_instances`) give the same output, byte for
//!   byte, as the Python tool for the same request (tests/test_catalog_parity.py checks).
//! - **Any kind of file:** `kinds` adds presets beyond images and video (camera RAW,
//!   science data, tables, documents, audio and sonar, all files) plus custom extensions.
//!   Images stay the default.
//! - **Detailed exports:** CSV or JSON with each file's size, type, original dates, checksums
//!   and storage class, and optional dataset information (NOAA InPort / ISO 19115).
//!
//! The listing is kept (`Listing`) so changing the grouping regroups without listing again.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::sync::atomic::Ordering;

use anyhow::{bail, Result};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};

use crate::fileops::OpProgress;
use crate::gcs::{GcsClient, FILE_CREATED_KEY, FILE_MTIME_KEY};

/// The Python tool's defaults, kept exactly so default catalogs match.
pub const IMAGE_EXTENSIONS: &[&str] = &[".bmp", ".jpeg", ".jpg", ".png", ".tif", ".tiff", ".webp"];
pub const VIDEO_EXTENSIONS: &[&str] = &[".avi", ".m4v", ".mkv", ".mov", ".mp4", ".mpeg", ".mpg", ".webm"];
/// User-supplied regexes are capped, as in Python.
pub const MAX_REGEX_LENGTH: usize = 200;
/// Files kept by default (the Python tool stops at 50,000).
pub const DEFAULT_MAX_RESULTS: usize = 1_000_000;

/// File kind presets: (id, label, extensions). "all" keeps every file.
pub const KINDS: &[(&str, &str, &[&str])] = &[
    ("images", "Images", IMAGE_EXTENSIONS),
    ("raw", "Camera RAW", &[".dng", ".cr2", ".cr3", ".nef", ".arw", ".orf", ".rw2", ".raf"]),
    ("video", "Video", VIDEO_EXTENSIONS),
    ("science", "Science data", &[".nc", ".nc4", ".cdf", ".h5", ".hdf5", ".he5", ".hdf", ".zarr", ".parquet", ".grib", ".grib2", ".grb", ".grb2", ".mat", ".las", ".laz", ".shp", ".gpkg", ".geojson", ".kml", ".kmz"]),
    ("tables", "Tables and text", &[".csv", ".tsv", ".txt", ".json", ".jsonl", ".xml", ".yaml", ".yml", ".log", ".dat", ".asc", ".cnv", ".btl"]),
    ("documents", "Documents", &[".pdf", ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".md", ".rtf", ".html", ".htm"]),
    ("audio", "Audio and sonar", &[".wav", ".flac", ".mp3", ".aif", ".aiff", ".ogg", ".m4a", ".raw", ".xtf", ".bag", ".all", ".s7k", ".kmall", ".gsf", ".jsf", ".sdf"]),
    ("all", "All files", &[]),
];

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupByMode {
    None,
    #[default]
    FilenameToken,
    FolderSegment,
    Regex,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFormat {
    Flat,
    Grouped,
    #[default]
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SiteInstanceMode {
    #[default]
    None,
    Manual,
    Threshold,
    FolderBoundary,
    FilenamePattern,
}

/// Manual group mapping, in the order given (the first matching group wins, as in Python).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Overrides(pub Vec<(String, Vec<String>)>);

impl<'de> Deserialize<'de> for Overrides {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = Overrides;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of group name to patterns")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> std::result::Result<Overrides, M::Error> {
                let mut out = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Vec<String>>()? {
                    out.push((key, value));
                }
                Ok(Overrides(out))
            }
            /// [[group, [patterns]], ...] keeps the order whatever the group names are.
            fn visit_seq<S: serde::de::SeqAccess<'de>>(self, mut seq: S) -> std::result::Result<Overrides, S::Error> {
                let mut out = Vec::new();
                while let Some((key, value)) = seq.next_element::<(String, Vec<String>)>()? {
                    out.push((key, value));
                }
                Ok(Overrides(out))
            }
            fn visit_unit<E>(self) -> std::result::Result<Overrides, E> {
                Ok(Overrides::default())
            }
        }
        deserializer.deserialize_any(Visit)
    }
}

/// How files are grouped (changeable without listing again).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Grouping {
    #[serde(default)]
    pub catalog_format: CatalogFormat,
    #[serde(default)]
    pub group_by_mode: GroupByMode,
    #[serde(default = "default_delimiter")]
    pub filename_delimiter: String,
    #[serde(default = "default_token_count")]
    pub filename_token_count: usize,
    #[serde(default)]
    pub folder_segment_index: usize,
    #[serde(default)]
    pub regex_pattern: Option<String>,
    #[serde(default = "default_group_index")]
    pub regex_group_index: usize,
    #[serde(default)]
    pub manual_overrides: Overrides,
    #[serde(default)]
    pub selected_groups: Vec<String>,
    #[serde(default)]
    pub site_instance_mode: SiteInstanceMode,
    #[serde(default = "default_threshold")]
    pub site_instance_threshold: usize,
    #[serde(default)]
    pub site_instance_pattern: Option<String>,
    #[serde(default)]
    pub site_instance_folder_segment_index: usize,
    #[serde(default)]
    pub manual_site_groups: Vec<String>,
}

fn default_delimiter() -> String {
    "_".into()
}
fn default_token_count() -> usize {
    3
}
fn default_group_index() -> usize {
    1
}
fn default_threshold() -> usize {
    25
}

impl Default for Grouping {
    fn default() -> Self {
        serde_json::from_str("{}").expect("defaults")
    }
}

/// What to list (the Python tool's scan request, plus `kinds`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogRequest {
    pub gcs_prefix: String,
    /// Kind presets (see KINDS). Empty: the Python behaviour (`include_images`,
    /// `include_videos`, or `extensions` alone).
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default = "yes")]
    pub include_images: bool,
    #[serde(default = "yes")]
    pub include_videos: bool,
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
    #[serde(default)]
    pub filename_filter: Option<String>,
    #[serde(default)]
    pub folder_filter: Option<String>,
    #[serde(default, deserialize_with = "names_list")]
    pub ignore_folder_names: Vec<String>,
    #[serde(default)]
    pub max_path_depth: Option<usize>,
    /// 0: the default cap.
    #[serde(default)]
    pub max_results: usize,
    #[serde(flatten)]
    pub grouping: Grouping,
}

/// A list, or a comma-separated string; blanks and case-insensitive repeats dropped.
fn names_list<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Names {
        Text(String),
        List(Vec<Option<String>>),
        Nothing(()),
    }
    let items: Vec<String> = match Names::deserialize(deserializer)? {
        Names::Text(text) => text.split(',').map(str::to_string).collect(),
        Names::List(list) => list.into_iter().flatten().collect(),
        Names::Nothing(()) => Vec::new(),
    };
    let mut seen = HashSet::new();
    Ok(items.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && seen.insert(s.to_lowercase())).collect())
}

impl CatalogRequest {
    /// "gs://b/p" -> "gs://b/p/", as the Python validator does.
    pub fn normalized_prefix(&self) -> Result<String> {
        if !self.gcs_prefix.starts_with("gs://") {
            bail!("Path must start with gs://");
        }
        Ok(format!("{}/", self.gcs_prefix.trim_end_matches('/')))
    }

    /// Extensions to keep, sorted (empty with the "all" kind: every file).
    pub fn extensions_used(&self) -> Result<Vec<String>> {
        let normalize = |e: &str| {
            let e = e.trim().to_lowercase();
            if e.starts_with('.') {
                e
            } else {
                format!(".{e}")
            }
        };
        let custom: Vec<String> = self.extensions.iter().flatten().filter(|e| !e.trim().is_empty()).map(|e| normalize(e)).collect();
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        if self.kinds.is_empty() {
            // The Python rules.
            if !custom.is_empty() {
                set.extend(custom);
            } else {
                if self.include_images {
                    set.extend(IMAGE_EXTENSIONS.iter().map(|s| s.to_string()));
                }
                if self.include_videos {
                    set.extend(VIDEO_EXTENSIONS.iter().map(|s| s.to_string()));
                }
            }
            if set.is_empty() {
                bail!("At least one media type or extension must be selected");
            }
            return Ok(set.into_iter().collect());
        }
        if self.kinds.iter().any(|k| k == "all") {
            return Ok(Vec::new());
        }
        for kind in &self.kinds {
            let Some((_, _, exts)) = KINDS.iter().find(|(id, _, _)| id == kind) else { bail!("unknown kind of file: {kind}") };
            set.extend(exts.iter().map(|s| s.to_string()));
        }
        set.extend(custom);
        if set.is_empty() {
            bail!("Choose at least one kind of file or extension");
        }
        Ok(set.into_iter().collect())
    }
}

/// One listed file with what the detailed exports show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRow {
    pub name: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub storage_class: Option<String>,
    pub crc32c: Option<String>,
    pub md5: Option<String>,
    pub generation: Option<String>,
    /// Upload time (RFC 3339).
    pub uploaded: Option<String>,
    /// The original file's modified and created times (seconds), when recorded.
    pub modified: Option<i64>,
    pub created: Option<i64>,
}

impl FileRow {
    fn from_raw(raw: &serde_json::Value) -> Option<Self> {
        let s = |key: &str| raw.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let meta = |key: &str| raw.get("metadata").and_then(|m| m.get(key)).and_then(|v| v.as_str()).and_then(|v| v.trim().parse::<i64>().ok());
        Some(Self {
            name: s("name")?,
            size: s("size").and_then(|v| v.parse().ok()).unwrap_or(0),
            content_type: s("contentType"),
            storage_class: s("storageClass"),
            crc32c: s("crc32c"),
            md5: s("md5Hash"),
            generation: s("generation"),
            uploaded: s("timeCreated"),
            modified: meta(FILE_MTIME_KEY),
            created: meta(FILE_CREATED_KEY),
        })
    }
}

/// A listed prefix: the files that passed the filters, in listing (name) order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listing {
    pub gcs_prefix: String,
    pub bucket: String,
    pub files: Vec<FileRow>,
    pub listed: u64,
    pub truncated: bool,
    pub extensions_used: Vec<String>,
    pub request: CatalogRequest,
}

/// Python's os.path.splitext for a file name: (stem, ".ext"); leading dots aren't extensions.
fn splitext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if name[..dot].chars().any(|c| c != '.') => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn dirname(path: &str) -> &str {
    path.rfind('/').map(|i| &path[..i]).unwrap_or("")
}

/// Python's fnmatch.translate, as a whole-string regex (both sides already lowercased).
pub fn glob_regex(pattern: &str) -> Option<Regex> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::from("(?s)^");
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        match c {
            '*' => {
                if !out.ends_with(".*") {
                    out.push_str(".*");
                }
            }
            '?' => out.push('.'),
            '[' => {
                let mut j = i;
                if j < chars.len() && chars[j] == '!' {
                    j += 1;
                }
                if j < chars.len() && chars[j] == ']' {
                    j += 1;
                }
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                if j >= chars.len() {
                    out.push_str("\\[");
                } else {
                    let stuff: String = chars[i..j].iter().collect();
                    i = j + 1;
                    let (negate, body) = match stuff.strip_prefix('!') {
                        Some(rest) => (true, rest.to_string()),
                        None => (false, stuff),
                    };
                    if body.is_empty() {
                        // "[]" or "[!]" can't match anything / matches any char in Python.
                        out.push_str(if negate { "." } else { "[^\\s\\S]" });
                        continue;
                    }
                    out.push('[');
                    if negate {
                        out.push('^');
                    }
                    for ch in body.chars() {
                        match ch {
                            '\\' | '[' | ']' | '&' | '~' | '^' => {
                                out.push('\\');
                                out.push(ch);
                            }
                            _ => out.push(ch),
                        }
                    }
                    out.push(']');
                }
            }
            _ => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out.push('$');
    Regex::new(&out).ok()
}

fn has_glob(text: &str) -> bool {
    text.contains(['*', '?', '['])
}

/// A user regex: None if too long or invalid (the Python tool treats it the same).
pub fn user_regex(pattern: &str) -> Option<Regex> {
    if pattern.is_empty() || pattern.chars().count() > MAX_REGEX_LENGTH {
        return None;
    }
    Regex::new(pattern).ok()
}

/// Does object `name` pass the request's filters? (`prefix` is the listed prefix.)
struct Filters {
    prefix: String,
    exts: HashSet<String>,
    ignored: HashSet<String>,
    depth: Option<usize>,
    folder: Option<String>,
    filename: Option<Regex>,
}

impl Filters {
    fn new(req: &CatalogRequest, prefix: &str, exts: &[String]) -> Self {
        let filename = req.filename_filter.as_deref().map(str::trim).filter(|f| !f.is_empty()).and_then(|f| {
            let f = if has_glob(f) { f.to_string() } else { format!("*{f}*") };
            glob_regex(&f.to_lowercase())
        });
        Self {
            prefix: prefix.to_string(),
            exts: exts.iter().cloned().collect(),
            ignored: req.ignore_folder_names.iter().map(|n| n.to_lowercase()).collect(),
            depth: req.max_path_depth,
            folder: req.folder_filter.as_deref().filter(|f| !f.is_empty()).map(str::to_lowercase),
            filename,
        }
    }

    fn keep(&self, name: &str) -> bool {
        if name.ends_with('/') {
            return false;
        }
        let relative = if !self.prefix.is_empty() && name.starts_with(&self.prefix) { &name[self.prefix.len()..] } else { name };
        let segments: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();
        let folders = if segments.is_empty() { &segments[..] } else { &segments[..segments.len() - 1] };
        if !self.ignored.is_empty() && folders.iter().any(|s| self.ignored.contains(&s.to_lowercase())) {
            return false;
        }
        if self.depth.is_some_and(|d| folders.len() > d) {
            return false;
        }
        if self.folder.as_ref().is_some_and(|f| !name.to_lowercase().contains(f.as_str())) {
            return false;
        }
        let file = basename(name);
        let ext = splitext(file).1.to_lowercase();
        if !self.exts.is_empty() && !self.exts.contains(&ext) {
            return false;
        }
        if self.filename.as_ref().is_some_and(|re| !re.is_match(&file.to_lowercase())) {
            return false;
        }
        true
    }
}

const LIST_FIELDS: &str = "name,size,contentType,storageClass,crc32c,md5Hash,generation,timeCreated,metadata";

/// List gs://bucket/prefix and keep the files that pass the filters. `progress.done` counts
/// objects looked at, `progress.skipped` the ones filtered out, `progress.bytes` the bytes kept.
pub async fn scan(client: &GcsClient, request: &CatalogRequest, progress: &OpProgress) -> Result<Listing> {
    let gcs_prefix = request.normalized_prefix()?;
    let exts = request.extensions_used()?;
    let rest = &gcs_prefix[5..];
    let (bucket, prefix) = rest.split_once('/').map(|(b, p)| (b.to_string(), p.to_string())).unwrap_or((rest.to_string(), String::new()));
    if bucket.is_empty() {
        bail!("Expected gs://bucket/path");
    }
    let filters = Filters::new(request, &prefix, &exts);
    let cap = if request.max_results == 0 { DEFAULT_MAX_RESULTS } else { request.max_results };
    let mut files = Vec::new();
    let mut listed = 0u64;
    let mut truncated = false;
    let mut token: Option<String> = None;
    'pages: loop {
        if progress.cancel.load(Ordering::Relaxed) {
            bail!("stopped");
        }
        let (page, next) = client.list_raw(&bucket, &prefix, token.as_deref(), LIST_FIELDS, false).await?;
        for raw in &page {
            let Some(row) = FileRow::from_raw(raw) else { continue };
            listed += 1;
            progress.done.fetch_add(1, Ordering::Relaxed);
            if !filters.keep(&row.name) {
                progress.skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            progress.bytes.fetch_add(row.size, Ordering::Relaxed);
            files.push(row);
            if files.len() >= cap {
                truncated = true;
                break 'pages;
            }
        }
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    Ok(Listing { gcs_prefix, bucket, files, listed, truncated, extensions_used: exts, request: request.clone() })
}

/// A listing from object names instead of the bucket (tests and offline use).
pub fn listing_from_names(request: &CatalogRequest, names: &[&str]) -> Result<Listing> {
    let gcs_prefix = request.normalized_prefix()?;
    let exts = request.extensions_used()?;
    let rest = &gcs_prefix[5..];
    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    let filters = Filters::new(request, prefix, &exts);
    let cap = if request.max_results == 0 { DEFAULT_MAX_RESULTS } else { request.max_results };
    let mut files = Vec::new();
    let mut truncated = false;
    for name in names.iter().filter(|n| n.starts_with(prefix)) {
        if !filters.keep(name) {
            continue;
        }
        files.push(FileRow { name: name.to_string(), size: 0, content_type: None, storage_class: None, crc32c: None, md5: None, generation: None, uploaded: None, modified: None, created: None });
        if files.len() >= cap {
            truncated = true;
            break;
        }
    }
    Ok(Listing { gcs_prefix: gcs_prefix.clone(), bucket: bucket.to_string(), files, listed: names.len() as u64, truncated, extensions_used: exts, request: request.clone() })
}

/// gs:// URIs for listed names (Python's build_gcs_uris).
pub fn gcs_uris(gcs_prefix: &str, names: &[&str]) -> Vec<String> {
    let rest = gcs_prefix.trim_start_matches("gs://");
    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    let normalized = prefix.trim_end_matches('/');
    names
        .iter()
        .map(|name| {
            if name.starts_with("gs://") {
                name.to_string()
            } else if !normalized.is_empty() && name.starts_with(&format!("{normalized}/")) {
                format!("gs://{bucket}/{name}")
            } else {
                format!("gs://{bucket}/{}", format!("{normalized}/{name}").trim_matches('/'))
            }
        })
        .collect()
}

fn uri_path(uri: &str) -> &str {
    match uri.strip_prefix("gs://") {
        Some(rest) => rest.split_once('/').map(|(_, p)| p).unwrap_or(""),
        None => uri,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroupSummary {
    pub group_key: String,
    pub count: usize,
    pub is_site_instance: bool,
}

/// The catalog's `metadata`, in the Python tool's key order, plus what the desktop adds.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogMetadata {
    pub group_by_mode: GroupByMode,
    pub site_instance_mode: SiteInstanceMode,
    pub total_items: usize,
    pub total_groups: usize,
    pub extensions_used: Vec<String>,
    pub truncated: bool,
    pub total_matched: usize,
    pub generated_at: String,
    /// Desktop additions (the classic page ignores them): the request, to rebuild the
    /// catalog, and the dataset information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jetstream_request: Option<CatalogRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dataset: Option<serde_json::Value>,
}

/// The catalog, in the shape the Python API returns and saves (DatasetCatalogResponse).
#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub gcs_prefix: String,
    pub flat_instances: Vec<String>,
    pub grouped_instances: BTreeMap<String, Vec<String>>,
    pub site_instances: BTreeMap<String, Vec<String>>,
    pub groups: Vec<GroupSummary>,
    pub catalog_format: CatalogFormat,
    pub metadata: CatalogMetadata,
    /// Problems with the options (e.g. a regex the engine can't use); not saved.
    #[serde(skip)]
    pub warnings: Vec<String>,
    /// The grouping it was built with.
    #[serde(skip)]
    pub grouping: Option<Grouping>,
}

/// Manual groups with their patterns (and the glob regex for patterns with wildcards).
type ManualGroups = Vec<(String, Vec<(String, Option<Regex>)>)>;

fn group_key(g: &Grouping, uri: &str, regex: Option<&Regex>, manual: &ManualGroups) -> String {
    let path = uri_path(uri);
    let file = basename(path);
    let stem = splitext(file).0;
    match g.group_by_mode {
        GroupByMode::FilenameToken => {
            let parts: Vec<&str> = stem.split(g.filename_delimiter.as_str()).filter(|p| !p.is_empty()).collect();
            if parts.is_empty() {
                return "ungrouped".into();
            }
            parts[..g.filename_token_count.min(parts.len())].join(&g.filename_delimiter)
        }
        GroupByMode::FolderSegment => {
            let segments: Vec<&str> = dirname(path).split('/').filter(|s| !s.is_empty()).collect();
            if segments.is_empty() {
                return "root".into();
            }
            segments[g.folder_segment_index.min(segments.len() - 1)].to_string()
        }
        GroupByMode::Regex => {
            let Some(re) = regex else { return "unmatched".into() };
            match re.captures(file) {
                Some(caps) if g.regex_group_index < caps.len() => caps.get(g.regex_group_index).map(|m| m.as_str().to_string()).unwrap_or_else(|| "None".into()),
                Some(caps) => caps[0].to_string(),
                None => "unmatched".into(),
            }
        }
        GroupByMode::Manual if !manual.is_empty() => {
            let (lower_uri, lower_file) = (uri.to_lowercase(), file.to_lowercase());
            for (group, patterns) in manual {
                for (pattern, glob) in patterns {
                    let hit = match glob {
                        Some(re) => re.is_match(&lower_file) || re.is_match(&lower_uri),
                        None => lower_file.contains(pattern.as_str()) || lower_uri.contains(pattern.as_str()),
                    };
                    if hit {
                        return group.clone();
                    }
                }
            }
            "unmapped".into()
        }
        _ => "all".into(),
    }
}

fn site_instances(g: &Grouping, grouped: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    let pick = |f: &dyn Fn(&str, &Vec<String>) -> bool| grouped.iter().filter(|(k, v)| f(k, v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    match g.site_instance_mode {
        SiteInstanceMode::None => BTreeMap::new(),
        SiteInstanceMode::Manual => {
            let allow: HashSet<&str> = g.manual_site_groups.iter().map(String::as_str).collect();
            pick(&|k, _| allow.contains(k))
        }
        SiteInstanceMode::Threshold => pick(&|_, v| v.len() >= g.site_instance_threshold),
        SiteInstanceMode::FolderBoundary => pick(&|k, v| {
            let Some(first) = v.first() else { return false };
            let segments: Vec<&str> = dirname(uri_path(first)).split('/').filter(|s| !s.is_empty()).collect();
            !segments.is_empty() && segments[g.site_instance_folder_segment_index.min(segments.len() - 1)] == k
        }),
        SiteInstanceMode::FilenamePattern => match user_regex(g.site_instance_pattern.as_deref().unwrap_or("")) {
            Some(re) => pick(&|_, v| v.iter().any(|u| re.is_match(basename(uri_path(u))))),
            None => BTreeMap::new(),
        },
    }
}

/// Group a listing (Python's build_catalog, plus the router's metadata).
pub fn build(listing: &Listing, g: &Grouping, generated_at: &str) -> Result<Catalog> {
    if g.group_by_mode == GroupByMode::FilenameToken && g.filename_delimiter.is_empty() {
        bail!("The filename delimiter can't be empty");
    }
    if g.filename_token_count == 0 {
        bail!("Token count must be at least 1");
    }
    let mut warnings = Vec::new();
    let regex = match (&g.group_by_mode, g.regex_pattern.as_deref().filter(|p| !p.is_empty())) {
        (GroupByMode::Regex, Some(pattern)) => {
            let re = user_regex(pattern);
            if re.is_none() {
                warnings.push(format!("The pattern {pattern:?} isn't a usable regular expression (or is over {MAX_REGEX_LENGTH} characters), so every file is \"unmatched\"."));
            }
            re
        }
        _ => None,
    };
    if g.site_instance_mode == SiteInstanceMode::FilenamePattern && user_regex(g.site_instance_pattern.as_deref().unwrap_or("")).is_none() {
        warnings.push("The site instance pattern isn't a usable regular expression, so no group is a site instance.".into());
    }
    let manual: ManualGroups = g
        .manual_overrides
        .0
        .iter()
        .map(|(group, patterns)| {
            let patterns = patterns
                .iter()
                .map(|p| p.trim().to_lowercase())
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let glob = if has_glob(&p) { glob_regex(&p) } else { None };
                    (p, glob)
                })
                .collect();
            (group.clone(), patterns)
        })
        .collect();

    let names: Vec<&str> = listing.files.iter().map(|f| f.name.as_str()).collect();
    let uris = gcs_uris(&listing.gcs_prefix, &names);
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for uri in uris {
        grouped.entry(group_key(g, &uri, regex.as_ref(), &manual)).or_default().push(uri);
    }
    if !g.selected_groups.is_empty() {
        let allow: HashSet<&str> = g.selected_groups.iter().map(String::as_str).collect();
        grouped.retain(|k, _| allow.contains(k.as_str()));
    }
    for values in grouped.values_mut() {
        values.sort();
    }
    let mut flat: Vec<String> = grouped.values().flatten().cloned().collect();
    flat.sort();
    let sites = site_instances(g, &grouped);
    let groups = grouped.iter().map(|(k, v)| GroupSummary { group_key: k.clone(), count: v.len(), is_site_instance: sites.contains_key(k) }).collect();
    Ok(Catalog {
        gcs_prefix: listing.gcs_prefix.clone(),
        metadata: CatalogMetadata {
            group_by_mode: g.group_by_mode,
            site_instance_mode: g.site_instance_mode,
            total_items: flat.len(),
            total_groups: grouped.len(),
            extensions_used: listing.extensions_used.clone(),
            truncated: listing.truncated,
            total_matched: listing.files.len(),
            generated_at: generated_at.to_string(),
            jetstream_request: None,
            dataset: None,
        },
        flat_instances: flat,
        grouped_instances: grouped,
        site_instances: sites,
        groups,
        catalog_format: g.catalog_format,
        warnings,
        grouping: Some(g.clone()),
    })
}

/// The OASIS export (frozen contract, Python's to_export_payload).
pub fn oasis_payload(catalog: &Catalog) -> serde_json::Value {
    use serde_json::json;
    match catalog.catalog_format {
        CatalogFormat::Flat => json!({ "instances": catalog.flat_instances }),
        CatalogFormat::Grouped => json!({ "instances": catalog.grouped_instances }),
        CatalogFormat::Both => {
            let mut payload = serde_json::Map::new();
            payload.insert("instances".into(), json!(catalog.flat_instances));
            payload.insert("instances_grouped".into(), json!(catalog.grouped_instances));
            if !catalog.site_instances.is_empty() {
                payload.insert("site_instances".into(), json!(catalog.site_instances));
            }
            serde_json::Value::Object(payload)
        }
    }
}

/// The OASIS export as the Python tool writes it: json.dumps(payload, indent=2)
/// (non-ASCII escaped as \uXXXX, no trailing newline).
pub fn oasis_json(catalog: &Catalog) -> String {
    let mut out = String::new();
    python_json(&oasis_payload(catalog), 0, &mut out);
    out
}

fn python_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

fn python_json(value: &serde_json::Value, depth: usize, out: &mut String) {
    use serde_json::Value;
    let pad = |n: usize, out: &mut String| out.extend(std::iter::repeat_n(' ', n * 2));
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => python_string(s, out),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                pad(depth + 1, out);
                python_json(item, depth + 1, out);
            }
            out.push('\n');
            pad(depth, out);
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                pad(depth + 1, out);
                python_string(key, out);
                out.push_str(": ");
                python_json(item, depth + 1, out);
            }
            out.push('\n');
            pad(depth, out);
            out.push('}');
        }
    }
}

/// "2024-03-02T08:15:00Z" from Unix seconds.
pub fn iso_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Group and site instance of each catalogued file, in catalog (path) order.
fn rows_in_catalog<'a>(listing: &'a Listing, catalog: &'a Catalog) -> Vec<(&'a FileRow, String, &'a str, bool)> {
    let names: Vec<&str> = listing.files.iter().map(|f| f.name.as_str()).collect();
    let uris = gcs_uris(&listing.gcs_prefix, &names);
    let mut group_of: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for (group, values) in &catalog.grouped_instances {
        for uri in values {
            group_of.insert(uri.as_str(), group.as_str());
        }
    }
    let mut rows: Vec<_> = listing
        .files
        .iter()
        .zip(uris)
        .filter_map(|(file, uri)| {
            let group = *group_of.get(uri.as_str())?;
            Some((file, uri.clone(), group, catalog.site_instances.contains_key(group)))
        })
        .collect();
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    rows
}

const CSV_COLUMNS: &[&str] = &["path", "group", "site_instance", "size", "content_type", "storage_class", "modified", "created", "uploaded", "crc32c", "md5", "generation"];

fn csv_field(text: &str, out: &mut String) {
    if text.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&text.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(text);
    }
}

/// One row per catalogued file (dates in UTC).
pub fn detailed_csv(listing: &Listing, catalog: &Catalog) -> String {
    let mut out = CSV_COLUMNS.join(",");
    out.push_str("\r\n");
    for (file, uri, group, site) in rows_in_catalog(listing, catalog) {
        let fields = [
            uri,
            group.to_string(),
            site.to_string(),
            file.size.to_string(),
            file.content_type.clone().unwrap_or_default(),
            file.storage_class.clone().unwrap_or_default(),
            file.modified.map(iso_utc).unwrap_or_default(),
            file.created.map(iso_utc).unwrap_or_default(),
            file.uploaded.clone().unwrap_or_default(),
            file.crc32c.clone().unwrap_or_default(),
            file.md5.clone().unwrap_or_default(),
            file.generation.clone().unwrap_or_default(),
        ];
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            csv_field(field, &mut out);
        }
        out.push_str("\r\n");
    }
    out
}

/// The catalog with every file's details and the dataset information.
pub fn detailed_json(listing: &Listing, catalog: &Catalog, dataset: Option<&serde_json::Value>) -> serde_json::Value {
    use serde_json::json;
    let files: Vec<_> = rows_in_catalog(listing, catalog)
        .into_iter()
        .map(|(file, uri, group, site)| {
            json!({
                "path": uri,
                "group": group,
                "site_instance": site,
                "size": file.size,
                "content_type": file.content_type,
                "storage_class": file.storage_class,
                "modified": file.modified.map(iso_utc),
                "created": file.created.map(iso_utc),
                "uploaded": file.uploaded,
                "crc32c": file.crc32c,
                "md5": file.md5,
                "generation": file.generation,
            })
        })
        .collect();
    let r = &listing.request;
    json!({
        "catalog": {
            "gcs_prefix": catalog.gcs_prefix,
            "generated_at": catalog.metadata.generated_at,
            "kinds": if r.kinds.is_empty() { json!(null) } else { json!(r.kinds) },
            "extensions_used": catalog.metadata.extensions_used,
            "filters": {
                "filename": r.filename_filter,
                "folder": r.folder_filter,
                "ignore_folder_names": r.ignore_folder_names,
                "max_path_depth": r.max_path_depth,
            },
            "group_by_mode": catalog.metadata.group_by_mode,
            "site_instance_mode": catalog.metadata.site_instance_mode,
            "total_items": catalog.metadata.total_items,
            "total_groups": catalog.metadata.total_groups,
            "total_bytes": listing.files.iter().map(|f| f.size).sum::<u64>(),
            "truncated": catalog.metadata.truncated,
        },
        "dataset": dataset,
        "groups": catalog.groups,
        "files": files,
    })
}

/// Each group's files, in path order (groups in key order).
pub fn files_by_group<'a>(listing: &'a Listing, catalog: &'a Catalog) -> Vec<(&'a str, Vec<&'a FileRow>)> {
    let mut groups: BTreeMap<&str, Vec<&FileRow>> = BTreeMap::new();
    for (file, _, group, _) in rows_in_catalog(listing, catalog) {
        groups.entry(group).or_default().push(file);
    }
    groups.into_iter().collect()
}

/// Group key -> catalogued files (for the app's gallery), in path order.
pub fn group_files<'a>(listing: &'a Listing, catalog: &'a Catalog, group: Option<&str>) -> Vec<&'a FileRow> {
    rows_in_catalog(listing, catalog).into_iter().filter(|(_, _, g, _)| group.is_none_or(|want| *g == want)).map(|(f, ..)| f).collect()
}

// ----- What the app shows ------------------------------------------------------------------

/// Group rows sent to the app (the counts cover all groups).
pub const SUMMARY_GROUP_ROWS: usize = 2000;
const SUMMARY_SAMPLES: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct GroupRow {
    pub key: String,
    pub count: usize,
    pub bytes: u64,
    pub site: bool,
    /// In the catalog (false when `selected_groups` leaves it out).
    pub included: bool,
    pub samples: Vec<crate::browse::Entry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub gcs_prefix: String,
    pub listed: u64,
    pub matched: usize,
    pub matched_bytes: u64,
    pub truncated: bool,
    pub extensions_used: Vec<String>,
    pub total_items: usize,
    pub total_groups: usize,
    /// Groups before `selected_groups` (rows in `groups`: up to SUMMARY_GROUP_ROWS).
    pub all_groups: usize,
    pub site_groups: usize,
    pub groups: Vec<GroupRow>,
    pub warnings: Vec<String>,
    pub generated_at: String,
}

/// A listed file as the cloud browser's entries (for thumbnails and the preview pane).
pub fn entry(file: &FileRow) -> crate::browse::Entry {
    crate::browse::Entry {
        name: basename(&file.name).to_string(),
        path: file.name.clone(),
        is_folder: false,
        size: Some(file.size),
        updated: file.uploaded.clone(),
        storage_class: file.storage_class.clone(),
        content_type: file.content_type.clone(),
        generation: file.generation.clone(),
        modified: file.modified,
        created: file.created,
    }
}

/// Build the catalog and, when only some groups are selected, the full one the app's group
/// table shows (so left-out groups can be ticked back in).
pub fn build_with_table(listing: &Listing, grouping: &Grouping, generated_at: &str) -> Result<(Catalog, Option<Catalog>)> {
    let built = build(listing, grouping, generated_at)?;
    if grouping.selected_groups.is_empty() {
        return Ok((built, None));
    }
    let full = build(listing, &Grouping { selected_groups: Vec::new(), ..grouping.clone() }, generated_at)?;
    Ok((built, Some(full)))
}

pub fn summary(listing: &Listing, built: &Catalog, full: Option<&Catalog>) -> Summary {
    let table = full.unwrap_or(built);
    let mut by_group: std::collections::HashMap<&str, (u64, Vec<&FileRow>)> = std::collections::HashMap::new();
    for (group, files) in files_by_group(listing, table) {
        let bytes = files.iter().map(|f| f.size).sum();
        by_group.insert(group, (bytes, files.into_iter().take(SUMMARY_SAMPLES).collect()));
    }
    let groups = table
        .groups
        .iter()
        .take(SUMMARY_GROUP_ROWS)
        .map(|g| {
            let (bytes, samples) = by_group.remove(g.group_key.as_str()).unwrap_or_default();
            GroupRow {
                key: g.group_key.clone(),
                count: g.count,
                bytes,
                site: g.is_site_instance,
                included: built.grouped_instances.contains_key(&g.group_key),
                samples: samples.into_iter().map(entry).collect(),
            }
        })
        .collect();
    Summary {
        gcs_prefix: listing.gcs_prefix.clone(),
        listed: listing.listed,
        matched: listing.files.len(),
        matched_bytes: listing.files.iter().map(|f| f.size).sum(),
        truncated: listing.truncated,
        extensions_used: listing.extensions_used.clone(),
        total_items: built.metadata.total_items,
        total_groups: built.metadata.total_groups,
        all_groups: table.metadata.total_groups,
        site_groups: built.site_instances.len(),
        groups,
        warnings: built.warnings.clone(),
        generated_at: built.metadata.generated_at.clone(),
    }
}

/// A page of a group's files (all files with `group` None) as browser entries, with the
/// group's total. A group left out by `selected_groups` still shows its files.
pub fn group_page(listing: &Listing, built: &Catalog, group: Option<&str>, offset: usize, limit: usize) -> Result<(usize, Vec<crate::browse::Entry>)> {
    let left_out = match (&built.grouping, group) {
        (Some(g), Some(key)) if !g.selected_groups.is_empty() && !built.grouped_instances.contains_key(key) => Some(build(listing, &Grouping { selected_groups: Vec::new(), ..g.clone() }, &built.metadata.generated_at)?),
        _ => None,
    };
    let catalog = left_out.as_ref().unwrap_or(built);
    let files = group_files(listing, catalog, group);
    Ok((files.len(), files.iter().skip(offset).take(limit).map(|f| entry(f)).collect()))
}

/// Save a catalog with the JetStream server (shared with the classic catalog page):
/// POST /api/dataset-creator/catalogs. Returns the saved record's summary.
pub async fn save_to_server(base_url: &str, name: Option<&str>, catalog: &Catalog) -> Result<serde_json::Value> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)).build()?;
    let response = client
        .post(format!("{}/api/dataset-creator/catalogs", base_url.trim_end_matches('/')))
        .json(&serde_json::json!({ "name": name, "catalog": catalog }))
        .send()
        .await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        bail!("the JetStream server couldn't save it ({status}): {}", body.get("detail").map(|d| d.to_string()).unwrap_or_default());
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(names: &[&str], request: CatalogRequest) -> Listing {
        let exts = request.extensions_used().unwrap();
        let prefix = request.normalized_prefix().unwrap();
        let p = prefix.trim_start_matches("gs://").split_once('/').unwrap().1.to_string();
        let filters = Filters::new(&request, &p, &exts);
        let files = names
            .iter()
            .filter(|n| filters.keep(n))
            .map(|n| FileRow { name: n.to_string(), size: 10, content_type: None, storage_class: None, crc32c: None, md5: None, generation: None, uploaded: None, modified: None, created: None })
            .collect();
        Listing { gcs_prefix: prefix, bucket: "b".into(), files, listed: names.len() as u64, truncated: false, extensions_used: exts, request }
    }

    fn request(json: serde_json::Value) -> CatalogRequest {
        serde_json::from_value(json).unwrap()
    }

    const NAMES: &[&str] = &[
        "survey/site_A_001_2024.jpg",
        "survey/site_A_002_2024.JPG",
        "survey/site_B_001_2024.png",
        "survey/thumbs/site_B_001_2024.png",
        "survey/deep/er/site_C_9.tif",
        "survey/clip_01.mp4",
        "survey/notes.txt",
        "survey/.hidden.jpg",
        "survey/folder/",
        "survey/sst_2024.nc",
    ];

    #[test]
    fn python_helpers() {
        assert_eq!(splitext("a.tar.gz"), ("a.tar", ".gz"));
        assert_eq!(splitext(".bashrc"), (".bashrc", ""));
        assert_eq!(splitext("..x"), ("..x", ""));
        assert_eq!(splitext("file."), ("file", "."));
        assert!(glob_regex("*site_a*").unwrap().is_match("site_a_001.jpg"));
        assert!(glob_regex("img_[0-9]?.jpg").unwrap().is_match("img_12.jpg"));
        assert!(glob_regex("[!a]*").unwrap().is_match("b.jpg"));
        assert!(!glob_regex("[!a]*").unwrap().is_match("a.jpg"));
        assert!(glob_regex("x[.jpg").unwrap().is_match("x[.jpg"), "unclosed bracket is literal");
        assert_eq!(iso_utc(1_709_367_300), "2024-03-02T08:15:00Z");
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn default_is_images_and_video_like_python_and_images_kind_is_images_only() {
        let both = listing(NAMES, request(serde_json::json!({ "gcs_prefix": "gs://b/survey" })));
        assert_eq!(both.files.len(), 7, "{:?}", both.files.iter().map(|f| &f.name).collect::<Vec<_>>());
        let images = listing(NAMES, request(serde_json::json!({ "gcs_prefix": "gs://b/survey/", "kinds": ["images"] })));
        assert_eq!(images.files.len(), 6);
        let all = listing(NAMES, request(serde_json::json!({ "gcs_prefix": "gs://b/survey/", "kinds": ["all"] })));
        assert_eq!(all.files.len(), 9, "everything but the folder marker");
        let custom = listing(NAMES, request(serde_json::json!({ "gcs_prefix": "gs://b/survey/", "kinds": ["science"], "extensions": ["TXT"] })));
        assert_eq!(custom.files.len(), 2);
    }

    #[test]
    fn filters() {
        let r = request(serde_json::json!({ "gcs_prefix": "gs://b/survey/", "kinds": ["images"], "ignore_folder_names": "Thumbs, thumbs", "max_path_depth": 1, "filename_filter": "site" }));
        assert_eq!(r.ignore_folder_names, vec!["Thumbs"]);
        let l = listing(NAMES, r);
        let names: Vec<_> = l.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["survey/site_A_001_2024.jpg", "survey/site_A_002_2024.JPG", "survey/site_B_001_2024.png"]);
    }

    #[test]
    fn grouping_modes_and_oasis_export() {
        let l = listing(NAMES, request(serde_json::json!({ "gcs_prefix": "gs://b/survey/", "kinds": ["images"] })));
        let g: Grouping = serde_json::from_value(serde_json::json!({ "filename_token_count": 2 })).unwrap();
        let c = build(&l, &g, "t").unwrap();
        assert_eq!(c.grouped_instances.keys().collect::<Vec<_>>(), vec![".hidden", "site_A", "site_B", "site_C"]);
        assert_eq!(c.grouped_instances["site_A"], vec!["gs://b/survey/site_A_001_2024.jpg", "gs://b/survey/site_A_002_2024.JPG"]);

        let g: Grouping = serde_json::from_value(serde_json::json!({ "group_by_mode": "folder_segment", "folder_segment_index": 1, "site_instance_mode": "threshold", "site_instance_threshold": 2 })).unwrap();
        let c = build(&l, &g, "t").unwrap();
        assert_eq!(c.grouped_instances.keys().collect::<Vec<_>>(), vec!["deep", "survey", "thumbs"]);
        assert_eq!(c.site_instances.keys().collect::<Vec<_>>(), vec!["survey"]);
        let json = oasis_json(&c);
        assert!(json.starts_with("{\n  \"instances\": [\n    \"gs://b/survey/.hidden.jpg\",\n"), "{json}");
        assert!(json.contains("\"site_instances\": {\n    \"survey\": ["));
        assert!(!json.ends_with('\n'));

        let g: Grouping = serde_json::from_value(serde_json::json!({ "group_by_mode": "regex", "regex_pattern": "site_([A-Z])(_x)?", "regex_group_index": 2 })).unwrap();
        let c = build(&l, &g, "t").unwrap();
        assert!(c.grouped_instances.contains_key("None"), "unmatched optional group is Python's None: {:?}", c.grouped_instances.keys());
        let g: Grouping = serde_json::from_value(serde_json::json!({ "group_by_mode": "regex", "regex_pattern": "(?<=x)y" })).unwrap();
        let c = build(&l, &g, "t").unwrap();
        assert_eq!(c.grouped_instances.keys().collect::<Vec<_>>(), vec!["unmatched"]);
        assert_eq!(c.warnings.len(), 1);

        let g: Grouping = serde_json::from_value(serde_json::json!({ "group_by_mode": "manual", "manual_overrides": { "Bsite": ["*_b_*"], "Asite": ["site_a"] }, "selected_groups": ["Asite", "unmapped"], "catalog_format": "grouped" })).unwrap();
        let c = build(&l, &g, "t").unwrap();
        assert_eq!(c.grouped_instances.keys().collect::<Vec<_>>(), vec!["Asite", "unmapped"]);
        assert_eq!(oasis_payload(&c).as_object().unwrap().keys().collect::<Vec<_>>(), vec!["instances"]);
    }

    #[test]
    fn python_json_escapes_like_json_dumps() {
        let mut out = String::new();
        python_json(&serde_json::json!({ "a": ["Montérey 🐠\u{7f}\"\\"], "e": [], "o": {} }), 0, &mut out);
        assert_eq!(out, "{\n  \"a\": [\n    \"Mont\\u00e9rey \\ud83d\\udc20\\u007f\\\"\\\\\"\n  ],\n  \"e\": [],\n  \"o\": {}\n}");
    }

    #[test]
    fn detailed_exports() {
        let mut l = listing(NAMES, request(serde_json::json!({ "gcs_prefix": "gs://b/survey/", "kinds": ["images"] })));
        l.files[0].modified = Some(1_709_367_300);
        l.files[0].content_type = Some("image/jpeg".into());
        let c = build(&l, &Grouping::default(), "t").unwrap();
        let csv = detailed_csv(&l, &c);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], CSV_COLUMNS.join(","));
        assert_eq!(lines.len(), 1 + 6);
        assert!(csv.contains("gs://b/survey/site_A_001_2024.jpg,site_A_001,false,10,image/jpeg,,2024-03-02T08:15:00Z,"), "{csv}");
        let json = detailed_json(&l, &c, Some(&serde_json::json!({ "title": "Reef survey" })));
        assert_eq!(json["files"].as_array().unwrap().len(), 6);
        assert_eq!(json["dataset"]["title"], "Reef survey");
        assert_eq!(json["catalog"]["total_bytes"], 60);
    }
}
