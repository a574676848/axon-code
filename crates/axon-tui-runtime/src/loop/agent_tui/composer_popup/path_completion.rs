//! Composer 路径补全后台扫描。

use std::{
    env,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};

use axon_fs_walk::{EntryKind, Walk, WalkAction, WalkDepth, WalkEntry, WalkOpts};

use super::candidate::{ComposerPopupItem, ComposerPopupKind, ComposerPopupMentionType};

const PATH_COMPLETION_ITEM_LIMIT: usize = 12;
const PATH_COMPLETION_SCAN_LIMIT: usize = 256;
const DIRECTORY_SCORE_BONUS: i32 = 8;
const EXACT_MATCH_SCORE: i32 = 1_000;
const PREFIX_MATCH_SCORE: i32 = 750;
const SUBSTRING_MATCH_SCORE: i32 = 500;
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathCompletionMode {
    File,
    Image,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathCompletionKey {
    mode: PathCompletionMode,
    cwd: PathBuf,
    input: String,
}

impl PathCompletionKey {
    pub(crate) fn from_popup(kind: ComposerPopupKind, input: &str, cwd: &str) -> Option<Self> {
        let mode = match kind {
            ComposerPopupKind::File => PathCompletionMode::File,
            ComposerPopupKind::Image => PathCompletionMode::Image,
            ComposerPopupKind::Path => PathCompletionMode::Path,
            ComposerPopupKind::Skill | ComposerPopupKind::Connector | ComposerPopupKind::Shell => {
                return None;
            }
        };
        Some(Self {
            mode,
            cwd: PathBuf::from(cwd),
            input: input.to_owned(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct PathCompletionState {
    next_request_id: u64,
    cached_key: Option<PathCompletionKey>,
    cached_items: Vec<ComposerPopupItem>,
    pending: Option<PendingPathCompletion>,
}

impl Clone for PathCompletionState {
    fn clone(&self) -> Self {
        Self {
            next_request_id: self.next_request_id,
            cached_key: self.cached_key.clone(),
            cached_items: self.cached_items.clone(),
            pending: None,
        }
    }
}

impl Default for PathCompletionState {
    fn default() -> Self {
        Self {
            next_request_id: 1,
            cached_key: None,
            cached_items: Vec::new(),
            pending: None,
        }
    }
}

impl PathCompletionState {
    pub(crate) fn request(&mut self, key: PathCompletionKey) {
        if self.cached_key.as_ref() == Some(&key)
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.key == key)
        {
            return;
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let (tx, rx) = mpsc::sync_channel(1);
        let scan_key = key.clone();
        let spawn_result = thread::Builder::new()
            .name("axon-path-completion".to_owned())
            .spawn(move || {
                let items = scan_path_completion(&scan_key);
                let _ = tx.try_send(PathCompletionResult {
                    request_id,
                    key: scan_key,
                    items,
                });
            });
        if spawn_result.is_ok() {
            self.pending = Some(PendingPathCompletion {
                request_id,
                key,
                rx,
            });
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        match pending.rx.try_recv() {
            Ok(result) if result.request_id == pending.request_id && result.key == pending.key => {
                self.cached_key = Some(result.key);
                self.cached_items = result.items;
                true
            }
            Ok(_) => false,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                false
            }
            Err(TryRecvError::Disconnected) => false,
        }
    }

    pub(crate) fn cached_items(&self, key: &PathCompletionKey) -> &[ComposerPopupItem] {
        if self.cached_key.as_ref() == Some(key) {
            self.cached_items.as_slice()
        } else {
            &[]
        }
    }

    pub(crate) fn clear(&mut self) {
        self.cached_key = None;
        self.cached_items.clear();
        self.pending = None;
    }
}

#[derive(Debug)]
struct PendingPathCompletion {
    request_id: u64,
    key: PathCompletionKey,
    rx: Receiver<PathCompletionResult>,
}

#[derive(Debug)]
struct PathCompletionResult {
    request_id: u64,
    key: PathCompletionKey,
    items: Vec<ComposerPopupItem>,
}

fn scan_path_completion(key: &PathCompletionKey) -> Vec<ComposerPopupItem> {
    let Some(query) = resolve_path_query(&key.input, &key.cwd, home_dir().as_deref()) else {
        return Vec::new();
    };
    let opts = WalkOpts {
        depth: WalkDepth::One,
        max_entries: Some(PATH_COMPLETION_SCAN_LIMIT),
        ..WalkOpts::default()
    };
    let mut items = Vec::new();
    let _ = Walk::run(&query.scan_dir, &opts, |entry| {
        if let Some(item) = path_item(key.mode, &query, entry) {
            items.push(item);
        }
        WalkAction::Continue
    });
    sort_path_items(items.as_mut_slice());
    items.truncate(PATH_COMPLETION_ITEM_LIMIT);
    items
}

fn path_item(
    mode: PathCompletionMode,
    query: &ResolvedPathQuery,
    entry: &WalkEntry,
) -> Option<ComposerPopupItem> {
    let name = entry.path.file_name()?.to_string_lossy().to_string();
    let is_dir = entry.kind == EntryKind::Dir;
    if !is_dir && mode == PathCompletionMode::Image && !is_image_name(&name) {
        return None;
    }
    let score = path_match_score(&query.prefix, &name, is_dir)?;
    let suffix = if is_dir { "/" } else { "" };
    let display = format!("{}{}{}", query.display_parent, name, suffix);
    let insert_text = match mode {
        PathCompletionMode::File | PathCompletionMode::Image => format!("@{display}"),
        PathCompletionMode::Path => display.clone(),
    };
    let mention_type = match (mode, is_dir) {
        (_, true) => ComposerPopupMentionType::Directory,
        (PathCompletionMode::Image, false) => ComposerPopupMentionType::Image,
        _ => ComposerPopupMentionType::File,
    };
    Some(ComposerPopupItem {
        value: insert_text.clone(),
        description: mention_type.label().to_owned(),
        insert_text,
        target: format!("file://{display}"),
        mention_type,
        match_indices: path_match_indices(&query.prefix, &display, mode),
        score,
    })
}

fn sort_path_items(items: &mut [ComposerPopupItem]) {
    items.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.value.cmp(&right.value))
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPathQuery {
    scan_dir: PathBuf,
    display_parent: String,
    prefix: String,
}

fn resolve_path_query(input: &str, cwd: &Path, home: Option<&Path>) -> Option<ResolvedPathQuery> {
    let normalized = input.trim().replace('\\', "/");
    let (parent_text, prefix) = split_parent_and_prefix(&normalized);
    let scan_dir = resolve_scan_dir(&parent_text, cwd, home)?;
    let display_parent = normalized_parent_display(&parent_text);
    Some(ResolvedPathQuery {
        scan_dir,
        display_parent,
        prefix,
    })
}

fn split_parent_and_prefix(input: &str) -> (String, String) {
    if input.ends_with('/') {
        return (input.to_owned(), String::new());
    }
    input.rsplit_once('/').map_or_else(
        || (String::new(), input.to_owned()),
        |(parent, prefix)| (format!("{parent}/"), prefix.to_owned()),
    )
}

fn resolve_scan_dir(parent_text: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if parent_text.is_empty() {
        return Some(cwd.to_path_buf());
    }
    let expanded = expand_home(parent_text, home)?;
    let is_windows_absolute = windows_absolute_path(&expanded);
    let path = PathBuf::from(expanded);
    Some(if path.is_absolute() || is_windows_absolute {
        path
    } else {
        cwd.join(path)
    })
}

fn expand_home(input: &str, home: Option<&Path>) -> Option<String> {
    if input == "~" || input.starts_with("~/") {
        let home = home?;
        let suffix = input.strip_prefix('~').unwrap_or_default();
        return Some(format!(
            "{}{}",
            home.to_string_lossy().replace('\\', "/"),
            suffix
        ));
    }
    Some(input.to_owned())
}

fn normalized_parent_display(parent_text: &str) -> String {
    if parent_text.is_empty() {
        String::new()
    } else if parent_text.ends_with('/') {
        parent_text.to_owned()
    } else {
        format!("{parent_text}/")
    }
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
}

fn is_image_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

fn path_match_score(prefix: &str, name: &str, is_dir: bool) -> Option<i32> {
    let base_score = if prefix.is_empty() {
        PREFIX_MATCH_SCORE
    } else {
        let query = prefix.to_ascii_lowercase();
        let name = name.to_ascii_lowercase();
        if name == query {
            EXACT_MATCH_SCORE
        } else if name.starts_with(&query) {
            PREFIX_MATCH_SCORE
        } else if name.contains(&query) {
            SUBSTRING_MATCH_SCORE
        } else {
            return None;
        }
    };
    Some(base_score.saturating_add(if is_dir { DIRECTORY_SCORE_BONUS } else { 0 }))
}

fn path_match_indices(prefix: &str, display: &str, mode: PathCompletionMode) -> Option<Vec<usize>> {
    if prefix.is_empty() {
        return None;
    }
    let value_offset = match mode {
        PathCompletionMode::File | PathCompletionMode::Image => 1,
        PathCompletionMode::Path => 0,
    };
    let display = display.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    let byte_start = display.rfind(&prefix)?;
    let char_start = display[..byte_start].chars().count() + value_offset;
    Some((char_start..char_start + prefix.chars().count()).collect())
}

pub(crate) fn path_like_token(token: &str) -> bool {
    token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.starts_with('\\')
        || token.contains('/')
        || token.contains('\\')
        || windows_absolute_path(token)
}

fn windows_absolute_path(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_home_parent_without_touching_real_environment() {
        let cwd = PathBuf::from("C:/workspace");
        let home = PathBuf::from("C:/Users/me");

        let query = resolve_path_query("~/src/ma", &cwd, Some(&home)).expect("应解析 home 路径");

        assert_eq!(query.scan_dir, PathBuf::from("C:/Users/me/src/"));
        assert_eq!(query.display_parent, "~/src/");
        assert_eq!(query.prefix, "ma");
    }

    #[test]
    fn image_completion_filters_non_image_files_and_keeps_directories() {
        let dir = axon_test_support::test_temp_dir();
        std::fs::create_dir(dir.path().join("assets")).expect("测试目录应可写入");
        std::fs::write(dir.path().join("logo.png"), "").expect("图片文件应可写入");
        std::fs::write(dir.path().join("logo.txt"), "").expect("文本文件应可写入");
        let key = PathCompletionKey {
            mode: PathCompletionMode::Image,
            cwd: dir.path().to_path_buf(),
            input: "lo".to_owned(),
        };

        let items = scan_path_completion(&key);

        assert!(items.iter().any(|item| item.value == "@logo.png"));
        assert!(!items.iter().any(|item| item.value == "@logo.txt"));
    }

    #[test]
    fn completion_scans_only_the_requested_directory() {
        let dir = axon_test_support::test_temp_dir();
        let nested = dir.path().join("src");
        std::fs::create_dir(&nested).expect("嵌套目录应可创建");
        std::fs::write(nested.join("model.rs"), "").expect("嵌套文件应可写入");
        let root_key = PathCompletionKey {
            mode: PathCompletionMode::File,
            cwd: dir.path().to_path_buf(),
            input: "model".to_owned(),
        };
        let nested_key = PathCompletionKey {
            mode: PathCompletionMode::File,
            cwd: dir.path().to_path_buf(),
            input: "src/mo".to_owned(),
        };

        assert!(scan_path_completion(&root_key).is_empty());
        assert!(
            scan_path_completion(&nested_key)
                .iter()
                .any(|item| item.value == "@src/model.rs")
        );
    }

    #[test]
    fn completion_caps_visible_items() {
        let dir = axon_test_support::test_temp_dir();
        for index in 0..32 {
            std::fs::write(dir.path().join(format!("item-{index:02}.rs")), "")
                .expect("测试文件应可写入");
        }
        let key = PathCompletionKey {
            mode: PathCompletionMode::File,
            cwd: dir.path().to_path_buf(),
            input: "item".to_owned(),
        };

        assert_eq!(scan_path_completion(&key).len(), PATH_COMPLETION_ITEM_LIMIT);
    }
}
