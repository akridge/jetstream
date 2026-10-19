//! Fast local folder analysis: what a folder contains, what's big, old or junk.
//! Top-level subfolders are walked in parallel; progress counters can be polled while it runs.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use rayon::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub top_n: usize,
    /// Files older than this many days and at least `archive_min_bytes` are archive candidates.
    pub archive_age_days: u64,
    pub archive_min_bytes: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { top_n: 25, archive_age_days: 365, archive_min_bytes: 100 * 1024 * 1024 }
    }
}

/// Live counters for progress display; also the cancel switch.
#[derive(Default)]
pub struct ScanProgress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    pub name: String,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileItem {
    pub bytes: u64,
    /// Path relative to the scanned folder, with '/' separators.
    pub path: String,
    /// Seconds since the Unix epoch.
    pub modified: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FileGroup {
    pub files: u64,
    pub bytes: u64,
    /// Largest examples.
    pub examples: Vec<FileItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub path: String,
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    pub unreadable: u64,
    pub categories: Vec<Bucket>,
    pub extensions: Vec<Bucket>,
    pub top_folders: Vec<Bucket>,
    pub largest_files: Vec<FileItem>,
    pub archive_candidates: FileGroup,
    pub junk: FileGroup,
    pub oldest_modified: Option<u64>,
    pub newest_modified: Option<u64>,
    pub elapsed_seconds: f64,
    pub cancelled: bool,
}

const JUNK_NAMES: &[&str] = &["thumbs.db", ".ds_store", "desktop.ini", "ehthumbs.db"];
const JUNK_EXTENSIONS: &[&str] = &["tmp", "temp", "bak", "old", "swp", "dmp", "cache", "gstmp", "crdownload", "part"];

pub fn category(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng"
        | "orf" | "rw2" | "raf" => "Images",
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "m4v" | "mts" | "mpg" | "mpeg" => "Video",
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "aiff" => "Audio",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "odt" | "md" | "csv" => "Documents",
        "json" | "xml" | "parquet" | "nc" | "h5" | "hdf5" | "sqlite" | "db" | "shp" | "gpkg" | "geojson" | "tsv" | "feather"
        | "avro" | "ndjson" | "las" | "laz" => "Data",
        "zip" | "gz" | "tgz" | "tar" | "7z" | "rar" | "bz2" | "xz" => "Archives",
        _ => "Other",
    }
}

fn is_junk(name_lower: &str, ext: &str) -> bool {
    JUNK_NAMES.contains(&name_lower) || JUNK_EXTENSIONS.contains(&ext) || name_lower.starts_with("~$")
}

/// Keeps the N largest items seen.
struct TopN {
    n: usize,
    heap: BinaryHeap<Reverse<FileItem>>,
}

impl TopN {
    fn new(n: usize) -> Self {
        Self { n, heap: BinaryHeap::new() }
    }
    fn push(&mut self, item: FileItem) {
        if self.heap.len() < self.n {
            self.heap.push(Reverse(item));
        } else if let Some(Reverse(smallest)) = self.heap.peek() {
            if item.bytes > smallest.bytes {
                self.heap.pop();
                self.heap.push(Reverse(item));
            }
        }
    }
    fn merge(&mut self, other: TopN) {
        for Reverse(item) in other.heap {
            self.push(item);
        }
    }
    fn into_sorted(self) -> Vec<FileItem> {
        let mut items: Vec<_> = self.heap.into_iter().map(|Reverse(i)| i).collect();
        items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
        items
    }
}

struct Partial {
    files: u64,
    folders: u64,
    bytes: u64,
    unreadable: u64,
    categories: HashMap<&'static str, (u64, u64)>,
    extensions: HashMap<String, (u64, u64)>,
    largest: TopN,
    archive: (u64, u64, TopN),
    junk: (u64, u64, TopN),
    oldest: Option<u64>,
    newest: Option<u64>,
}

impl Partial {
    fn new(n: usize) -> Self {
        Self {
            files: 0,
            folders: 0,
            bytes: 0,
            unreadable: 0,
            categories: HashMap::new(),
            extensions: HashMap::new(),
            largest: TopN::new(n),
            archive: (0, 0, TopN::new(n)),
            junk: (0, 0, TopN::new(n)),
            oldest: None,
            newest: None,
        }
    }

    fn merge(&mut self, other: Partial) {
        self.files += other.files;
        self.folders += other.folders;
        self.bytes += other.bytes;
        self.unreadable += other.unreadable;
        for (k, (f, b)) in other.categories {
            let e = self.categories.entry(k).or_default();
            e.0 += f;
            e.1 += b;
        }
        for (k, (f, b)) in other.extensions {
            let e = self.extensions.entry(k).or_default();
            e.0 += f;
            e.1 += b;
        }
        self.largest.merge(other.largest);
        self.archive.0 += other.archive.0;
        self.archive.1 += other.archive.1;
        self.archive.2.merge(other.archive.2);
        self.junk.0 += other.junk.0;
        self.junk.1 += other.junk.1;
        self.junk.2.merge(other.junk.2);
        self.oldest = match (self.oldest, other.oldest) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.newest = self.newest.max(other.newest);
    }
}

fn walk(root: &Path, start: &Path, opts: &ScanOptions, now: u64, progress: &ScanProgress, recurse: bool) -> Partial {
    let mut p = Partial::new(opts.top_n);
    let archive_cutoff = now.saturating_sub(opts.archive_age_days * 86_400);
    let walker = walkdir::WalkDir::new(start).follow_links(false).max_depth(if recurse { usize::MAX } else { 1 });
    for entry in walker {
        if progress.cancel.load(Ordering::Relaxed) {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                p.unreadable += 1;
                continue;
            }
        };
        if entry.file_type().is_dir() {
            if entry.depth() > 0 {
                p.folders += 1;
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            p.unreadable += 1;
            continue;
        };
        let size = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let name_lower = entry.file_name().to_string_lossy().to_lowercase();
        let ext = Path::new(&name_lower)
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        p.files += 1;
        p.bytes += size;
        progress.files.fetch_add(1, Ordering::Relaxed);
        progress.bytes.fetch_add(size, Ordering::Relaxed);
        let c = p.categories.entry(category(&ext)).or_default();
        c.0 += 1;
        c.1 += size;
        let e = p.extensions.entry(if ext.is_empty() { "(none)".into() } else { format!(".{ext}") }).or_default();
        e.0 += 1;
        e.1 += size;
        if modified > 0 {
            p.oldest = Some(p.oldest.map_or(modified, |o| o.min(modified)));
            p.newest = Some(p.newest.map_or(modified, |n| n.max(modified)));
        }
        let item = FileItem { bytes: size, path: rel, modified };
        if is_junk(&name_lower, &ext) {
            p.junk.0 += 1;
            p.junk.1 += size;
            p.junk.2.push(item.clone());
        }
        if modified > 0 && modified < archive_cutoff && size >= opts.archive_min_bytes {
            p.archive.0 += 1;
            p.archive.1 += size;
            p.archive.2.push(item.clone());
        }
        p.largest.push(item);
    }
    p
}

fn sorted_buckets<K: ToString>(map: HashMap<K, (u64, u64)>, limit: usize) -> Vec<Bucket> {
    let mut items: Vec<Bucket> = map
        .into_iter()
        .map(|(name, (files, bytes))| Bucket { name: name.to_string(), files, bytes })
        .collect();
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| b.files.cmp(&a.files)));
    items.truncate(limit);
    items
}

pub fn scan_folder(path: &Path, opts: &ScanOptions, progress: &ScanProgress) -> Result<ScanReport> {
    if !path.is_dir() {
        bail!("{} is not a folder", path.display());
    }
    let started = Instant::now();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);

    let mut subdirs: Vec<PathBuf> = Vec::new();
    let mut unreadable_root = 0;
    match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    subdirs.push(entry.path());
                }
            }
        }
        Err(_) => unreadable_root += 1,
    }

    // Files directly in the folder, then each top-level subfolder in parallel.
    let mut total = walk(path, path, opts, now, progress, false);
    total.unreadable += unreadable_root;
    let per_folder: Vec<(String, Partial)> = subdirs
        .par_iter()
        .map(|dir| {
            let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            (name, walk(path, dir, opts, now, progress, true))
        })
        .collect();

    let mut top_folders = Vec::new();
    if total.files > 0 {
        top_folders.push(Bucket { name: "(files in this folder)".into(), files: total.files, bytes: total.bytes });
    }
    for (name, partial) in per_folder {
        top_folders.push(Bucket { name, files: partial.files, bytes: partial.bytes });
        total.merge(partial);
    }
    top_folders.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    top_folders.truncate(50);

    Ok(ScanReport {
        path: path.display().to_string(),
        files: total.files,
        folders: total.folders,
        bytes: total.bytes,
        unreadable: total.unreadable,
        categories: sorted_buckets(total.categories, 20),
        extensions: sorted_buckets(total.extensions, 15),
        top_folders,
        largest_files: total.largest.into_sorted(),
        archive_candidates: FileGroup { files: total.archive.0, bytes: total.archive.1, examples: total.archive.2.into_sorted() },
        junk: FileGroup { files: total.junk.0, bytes: total.junk.1, examples: total.junk.2.into_sorted() },
        oldest_modified: total.oldest,
        newest_modified: total.newest,
        elapsed_seconds: (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
        cancelled: progress.cancel.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write(dir: &Path, rel: &str, len: usize) -> PathBuf {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![1u8; len]).unwrap();
        path
    }

    #[test]
    fn summarizes_a_folder_tree() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "readme.txt", 10);
        write(dir.path(), "photos/a.jpg", 1000);
        write(dir.path(), "photos/b.JPG", 3000);
        write(dir.path(), "photos/Thumbs.db", 5);
        write(dir.path(), "video/deep/clip.mp4", 5000);
        write(dir.path(), "video/deep/clip.mp4.tmp", 7);
        let old = write(dir.path(), "data/old.nc", 2000);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(400 * 86_400))
            .unwrap();

        let opts = ScanOptions { top_n: 3, archive_age_days: 365, archive_min_bytes: 1000 };
        let progress = ScanProgress::default();
        let report = scan_folder(dir.path(), &opts, &progress).unwrap();

        assert_eq!(report.files, 7);
        assert_eq!(report.bytes, 10 + 1000 + 3000 + 5 + 5000 + 7 + 2000);
        assert_eq!(report.folders, 4); // photos, video, video/deep, data
        assert_eq!(progress.files.load(Ordering::Relaxed), 7);

        let images = report.categories.iter().find(|c| c.name == "Images").unwrap();
        assert_eq!((images.files, images.bytes), (2, 4000));
        assert_eq!(report.top_folders[0].name, "video");
        assert_eq!(report.top_folders.iter().find(|f| f.name == "(files in this folder)").unwrap().files, 1);

        let largest: Vec<_> = report.largest_files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(largest, ["video/deep/clip.mp4", "photos/b.JPG", "data/old.nc"]);

        assert_eq!((report.junk.files, report.junk.bytes), (2, 12));
        assert_eq!(report.archive_candidates.files, 1);
        assert_eq!(report.archive_candidates.examples[0].path, "data/old.nc");
        assert!(!report.cancelled);
    }

    #[test]
    fn rejects_missing_folders() {
        assert!(scan_folder(Path::new("/definitely/not/here"), &ScanOptions::default(), &ScanProgress::default()).is_err());
    }
}
