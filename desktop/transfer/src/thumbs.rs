//! Thumbnails of images in a bucket, for the cloud browser's optional thumbnail view.
//!
//! Careful with downloads and disk:
//! - JPEGs first try the preview the camera embedded in the EXIF block, from a small range read
//!   of the file's start (about 128 KiB instead of a whole photo).
//! - Otherwise the image is downloaded only if it's under a size cap, and decoded with limits
//!   (dimensions and memory), so a huge or hostile file can't exhaust memory.
//! - Nearline/Coldline/Archive objects are skipped unless asked for (they charge retrieval).
//! - Results go to a size-limited cache folder, keyed by bucket, name and generation (a
//!   replaced object gets a new thumbnail). Files that aren't usable images get a small marker
//!   so they aren't downloaded again. The oldest thumbnails are removed when the cache is full.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use serde::Serialize;

use crate::gcs::GcsClient;

/// Long edge of a thumbnail, in pixels.
pub const EDGE: u32 = 256;
/// Bytes read from the start of a JPEG to look for its embedded preview.
const EXIF_PROBE: u64 = 128 * 1024;
/// Embedded previews smaller than this (long edge) look too blurry; use the full image instead.
const MIN_EMBEDDED_EDGE: u32 = 120;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "jpe", "png", "gif", "webp", "bmp", "tif", "tiff"];

/// The object to make a thumbnail of (as listed by the browser).
#[derive(Debug, Clone, Default)]
pub struct ObjectRef {
    pub bucket: String,
    pub name: String,
    pub generation: Option<String>,
    pub size: u64,
    pub content_type: Option<String>,
    pub storage_class: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ThumbOptions {
    /// Don't download images bigger than this (the embedded-preview read still happens).
    pub max_bytes: u64,
    /// Preview Nearline/Coldline/Archive objects too (retrieval is charged).
    pub allow_cold: bool,
}

impl Default for ThumbOptions {
    fn default() -> Self {
        Self { max_bytes: 20 * 1024 * 1024, allow_cold: false }
    }
}

/// Why there's no thumbnail.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "skip", rename_all = "snake_case")]
pub enum Skip {
    NotImage,
    /// Nearline/Coldline/Archive: a click can ask for it anyway.
    Cold { storage_class: String },
    /// Bigger than the cap: a click can ask for it anyway.
    TooLarge { size: u64 },
    /// Couldn't be decoded (corrupt, unsupported variant, too big when decoded).
    Unreadable { message: String },
    /// Download failed (network, permission); not remembered, so it's tried again later.
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Cache,
    /// The preview embedded in the JPEG (small range read).
    Embedded,
    /// The whole image was downloaded and scaled down.
    Full,
}

#[derive(Debug, Clone)]
pub struct Thumb {
    /// JPEG bytes.
    pub jpeg: Vec<u8>,
    pub source: Source,
}

pub fn is_image(name: &str, content_type: Option<&str>) -> bool {
    if content_type.is_some_and(|t| t.starts_with("image/") && !t.contains("svg") && !t.contains("heic") && !t.contains("heif")) {
        return true;
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    IMAGE_EXTENSIONS.contains(&ext.as_str())
}

pub fn is_cold(storage_class: Option<&str>) -> bool {
    matches!(storage_class.map(|c| c.to_ascii_uppercase()).as_deref(), Some("NEARLINE" | "COLDLINE" | "ARCHIVE"))
}

fn is_jpeg(name: &str, content_type: Option<&str>) -> bool {
    content_type.is_some_and(|t| t == "image/jpeg" || t == "image/jpg")
        || matches!(name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref(), Some("jpg" | "jpeg" | "jpe"))
}

/// Decoding limits: no thumbnail is worth more than this to decode.
fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits
}

/// Decode `data`, turn it upright (EXIF orientation), scale it to fit `edge` and encode as JPEG.
pub fn make_thumbnail(data: &[u8], edge: u32) -> Result<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    reader.limits(limits());
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder)?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    encode(image, edge)
}

fn encode(image: DynamicImage, edge: u32) -> Result<Vec<u8>> {
    let image = if image.width() > edge || image.height() > edge { image.thumbnail(edge, edge) } else { image };
    // JPEG has no transparency: put transparent images on white.
    let rgb = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
        for (out, px) in rgb.pixels_mut().zip(rgba.pixels()) {
            let a = px[3] as u32;
            for c in 0..3 {
                out[c] = ((px[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
        }
        rgb
    } else {
        image.to_rgb8()
    };
    let mut out = Vec::with_capacity(16 * 1024);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&rgb)?;
    Ok(out)
}

/// The preview a camera embedded in a JPEG's EXIF block, made upright and re-encoded, if the
/// start of the file holds one big enough to look decent.
pub fn embedded_preview(start_of_file: &[u8], edge: u32) -> Option<Vec<u8>> {
    let exif = exif::Reader::new().read_from_container(&mut Cursor::new(start_of_file)).ok()?;
    let field = |tag| exif.get_field(tag, exif::In::THUMBNAIL).and_then(|f| f.value.get_uint(0));
    let offset = field(exif::Tag::JPEGInterchangeFormat)? as usize;
    let length = field(exif::Tag::JPEGInterchangeFormatLength)? as usize;
    let preview = exif.buf().get(offset..offset.checked_add(length)?)?;
    let mut reader = ImageReader::with_format(Cursor::new(preview), image::ImageFormat::Jpeg);
    reader.limits(limits());
    let mut image = reader.decode().ok()?;
    if image.width().max(image.height()) < MIN_EMBEDDED_EDGE {
        return None;
    }
    // The preview is stored like the main image: apply the main image's orientation.
    let orientation = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0));
    if let Some(o) = orientation.and_then(|o| image::metadata::Orientation::from_exif(o as u8)) {
        image.apply_orientation(o);
    }
    encode(image, edge).ok()
}

/// Make a thumbnail of `object` (no cache): embedded preview first for JPEGs, else the whole
/// image if it's under the cap.
pub async fn fetch(client: &GcsClient, object: &ObjectRef, opts: &ThumbOptions) -> std::result::Result<Thumb, Skip> {
    if !is_image(&object.name, object.content_type.as_deref()) {
        return Err(Skip::NotImage);
    }
    if !opts.allow_cold && is_cold(object.storage_class.as_deref()) {
        return Err(Skip::Cold { storage_class: object.storage_class.clone().unwrap_or_default() });
    }
    let generation = object.generation.as_deref();
    if is_jpeg(&object.name, object.content_type.as_deref()) && object.size > EXIF_PROBE {
        let start = client
            .download_range(&object.bucket, &object.name, generation, 0, EXIF_PROBE - 1)
            .await
            .map_err(|e| Skip::Error { message: e.to_string() })?;
        if let Some(jpeg) = tokio::task::spawn_blocking(move || embedded_preview(&start, EDGE)).await.ok().flatten() {
            return Ok(Thumb { jpeg, source: Source::Embedded });
        }
    }
    if object.size > opts.max_bytes {
        return Err(Skip::TooLarge { size: object.size });
    }
    let response = client.download(&object.bucket, &object.name, generation).await.map_err(|e| Skip::Error { message: e.to_string() })?;
    let data = response.bytes().await.map_err(|e| Skip::Error { message: format!("network error: {e}") })?;
    if data.len() as u64 > opts.max_bytes {
        return Err(Skip::TooLarge { size: data.len() as u64 });
    }
    match tokio::task::spawn_blocking(move || make_thumbnail(&data, EDGE)).await {
        Ok(Ok(jpeg)) => Ok(Thumb { jpeg, source: Source::Full }),
        Ok(Err(err)) => Err(Skip::Unreadable { message: format!("{err:#}") }),
        Err(err) => Err(Skip::Unreadable { message: err.to_string() }),
    }
}

/// Size-limited thumbnail cache folder.
pub struct ThumbCache {
    dir: PathBuf,
    max_bytes: std::sync::atomic::AtomicU64,
    writes: AtomicU32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheInfo {
    pub dir: String,
    pub bytes: u64,
    pub files: u64,
    pub max_bytes: u64,
}

/// Prune every this many writes (and when the cache is opened).
const PRUNE_EVERY: u32 = 50;

impl ThumbCache {
    pub fn open(dir: impl Into<PathBuf>, max_bytes: u64) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let cache = Self { dir, max_bytes: max_bytes.into(), writes: AtomicU32::new(0) };
        cache.prune();
        Ok(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    pub fn set_max_bytes(&self, max_bytes: u64) {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
        self.prune();
    }

    /// Bucket, name and generation: a replaced object gets a new key.
    pub fn key(object: &ObjectRef) -> String {
        let text = format!("{}\0{}\0{}\0{}", object.bucket, object.name, object.generation.as_deref().unwrap_or(""), object.size);
        // FNV-1a (64-bit) plus crc32c: stable across builds, and collisions are vanishingly rare.
        let mut fnv: u64 = 0xcbf29ce484222325;
        for byte in text.bytes() {
            fnv ^= byte as u64;
            fnv = fnv.wrapping_mul(0x100000001b3);
        }
        format!("{fnv:016x}{:08x}", crc32c::crc32c(text.as_bytes()))
    }

    fn path(&self, key: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{key}.{ext}"))
    }

    /// A cached thumbnail (Ok) or remembered "not a usable image" (Err), refreshing its age.
    pub fn get(&self, key: &str) -> Option<std::result::Result<Vec<u8>, Skip>> {
        let jpg = self.path(key, "jpg");
        if let Ok(bytes) = std::fs::read(&jpg) {
            touch(&jpg);
            return Some(Ok(bytes));
        }
        let skip = self.path(key, "skip");
        let reason = std::fs::read_to_string(&skip).ok()?;
        touch(&skip);
        Some(Err(serde_json::from_str(&reason).unwrap_or(Skip::NotImage)))
    }

    pub fn put(&self, key: &str, jpeg: &[u8]) {
        let _ = write_atomic(&self.path(key, "jpg"), jpeg);
        self.wrote();
    }

    /// Remember that this object isn't a usable image (so it isn't downloaded again).
    pub fn put_skip(&self, key: &str, skip: &Skip) {
        if let Ok(text) = serde_json::to_string(skip) {
            let _ = write_atomic(&self.path(key, "skip"), text.as_bytes());
            self.wrote();
        }
    }

    fn wrote(&self) {
        if self.writes.fetch_add(1, Ordering::Relaxed) % PRUNE_EVERY == PRUNE_EVERY - 1 {
            self.prune();
        }
    }

    fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(read) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        read.filter_map(|e| e.ok())
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                meta.is_file().then(|| (e.path(), meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
            })
            .collect()
    }

    /// If over the limit, delete the least recently used files until under 80% of it.
    pub fn prune(&self) {
        let max = self.max_bytes();
        let mut entries = self.entries();
        let mut total: u64 = entries.iter().map(|(_, size, _)| size).sum();
        if total <= max {
            return;
        }
        entries.sort_by_key(|(_, _, time)| *time);
        let target = max / 10 * 8;
        for (path, size, _) in entries {
            if total <= target {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    pub fn info(&self) -> CacheInfo {
        let entries = self.entries();
        CacheInfo {
            dir: self.dir.display().to_string(),
            bytes: entries.iter().map(|(_, size, _)| size).sum(),
            files: entries.iter().filter(|(path, _, _)| path.extension().is_some_and(|e| e == "jpg")).count() as u64,
            max_bytes: self.max_bytes(),
        }
    }

    pub fn clear(&self) -> Result<()> {
        for (path, _, _) in self.entries() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn touch(path: &Path) {
    if let Ok(file) = std::fs::File::options().append(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).map_err(|e| anyhow!("{e}"))
}

/// A thumbnail through the cache: a cached one, or fetched and stored. Unusable images are
/// remembered; too-large, cold-storage and failed downloads aren't (a click or a later try can
/// still get them).
pub async fn thumbnail(client: &GcsClient, cache: &ThumbCache, object: &ObjectRef, opts: &ThumbOptions) -> std::result::Result<Thumb, Skip> {
    let key = ThumbCache::key(object);
    match cache.get(&key) {
        Some(Ok(jpeg)) => return Ok(Thumb { jpeg, source: Source::Cache }),
        Some(Err(skip)) => return Err(skip),
        None => {}
    }
    let result = fetch(client, object, opts).await;
    match &result {
        Ok(thumb) => cache.put(&key, &thumb.jpeg),
        Err(skip @ (Skip::NotImage | Skip::Unreadable { .. })) => cache.put_skip(&key, skip),
        Err(_) => {}
    }
    result
}


/// Test images (JPEGs with and without an embedded EXIF preview), for this crate's tests.
#[doc(hidden)]
pub mod test_images {
    pub fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 40]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img).unwrap();
        out
    }

    /// A JPEG with an EXIF block holding orientation and an embedded preview.
    pub fn jpeg_with_exif(w: u32, h: u32, preview: &[u8], orientation: u16) -> Vec<u8> {
        // TIFF (little endian): IFD0 with Orientation, IFD1 with the preview's offset/length.
        let mut tiff: Vec<u8> = b"II*\0".to_vec();
        tiff.extend(8u32.to_le_bytes());
        // IFD0: 1 entry, then next-IFD offset.
        let ifd1_offset = 8 + 2 + 12 + 4;
        tiff.extend(1u16.to_le_bytes());
        tiff.extend(0x0112u16.to_le_bytes());
        tiff.extend(3u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend((orientation as u32).to_le_bytes());
        tiff.extend((ifd1_offset as u32).to_le_bytes());
        // IFD1: 2 entries, then 0.
        let data_offset = ifd1_offset + 2 + 12 * 2 + 4;
        tiff.extend(2u16.to_le_bytes());
        tiff.extend(0x0201u16.to_le_bytes());
        tiff.extend(4u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend((data_offset as u32).to_le_bytes());
        tiff.extend(0x0202u16.to_le_bytes());
        tiff.extend(4u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend((preview.len() as u32).to_le_bytes());
        tiff.extend(0u32.to_le_bytes());
        tiff.extend(preview);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(&tiff);
        let main = jpeg(w, h);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend(((app1.len() + 2) as u16).to_be_bytes());
        out.extend(&app1);
        out.extend(&main[2..]); // the main image without its own SOI
        out
    }

}

#[cfg(test)]
mod tests {
    use super::test_images::{jpeg, jpeg_with_exif};
    use super::*;

    fn dims(jpeg: &[u8]) -> (u32, u32) {
        let img = image::load_from_memory(jpeg).unwrap();
        (img.width(), img.height())
    }


    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, if x < w / 2 { 255 } else { 0 }]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn images_become_small_jpegs() {
        let thumb = make_thumbnail(&png(1200, 600), EDGE).unwrap();
        assert_eq!(dims(&thumb), (256, 128));
        let thumb = make_thumbnail(&jpeg(300, 900), EDGE).unwrap();
        assert_eq!(dims(&thumb), (85, 256));
        let small = make_thumbnail(&png(40, 30), EDGE).unwrap();
        assert_eq!(dims(&small), (40, 30), "small images aren't enlarged");
        assert!(make_thumbnail(b"not an image", EDGE).is_err());
    }

    #[test]
    fn embedded_previews_are_used_and_turned_upright() {
        let preview = jpeg(160, 120);
        // Orientation 6: the camera was turned; the picture must be rotated 90 degrees.
        let file = jpeg_with_exif(4000, 3000, &preview, 6);
        let thumb = embedded_preview(&file[..file.len().min(EXIF_PROBE as usize)], EDGE).expect("preview found");
        assert_eq!(dims(&thumb), (120, 160));
        // The full decode reads the same orientation.
        assert_eq!(dims(&make_thumbnail(&file, EDGE).unwrap()), (192, 256));
        // A tiny embedded preview isn't good enough.
        assert!(embedded_preview(&jpeg_with_exif(400, 300, &jpeg(80, 60), 1), EDGE).is_none());
        assert!(embedded_preview(&jpeg(400, 300), EDGE).is_none(), "no EXIF");
    }

    #[test]
    fn huge_decoded_sizes_are_refused() {
        // A PNG header claiming 30000 x 30000 pixels.
        let mut png = png(10, 10);
        png[16..20].copy_from_slice(&30000u32.to_be_bytes());
        png[20..24].copy_from_slice(&30000u32.to_be_bytes());
        assert!(make_thumbnail(&png, EDGE).is_err());
    }

    #[test]
    fn what_counts_as_an_image() {
        assert!(is_image("a/b/photo.JPG", None));
        assert!(is_image("x", Some("image/png")));
        assert!(!is_image("vector.svg", Some("image/svg+xml")));
        assert!(!is_image("data.csv", Some("text/csv")));
        assert!(is_cold(Some("COLDLINE")) && is_cold(Some("archive")) && !is_cold(Some("STANDARD")) && !is_cold(None));
    }

    #[test]
    fn cache_keys_hits_markers_and_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbCache::open(dir.path(), 10_000).unwrap();
        let a = ObjectRef { bucket: "b".into(), name: "a.jpg".into(), generation: Some("1".into()), size: 5, ..Default::default() };
        let a2 = ObjectRef { generation: Some("2".into()), ..a.clone() };
        assert_ne!(ThumbCache::key(&a), ThumbCache::key(&a2), "a replaced object gets a new key");
        assert_eq!(ThumbCache::key(&a), ThumbCache::key(&a.clone()));

        let key = ThumbCache::key(&a);
        assert!(cache.get(&key).is_none());
        cache.put(&key, b"jpeg bytes");
        assert_eq!(cache.get(&key), Some(Ok(b"jpeg bytes".to_vec())));
        cache.put_skip("other", &Skip::Unreadable { message: "bad".into() });
        assert!(matches!(cache.get("other"), Some(Err(Skip::Unreadable { .. }))));

        // Fill past the limit: the oldest go first, down to 80%.
        for i in 0..20 {
            cache.put(&format!("k{i:02}"), &[0u8; 1000]);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        cache.prune();
        let info = cache.info();
        assert!(info.bytes <= 8_000, "{info:?}");
        assert!(cache.get("k19").is_some(), "newest kept");
        assert!(cache.get("k00").is_none(), "oldest removed");
        cache.clear().unwrap();
        assert_eq!(cache.info().bytes, 0);
    }
}
