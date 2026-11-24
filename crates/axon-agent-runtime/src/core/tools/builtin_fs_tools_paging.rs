use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader},
    path::Path,
};

use axon_crypto::{Sha256Hex, sha256_hex};
use axon_domain::{AxonError, AxonResult};

use super::{FS_TOOL_PATH_NOT_FOUND_ERROR, FS_TOOL_PATH_UNAVAILABLE_ERROR};
use crate::workspace_file_session::WORKSPACE_FILE_SNAPSHOT_MAX_BYTES;

pub(super) const READ_FILE_DEFAULT_LINE_LIMIT: usize = 2_000;

const PATH_SUGGESTION_LIMIT: usize = 3;

#[derive(Default)]
pub(super) struct TextPage {
    pub(super) preview: String,
    pub(super) line_end: usize,
    pub(super) total_lines: usize,
    pub(super) next_offset: Option<usize>,
    pub(super) returned_bytes: usize,
    pub(super) truncation_reason: Option<String>,
    pub(super) original_chars: usize,
    pub(super) filtered_chars: usize,
    pub(super) content_sha256: String,
    pub(super) snapshot_content: Option<String>,
    pub(super) seen_lines: BTreeSet<usize>,
    pub(super) filter_result: Option<axon_output_filter::lsp_symbol_filter::LspFilterResult>,
}

pub(super) fn read_text_page(path: &Path, offset: usize, limit: usize) -> AxonResult<TextPage> {
    let file = fs::File::open(path)
        .map_err(|_| AxonError::tool_execution_failed(FS_TOOL_PATH_UNAVAILABLE_ERROR))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256Hex::new();
    let mut raw = Vec::new();
    let mut snapshot_bytes = Vec::new();
    let mut snapshot_enabled = true;
    let mut page = TextPage::default();
    let mut lines = Vec::new();

    loop {
        raw.clear();
        let read = reader
            .read_until(b'\n', &mut raw)
            .map_err(|_| AxonError::tool_execution_failed(FS_TOOL_PATH_UNAVAILABLE_ERROR))?;
        if read == 0 {
            break;
        }
        hasher.update(&raw);
        if snapshot_enabled
            && snapshot_bytes.len().saturating_add(raw.len()) <= WORKSPACE_FILE_SNAPSHOT_MAX_BYTES
        {
            snapshot_bytes.extend_from_slice(&raw);
        } else {
            snapshot_enabled = false;
            snapshot_bytes.clear();
        }
        page.total_lines += 1;
        let text = std::str::from_utf8(&raw)
            .map_err(|_| AxonError::tool_invalid_arguments("read_file 仅支持 UTF-8 文本文件"))?;
        page.original_chars += text.chars().count();
        if page.total_lines < offset || lines.len() >= limit {
            continue;
        }
        let line_number = page.total_lines;
        push_numbered_line(
            &mut page,
            &mut lines,
            line_number,
            text.trim_end_matches(['\r', '\n']),
        );
    }

    let content_sha256 = hasher.finalize_hex();
    if snapshot_enabled {
        page.snapshot_content = String::from_utf8(snapshot_bytes).ok();
    }
    finish_page(page, lines, offset, content_sha256)
}

pub(super) fn page_text(content: &str, offset: usize, limit: usize) -> AxonResult<TextPage> {
    let mut page = TextPage {
        original_chars: content.chars().count(),
        content_sha256: sha256_hex(content.as_bytes()),
        snapshot_content: (content.len() <= WORKSPACE_FILE_SNAPSHOT_MAX_BYTES)
            .then(|| content.to_owned()),
        ..TextPage::default()
    };
    let mut lines = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        page.total_lines = line_number;
        if line_number < offset || lines.len() >= limit {
            continue;
        }
        push_numbered_line(&mut page, &mut lines, line_number, line);
    }
    let hash = page.content_sha256.clone();
    finish_page(page, lines, offset, hash)
}

pub(super) fn missing_path_error(root: &Path, requested: &Path) -> AxonError {
    let suggestions = path_suggestions(root, requested);
    let message = if suggestions.is_empty() {
        FS_TOOL_PATH_NOT_FOUND_ERROR.to_owned()
    } else {
        format!(
            "{FS_TOOL_PATH_NOT_FOUND_ERROR}；相似路径：{}",
            suggestions.join("、")
        )
    };
    AxonError::tool_invalid_arguments(message)
}

fn push_numbered_line(page: &mut TextPage, lines: &mut Vec<String>, number: usize, line: &str) {
    let numbered = format!("{number}:{line}");
    let additional = numbered.len() + usize::from(!lines.is_empty());
    page.returned_bytes = page.returned_bytes.saturating_add(additional);
    page.line_end = number;
    page.seen_lines.insert(number);
    lines.push(numbered);
}

fn finish_page(
    mut page: TextPage,
    lines: Vec<String>,
    offset: usize,
    content_sha256: String,
) -> AxonResult<TextPage> {
    if offset > page.total_lines.saturating_add(1) {
        return Err(AxonError::tool_invalid_arguments(format!(
            "read_file offset {offset} 超出文件行号范围；当前共有 {} 行",
            page.total_lines
        )));
    }
    page.preview = lines.join("\n");
    page.filtered_chars = page.preview.chars().count();
    page.content_sha256 = content_sha256;
    page.next_offset =
        (page.line_end < page.total_lines).then_some(page.line_end.saturating_add(1).max(offset));
    if page.next_offset.is_some() && page.truncation_reason.is_none() {
        page.truncation_reason = Some("line_limit".to_owned());
    }
    Ok(page)
}

fn path_suggestions(root: &Path, requested: &Path) -> Vec<String> {
    let parent = requested.parent().unwrap_or_else(|| Path::new(""));
    let target = requested
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Ok(entries) = fs::read_dir(root.join(parent)) else {
        return Vec::new();
    };
    let mut candidates = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| similar_name(&name.to_ascii_lowercase(), &target))
        .map(|name| parent.join(name).to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.truncate(PATH_SUGGESTION_LIMIT);
    candidates
}

fn similar_name(candidate: &str, target: &str) -> bool {
    candidate.contains(target)
        || target.contains(candidate)
        || candidate
            .chars()
            .zip(target.chars())
            .filter(|(left, right)| left != right)
            .count()
            <= 2
}
