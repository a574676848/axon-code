use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

use sha2::{Digest, Sha256};

use super::model::{
    IndexKey, IndexSnapshot, SEARCH_INDEX_DIRECTORY, SEARCH_INDEX_FILE, validate_snapshot,
};

/// 校验和头部与 JSON payload 之间的分隔字节。
const STORED_INDEX_CHECKSUM_SEPARATOR: u8 = b'\n';
/// 存储头部使用十六进制 SHA-256。
const STORED_INDEX_CHECKSUM_HEX_LEN: usize = 64;

pub(crate) fn load(key: &IndexKey) -> Option<IndexSnapshot> {
    let path = index_path(key)?;
    let bytes = fs::read(path).ok()?;
    decode(&bytes, key)
}

fn decode(bytes: &[u8], key: &IndexKey) -> Option<IndexSnapshot> {
    let separator = bytes
        .iter()
        .position(|byte| *byte == STORED_INDEX_CHECKSUM_SEPARATOR)?;
    let (header, rest) = bytes.split_at(separator);
    let payload = rest.get(1..)?;
    if header.len() != STORED_INDEX_CHECKSUM_HEX_LEN || checksum(payload).as_bytes() != header {
        return None;
    }
    let snapshot = serde_json::from_slice::<IndexSnapshot>(payload).ok()?;
    let mut snapshot = snapshot;
    snapshot.rebuild_paths();
    if !validate_snapshot(&snapshot, key) {
        return None;
    }
    Some(snapshot)
}

pub(crate) fn persist(snapshot: &IndexSnapshot, key: &IndexKey) -> io::Result<()> {
    let path = index_path(key)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法解析 Axon 运行态目录"))?;
    let payload = serde_json::to_vec(snapshot)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let header = checksum(&payload);

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "索引路径缺少父目录"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_file_name(format!("{SEARCH_INDEX_FILE}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(header.as_bytes())?;
    file.write_all(&[STORED_INDEX_CHECKSUM_SEPARATOR])?;
    file.write_all(&payload)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);

    replace_file(&temporary, &path)
}

fn replace_file(temporary: &PathBuf, target: &PathBuf) -> io::Result<()> {
    if target.exists() {
        fs::remove_file(target)?;
    }
    fs::rename(temporary, target)
}

fn index_path(key: &IndexKey) -> Option<PathBuf> {
    let config_path = axon_config::ConfigLoader::default_user_config_path().ok()?;
    let runtime_root = axon_config::runtime_axon_root_path(&config_path).ok()?;
    Some(
        runtime_root
            .join(SEARCH_INDEX_DIRECTORY)
            .join(key_directory_name(key))
            .join(SEARCH_INDEX_FILE),
    )
}

fn key_directory_name(key: &IndexKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.root.to_string_lossy().as_bytes());
    hasher.update([u8::from(key.hidden), u8::from(key.gitignore)]);
    hex::encode(hasher.finalize())
}

fn checksum(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{STORED_INDEX_CHECKSUM_SEPARATOR, checksum, decode};
    use crate::search_index::model::{
        FileFingerprint, INDEX_FORMAT_VERSION, IndexKey, IndexSnapshot, IndexedFile,
        SEARCH_INDEX_SCHEMA_VERSION, TrigramKey,
    };

    fn snapshot(root: &std::path::Path) -> IndexSnapshot {
        IndexSnapshot {
            format_version: INDEX_FORMAT_VERSION,
            schema_version: SEARCH_INDEX_SCHEMA_VERSION,
            root: root.to_path_buf(),
            hidden: false,
            gitignore: true,
            generation: 1,
            files: Default::default(),
            postings: Default::default(),
            paths: Default::default(),
        }
    }

    fn encoded(snapshot: &IndexSnapshot) -> Vec<u8> {
        let payload = serde_json::to_vec(snapshot).unwrap();
        let mut bytes = checksum(&payload).into_bytes();
        bytes.push(STORED_INDEX_CHECKSUM_SEPARATOR);
        bytes.extend_from_slice(&payload);
        bytes
    }

    #[test]
    fn corrupted_checksum_is_rejected() {
        let root = std::path::Path::new("E:/axon-search-index-test");
        let snapshot = snapshot(root);
        let mut bytes = encoded(&snapshot);
        bytes[0] = if bytes[0] == b'0' { b'1' } else { b'0' };

        assert!(decode(&bytes, &IndexKey::new(root, false, true)).is_none());
    }

    #[test]
    fn valid_checksum_is_accepted() {
        let root = std::path::Path::new("E:/axon-search-index-test");
        let snapshot = snapshot(root);

        assert!(decode(&encoded(&snapshot), &IndexKey::new(root, false, true)).is_some());
    }

    #[test]
    fn truncated_payload_is_rejected() {
        let root = std::path::Path::new("E:/axon-search-index-test");
        let snapshot = snapshot(root);
        let bytes = encoded(&snapshot);
        let truncated = &bytes[..bytes.len() - 1];

        assert!(decode(truncated, &IndexKey::new(root, false, true)).is_none());
    }

    #[test]
    fn decoded_snapshot_rebuilds_derived_paths() {
        let root = std::path::Path::new("E:/axon-search-index-test");
        let mut snapshot = snapshot(root);
        snapshot.files.insert(
            "src/lib.rs".to_owned(),
            IndexedFile {
                id: 7,
                fingerprint: FileFingerprint {
                    length: 3,
                    modified_at_ms: 1,
                },
                searchable: true,
            },
        );
        snapshot.postings.insert(TrigramKey(0x010203), vec![7]);

        let decoded = decode(&encoded(&snapshot), &IndexKey::new(root, false, true)).unwrap();

        assert_eq!(decoded.path_of(7), Some("src/lib.rs"));
    }
}
