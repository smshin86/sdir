//! Compact private worker protocol. The request already owns the parent directory.
use crate::model::{CloudState, FileEntry, NativePath};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Name {
    Text(String),
    Wide(Vec<u16>),
}
impl Name {
    fn new(units: Vec<u16>) -> Self {
        match String::from_utf16(&units) {
            Ok(s) => Self::Text(s),
            Err(_) => Self::Wide(units),
        }
    }
}

// Tuple serialization avoids repeating field names as well as the parent path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListingEntry(Name, u8, Option<u64>, Option<u64>);
impl From<FileEntry> for ListingEntry {
    fn from(entry: FileEntry) -> Self {
        let path = entry.path.to_path();
        let name = NativePath::new(path.file_name().expect("directory entry has a filename"));
        let flags = u8::from(entry.is_dir)
            | (u8::from(entry.is_link) << 1)
            | (u8::from(entry.hidden) << 2)
            | (match entry.cloud {
                CloudState::None => 0,
                CloudState::Remote => 1,
                CloudState::Available => 2,
                CloudState::Pinned => 3,
            } << 3);
        Self(Name::new(name.0), flags, entry.bytes, entry.modified)
    }
}
impl ListingEntry {
    pub fn expand(self, parent: &Path) -> FileEntry {
        let (path, name) = match self.0 {
            Name::Text(name) => (NativePath::new(parent.join(&name)), name),
            Name::Wide(units) => {
                let name = String::from_utf16_lossy(&units);
                (
                    NativePath::new(parent.join(NativePath(units).to_path())),
                    name,
                )
            }
        };
        FileEntry {
            path,
            name,
            is_dir: self.1 & 1 != 0,
            is_link: self.1 & 2 != 0,
            hidden: self.1 & 4 != 0,
            cloud: match (self.1 >> 3) & 3 {
                1 => CloudState::Remote,
                2 => CloudState::Available,
                3 => CloudState::Pinned,
                _ => CloudState::None,
            },
            bytes: self.2,
            modified: self.3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_round_trip_keeps_paths_and_all_metadata() {
        let parent = Path::new("long-parent-folder/another-long-parent-folder/working-folder");
        for cloud in [
            CloudState::None,
            CloudState::Remote,
            CloudState::Available,
            CloudState::Pinned,
        ] {
            let original = FileEntry {
                path: NativePath::new(parent.join("보고서-🙂.txt")),
                name: "보고서-🙂.txt".into(),
                is_dir: false,
                is_link: true,
                hidden: true,
                bytes: Some(123456789),
                modified: Some(12345678),
                cloud,
            };
            let old = serde_json::to_vec(&original).unwrap();
            let bytes = serde_json::to_vec(&ListingEntry::from(original.clone())).unwrap();
            assert!(
                bytes.len() * 4 < old.len(),
                "parent must not be repeated on the wire"
            );
            let decoded: ListingEntry = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(decoded.expand(parent), original);
        }
    }
    #[test]
    fn unpaired_utf16_units_are_not_replaced_on_the_wire() {
        let units = vec![b'a' as u16, 0xd800, b'.' as u16, b'x' as u16];
        let encoded = serde_json::to_vec(&Name::new(units.clone())).unwrap();
        let decoded: Name = serde_json::from_slice(&encoded).unwrap();
        match decoded {
            Name::Wide(v) => assert_eq!(v, units),
            _ => panic!("lossless wide name required"),
        }
    }
    #[cfg(windows)]
    #[test]
    fn windows_odd_filename_and_directory_metadata_expand_losslessly() {
        let parent = Path::new(r"C:\parent\");
        let name = NativePath(vec![b'a' as u16, 0xd800]);
        let original = FileEntry {
            path: NativePath::new(parent.join(name.to_path())),
            name: name.display(),
            is_dir: true,
            is_link: true,
            hidden: false,
            bytes: None,
            modified: None,
            cloud: CloudState::Remote,
        };
        let bytes = serde_json::to_vec(&ListingEntry::from(original.clone())).unwrap();
        let decoded: ListingEntry = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.expand(parent), original);
    }
}
