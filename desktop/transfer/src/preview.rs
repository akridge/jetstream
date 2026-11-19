//! The cloud browser's preview pane: the start of a text file (CSV, JSON, logs, ...), read
//! with one range request so a 10 GB CSV costs 64 KB.

use serde::Serialize;

use crate::gcs::GcsClient;
use crate::thumbs::is_cold;

/// How much of a text file the pane shows.
pub const TEXT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TextPreview {
    Text {
        text: String,
        /// Bytes shown, and the file's size.
        shown: u64,
        size: u64,
        truncated: bool,
    },
    /// Not text (NUL bytes or mostly invalid UTF-8 near the start).
    Binary,
    /// Nearline/Coldline/Archive: reading is charged, so only on request.
    Cold { storage_class: String },
    Error { message: String },
}

/// Turn the first bytes of a file into text: None if it looks binary. A character cut off at
/// the end is dropped; a cut-off line is kept (the pane says the file goes on).
pub fn decode(bytes: &[u8]) -> Option<String> {
    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0) {
        return None;
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes); // UTF-8 byte order mark
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_string()),
        // Only the last character was cut off by the range read.
        Err(err) if err.error_len().is_none() => Some(String::from_utf8_lossy(&bytes[..err.valid_up_to()]).into_owned()),
        Err(_) => {
            // Some other encoding (Latin-1 logs, Windows-1252 CSVs): show it if most of it reads.
            let lossy = String::from_utf8_lossy(bytes);
            let bad = lossy.chars().filter(|&c| c == char::REPLACEMENT_CHARACTER).count();
            (bad * 20 < lossy.chars().count().max(1)).then(|| lossy.into_owned())
        }
    }
}

/// The start of gs://bucket/name (at most TEXT_BYTES).
pub async fn text(client: &GcsClient, bucket: &str, name: &str, generation: Option<&str>, size: u64, storage_class: Option<&str>, allow_cold: bool) -> TextPreview {
    if !allow_cold && is_cold(storage_class) {
        return TextPreview::Cold { storage_class: storage_class.unwrap_or_default().to_string() };
    }
    if size == 0 {
        return TextPreview::Text { text: String::new(), shown: 0, size, truncated: false };
    }
    let end = size.min(TEXT_BYTES) - 1;
    match client.download_range(bucket, name, generation, 0, end).await {
        Ok(bytes) => match decode(&bytes) {
            Some(text) => TextPreview::Text { shown: bytes.len() as u64, truncated: (bytes.len() as u64) < size, text, size },
            None => TextPreview::Binary,
        },
        Err(err) => TextPreview::Error { message: err.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_and_spots_binary() {
        assert_eq!(decode(b"depth,temp\n1,20\n").as_deref(), Some("depth,temp\n1,20\n"));
        assert_eq!(decode(b"\xEF\xBB\xBFa,b\n").as_deref(), Some("a,b\n"), "byte order mark dropped");
        // "é" cut in half by the range read: the half is dropped, nothing else.
        assert_eq!(decode("caf\u{e9}".as_bytes().split_last().unwrap().1).as_deref(), Some("caf"));
        assert_eq!(decode(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), None);
        // Latin-1 text with an accent: shown, with one replacement character.
        let latin1 = b"station,name\n1,Mont\xe9rey Bay\n2,Hilo\n3,Kona\n4,Pago Pago\n";
        assert!(decode(latin1).unwrap().contains("Hilo"));
        // Random bytes without NULs: binary.
        let noise: Vec<u8> = (0..4000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8 | 0x80).collect();
        assert_eq!(decode(&noise), None);
    }
}
