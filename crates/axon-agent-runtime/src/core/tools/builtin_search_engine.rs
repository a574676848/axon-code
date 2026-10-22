//! search tool 的流式文件扫描引擎。

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use axon_domain::{AxonError, AxonResult};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkFinish,
    SinkMatch,
};

use crate::{
    builtin_search_tool::{
        RegexSearchMode, SEARCH_TOOL_EMPTY_REGEX_ERROR, SEARCH_TOOL_REGEX_ERROR,
        SearchToolExecutionInput, SearchToolFile, SearchToolLine,
    },
    hashline_snapshot_tag,
    search_index::{IndexQuery, SearchIndexManager},
    workspace_file_session::WORKSPACE_FILE_SNAPSHOT_MAX_BYTES,
    workspace_walker::{
        WorkspaceWalkDefaultScope, WorkspaceWalkInclude, WorkspaceWalkInput, WorkspaceWalker,
    },
};

const SEARCH_DEFAULT_MAX_FILES: usize = 20;
const SEARCH_DEFAULT_MAX_MATCHES_PER_FILE: usize = 20;
const SEARCH_SINGLE_FILE_DEFAULT_MAX_MATCHES: usize = 200;
const SEARCH_BINARY_BYTE: u8 = 0;

pub(crate) struct SearchExecutionSummary {
    pub(crate) files: Vec<SearchToolFile>,
    pub(crate) files_scanned: usize,
    pub(crate) next_file_offset: Option<usize>,
    pub(crate) missing_paths: Vec<String>,
    pub(crate) binary_paths: Vec<String>,
    pub(crate) unreadable_paths: Vec<String>,
    pub(crate) incomplete: bool,
    pub(crate) symlinks_skipped: usize,
    pub(crate) root_escapes_skipped: usize,
}

pub(crate) fn scan_workspace(
    input: &SearchToolExecutionInput,
    root: &Path,
) -> AxonResult<SearchExecutionSummary> {
    scan_workspace_with_index(input, root, crate::search_index::global(), true)
}

/// `scan_workspace` 的实现体：允许注入索引管理器并显式关闭后台构建。
///
/// 差分 harness 用它构造确定性的对照：不回退路径使用真实管理器，回退路径使用空管理器
/// 且关闭构建，从而在同一临时 workspace 上比较两条路径的完整输出。
pub(crate) fn scan_workspace_with_index(
    input: &SearchToolExecutionInput,
    root: &Path,
    index: &Arc<SearchIndexManager>,
    allow_index_build: bool,
) -> AxonResult<SearchExecutionSummary> {
    let matcher = build_matcher(input)?;
    let walk = WorkspaceWalker::walk(WorkspaceWalkInput {
        root: root.to_path_buf(),
        paths: input.paths.clone(),
        hidden: input.hidden,
        gitignore: input.gitignore,
        include: WorkspaceWalkInclude::Files,
        default_scope: WorkspaceWalkDefaultScope::RecursiveFiles,
        cancel_token: input.cancel_token.clone(),
    })?;
    let is_literal = matches!(
        input.regex_mode,
        RegexSearchMode::Literal | RegexSearchMode::FixedString
    );
    let index_decision = index.prepare(
        root,
        &walk,
        IndexQuery {
            query: &input.query,
            is_literal,
            case_sensitive: input.case_sensitive,
            hidden: input.hidden,
            gitignore: input.gitignore,
            // 显式 paths 搜索的 walker 只覆盖被点名的文件，据此收敛快照会破坏索引。
            allow_update: allow_index_build && input.paths.is_empty(),
        },
    );
    let max_files = input.max_files.unwrap_or(SEARCH_DEFAULT_MAX_FILES);
    let default_match_limit = if is_explicit_file_target(input, root) {
        SEARCH_SINGLE_FILE_DEFAULT_MAX_MATCHES
    } else {
        SEARCH_DEFAULT_MAX_MATCHES_PER_FILE
    };
    let max_matches = input.max_matches_per_file.unwrap_or(default_match_limit);
    let mut summary = SearchExecutionSummary {
        files: Vec::new(),
        files_scanned: 0,
        next_file_offset: None,
        missing_paths: walk.missing_paths,
        binary_paths: Vec::new(),
        unreadable_paths: walk.unreadable_paths,
        incomplete: walk.incomplete,
        symlinks_skipped: walk.symlinks_skipped,
        root_escapes_skipped: walk.root_escapes_skipped,
    };
    let mut matched_file_index = 0usize;

    for entry in walk.entries {
        if !index_decision.should_scan(&entry.path) {
            continue;
        }
        if cancelled(input) {
            summary.incomplete = true;
            break;
        }
        summary.files_scanned = summary.files_scanned.saturating_add(1);
        let mut sink = SearchFileSink::new(&matcher, max_matches, input.cancel_token.as_ref());
        let search_result =
            build_searcher(input, max_matches).search_path(&matcher, &entry.path, &mut sink);

        if sink.cancelled {
            summary.incomplete = true;
            break;
        }
        if sink.binary {
            summary.binary_paths.push(entry.display_path);
            continue;
        }
        if search_result.is_err() || sink.invalid_utf8 {
            summary.unreadable_paths.push(entry.display_path);
            continue;
        }
        if sink.matched_count == 0 {
            continue;
        }
        if matched_file_index < input.file_offset {
            matched_file_index = matched_file_index.saturating_add(1);
            continue;
        }
        if summary.files.len() >= max_files {
            summary.next_file_offset = Some(matched_file_index);
            break;
        }

        let hashline_tag = record_snapshot(
            input,
            &entry.path,
            &sink.lines,
            entry.path.metadata().ok().map(|metadata| metadata.len()),
        );
        summary.files.push(SearchToolFile {
            path: entry.display_path,
            lines: sink.lines.into_values().collect(),
            matched_line_count: sink.matched_count.min(max_matches),
            matches_truncated: sink.matches_truncated,
            hashline_tag,
        });
        matched_file_index = matched_file_index.saturating_add(1);
    }

    deduplicate(&mut summary.binary_paths);
    deduplicate(&mut summary.unreadable_paths);
    if !summary.incomplete {
        // 只在这一轮走完了全部 walker entries 时更新索引，避免把不完整文件集写成新快照。
        index.schedule(index_decision.take_update());
    }
    Ok(summary)
}

fn build_matcher(input: &SearchToolExecutionInput) -> AxonResult<RegexMatcher> {
    let pattern = match input.regex_mode {
        RegexSearchMode::Literal | RegexSearchMode::FixedString => regex::escape(&input.query),
        RegexSearchMode::Regex => input.query.clone(),
    };
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!input.case_sensitive)
        .line_terminator(Some(b'\n'))
        .build(&pattern)
        .map_err(|_| AxonError::tool_invalid_arguments(SEARCH_TOOL_REGEX_ERROR))?;
    if matcher
        .find(b"")
        .map_err(|_| AxonError::tool_invalid_arguments(SEARCH_TOOL_REGEX_ERROR))?
        .is_some()
    {
        return Err(AxonError::tool_invalid_arguments(
            SEARCH_TOOL_EMPTY_REGEX_ERROR,
        ));
    }
    Ok(matcher)
}

fn is_explicit_file_target(input: &SearchToolExecutionInput, root: &Path) -> bool {
    let [expression] = input.paths.as_slice() else {
        return false;
    };
    let normalized = expression.trim().replace('\\', "/");
    if normalized.contains(['*', '?', '[', '{']) {
        return false;
    }
    let path = PathBuf::from(normalized);
    if path.is_absolute() {
        path.is_file()
    } else {
        root.join(path).is_file()
    }
}

fn build_searcher(input: &SearchToolExecutionInput, max_matches: usize) -> Searcher {
    let mut builder = SearcherBuilder::new();
    builder
        .line_number(true)
        .before_context(input.context_before)
        .after_context(input.context_after)
        .binary_detection(BinaryDetection::quit(SEARCH_BINARY_BYTE))
        .bom_sniffing(false)
        .max_matches(Some(
            u64::try_from(max_matches.saturating_add(1)).unwrap_or(u64::MAX),
        ));
    builder.build()
}

struct SearchFileSink<'a> {
    matcher: &'a RegexMatcher,
    max_matches: usize,
    cancel_token: Option<&'a crate::workspace_walker::WorkspaceWalkCancelToken>,
    lines: BTreeMap<usize, SearchToolLine>,
    pending_before: BTreeMap<usize, SearchToolLine>,
    matched_count: usize,
    matches_truncated: bool,
    binary: bool,
    invalid_utf8: bool,
    cancelled: bool,
}

impl<'a> SearchFileSink<'a> {
    fn new(
        matcher: &'a RegexMatcher,
        max_matches: usize,
        cancel_token: Option<&'a crate::workspace_walker::WorkspaceWalkCancelToken>,
    ) -> Self {
        Self {
            matcher,
            max_matches,
            cancel_token,
            lines: BTreeMap::new(),
            pending_before: BTreeMap::new(),
            matched_count: 0,
            matches_truncated: false,
            binary: false,
            invalid_utf8: false,
            cancelled: false,
        }
    }

    fn should_continue(&mut self) -> bool {
        if self.cancel_token.is_some_and(|token| token()) {
            self.cancelled = true;
            false
        } else {
            true
        }
    }

    fn line(
        &mut self,
        line_number: Option<u64>,
        bytes: &[u8],
        is_match: bool,
    ) -> Option<SearchToolLine> {
        let line_number = usize::try_from(line_number?).ok()?;
        let text = match std::str::from_utf8(trim_line_terminator(bytes)) {
            Ok(text) => text.to_owned(),
            Err(_) => {
                self.invalid_utf8 = true;
                return None;
            }
        };
        let mut match_spans = Vec::new();
        if is_match
            && self
                .matcher
                .find_iter(text.as_bytes(), |matched| {
                    match_spans.push((matched.start(), matched.end()));
                    true
                })
                .is_err()
        {
            self.invalid_utf8 = true;
            return None;
        }
        Some(SearchToolLine {
            line_number,
            is_match,
            match_spans,
            text,
        })
    }

    fn insert_context(&mut self, line: SearchToolLine) {
        self.lines.entry(line.line_number).or_insert(line);
    }

    fn insert_match(&mut self, line: SearchToolLine) {
        self.lines
            .entry(line.line_number)
            .and_modify(|existing| {
                existing.is_match = true;
                existing.match_spans = line.match_spans.clone();
                existing.text.clone_from(&line.text);
            })
            .or_insert(line);
    }
}

impl Sink for SearchFileSink<'_> {
    type Error = io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        matched: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        if !self.should_continue() {
            return Ok(false);
        }
        if self.matched_count >= self.max_matches {
            self.matches_truncated = true;
            self.pending_before.clear();
            return Ok(false);
        }
        let Some(line) = self.line(matched.line_number(), matched.bytes(), true) else {
            return Ok(false);
        };
        let pending = std::mem::take(&mut self.pending_before);
        for context in pending.into_values() {
            self.insert_context(context);
        }
        self.insert_match(line);
        self.matched_count = self.matched_count.saturating_add(1);
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        if !self.should_continue() {
            return Ok(false);
        }
        let Some(line) = self.line(context.line_number(), context.bytes(), false) else {
            return Ok(false);
        };
        match context.kind() {
            SinkContextKind::Before => {
                self.pending_before.insert(line.line_number, line);
            }
            SinkContextKind::After | SinkContextKind::Other => self.insert_context(line),
        }
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.pending_before.clear();
        Ok(self.should_continue())
    }

    fn binary_data(
        &mut self,
        _searcher: &Searcher,
        _binary_byte_offset: u64,
    ) -> Result<bool, Self::Error> {
        self.binary = true;
        Ok(false)
    }

    fn finish(&mut self, _searcher: &Searcher, finish: &SinkFinish) -> Result<(), Self::Error> {
        self.binary |= finish.binary_byte_offset().is_some();
        Ok(())
    }
}

fn record_snapshot(
    input: &SearchToolExecutionInput,
    path: &Path,
    lines: &BTreeMap<usize, SearchToolLine>,
    file_len: Option<u64>,
) -> Option<String> {
    let max_snapshot_bytes = u64::try_from(WORKSPACE_FILE_SNAPSHOT_MAX_BYTES).unwrap_or(u64::MAX);
    if file_len.is_none_or(|len| len > max_snapshot_bytes) {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let tag = hashline_snapshot_tag(&content);
    if let Some(session) = input.snapshot_session.as_deref() {
        let snapshot_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        session.record_read(&snapshot_path, content, lines.keys().copied());
    }
    Some(tag)
}

fn trim_line_terminator(mut bytes: &[u8]) -> &[u8] {
    if bytes.ends_with(b"\n") {
        bytes = &bytes[..bytes.len().saturating_sub(1)];
    }
    if bytes.ends_with(b"\r") {
        bytes = &bytes[..bytes.len().saturating_sub(1)];
    }
    bytes
}

fn cancelled(input: &SearchToolExecutionInput) -> bool {
    input.cancel_token.as_ref().is_some_and(|token| token())
}

fn deduplicate(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}
