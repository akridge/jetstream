//! Cloud-backed placeholder files on Windows (OneDrive "files on demand", and similar sync
//! clients). Their content isn't on the disk: reading one makes Windows download it first,
//! so an upload sits apparently frozen. They're skipped with a clear explanation instead.

/// FILE_ATTRIBUTE_OFFLINE
const OFFLINE: u32 = 0x0000_1000;
/// FILE_ATTRIBUTE_RECALL_ON_OPEN
const RECALL_ON_OPEN: u32 = 0x0004_0000;
/// FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

pub const ONLINE_ONLY_REASON: &str =
    "online-only file (OneDrive or another cloud drive): its content isn't on this computer. Right-click it and choose \"Always keep on this device\", then sync again";

/// Whether Windows file attributes mark a placeholder whose content must be downloaded first.
pub fn is_online_only_attributes(attributes: u32) -> bool {
    attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

/// Whether a file is an online-only placeholder (always false outside Windows).
#[cfg(windows)]
pub fn is_online_only(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    is_online_only_attributes(meta.file_attributes())
}

#[cfg(not(windows))]
pub fn is_online_only(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_placeholder_attributes() {
        assert!(is_online_only_attributes(RECALL_ON_DATA_ACCESS | 0x20)); // + ARCHIVE
        assert!(is_online_only_attributes(RECALL_ON_OPEN));
        assert!(is_online_only_attributes(OFFLINE));
        assert!(!is_online_only_attributes(0x20)); // ARCHIVE only: a normal file
        assert!(!is_online_only_attributes(0x0008_0000)); // PINNED ("always keep on this device")
    }
}
