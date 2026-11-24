//! LSP documentSymbol 结果缓存。

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

use super::{ExtractedSymbol, LspDocumentSymbolQuery};

const DOCUMENT_SYMBOL_CACHE_MAX_ENTRIES: usize = 128;
const FILE_URI_PREFIX: &str = "file:///";

static DOCUMENT_SYMBOL_CACHE: LazyLock<Mutex<DocumentSymbolCacheState>> =
    LazyLock::new(|| Mutex::new(DocumentSymbolCacheState::default()));

#[derive(Default)]
struct DocumentSymbolCacheState {
    entries: HashMap<DocumentSymbolCacheKey, DocumentSymbolCacheEntry>,
    access_clock: u64,
}

struct DocumentSymbolCacheEntry {
    symbols: Vec<ExtractedSymbol>,
    last_access: u64,
}

impl DocumentSymbolCacheState {
    fn get(&mut self, key: &DocumentSymbolCacheKey) -> Option<Vec<ExtractedSymbol>> {
        let entry = self.entries.get_mut(key)?;
        self.access_clock = self.access_clock.saturating_add(1);
        entry.last_access = self.access_clock;
        Some(entry.symbols.clone())
    }

    fn insert(&mut self, key: DocumentSymbolCacheKey, symbols: &[ExtractedSymbol]) {
        self.access_clock = self.access_clock.saturating_add(1);
        if self.entries.len() >= DOCUMENT_SYMBOL_CACHE_MAX_ENTRIES
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            DocumentSymbolCacheEntry {
                symbols: symbols.to_vec(),
                last_access: self.access_clock,
            },
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DocumentSymbolCacheKey {
    workspace_root: String,
    text_document_uri: String,
    file_hash: u64,
}

pub(super) fn cached_document_symbols(
    query: &LspDocumentSymbolQuery,
) -> Option<Vec<ExtractedSymbol>> {
    let key = cache_key(query)?;
    DOCUMENT_SYMBOL_CACHE
        .lock()
        .ok()
        .and_then(|mut cache| cache.get(&key))
}

pub(super) fn store_document_symbols(query: &LspDocumentSymbolQuery, symbols: &[ExtractedSymbol]) {
    let Some(key) = cache_key(query) else {
        return;
    };
    let Ok(mut cache) = DOCUMENT_SYMBOL_CACHE.lock() else {
        return;
    };
    cache.insert(key, symbols);
}

fn cache_key(query: &LspDocumentSymbolQuery) -> Option<DocumentSymbolCacheKey> {
    let file_path = file_uri_to_path(&query.text_document_uri)?;
    let bytes = std::fs::read(file_path).ok()?;
    Some(DocumentSymbolCacheKey {
        workspace_root: query.workspace_root.clone(),
        text_document_uri: query.text_document_uri.clone(),
        file_hash: content_hash(&bytes),
    })
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix(FILE_URI_PREFIX)?;
    #[cfg(windows)]
    {
        Some(PathBuf::from(path.replace('/', "\\")))
    }
    #[cfg(not(windows))]
    {
        let normalized = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        Some(PathBuf::from(normalized))
    }
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::file_uri_to_path;
    use std::path::PathBuf;

    #[cfg(not(windows))]
    #[test]
    fn file_uri_to_path_preserves_unix_absolute_path() {
        assert_eq!(
            file_uri_to_path("file:///Users/alex/project/src/lib.rs"),
            Some(PathBuf::from("/Users/alex/project/src/lib.rs"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn file_uri_to_path_handles_extra_slash_from_absolute_path_formatting() {
        assert_eq!(
            file_uri_to_path("file:////tmp/axon/src/lib.rs"),
            Some(PathBuf::from("/tmp/axon/src/lib.rs"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn file_uri_to_path_converts_windows_drive_path() {
        assert_eq!(
            file_uri_to_path("file:///E:/repo/src/main.rs"),
            Some(PathBuf::from("E:\\repo\\src\\main.rs"))
        );
    }
}
