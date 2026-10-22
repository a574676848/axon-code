//! Hashline 文本解析与文件级操作约束。

use std::collections::HashMap;

use super::{
    HASHLINE_LEGACY_TAG_LEN, HASHLINE_MISSING_TAG_HINT, HASHLINE_TAG_LEN, HashlineFileOperation,
    HashlineOp, hashline_legacy_tag_hint,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HashlineSection {
    pub(super) path: String,
    pub(super) tag: String,
    pub(super) ops: Vec<HashlineOp>,
    pub(super) file_operation: HashlineFileOperation,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Swap { start: usize, end: usize },
    Delete { start: usize, end: usize },
    InsertBefore { line: usize },
    InsertAfter { line: usize },
    InsertHead,
    InsertTail,
    SwapBlock { line: usize },
    DeleteBlock { line: usize },
    InsertAfterBlock { line: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingOp {
    line_number: usize,
    kind: PendingKind,
    body: Vec<PendingBodyRow>,
    deferred_blanks: Vec<PendingBodyRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingBodyRow {
    text: String,
    bare: bool,
}

const BARE_BODY_WARNING: &str =
    "已为未带 + 的 Hashline body 行自动补全字面量语义；后续请使用 +TEXT";

pub(super) fn parse_sections(input: &str) -> Result<Vec<HashlineSection>, String> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut sections = Vec::new();
    let mut section: Option<HashlineSection> = None;
    let mut pending: Option<PendingOp> = None;
    for (index, raw_line) in input.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.trim().is_empty() {
            if let Some(pending) = pending.as_mut()
                && !pending.body.is_empty()
            {
                pending.deferred_blanks.push(PendingBodyRow {
                    text: line.to_owned(),
                    bare: true,
                });
            }
            continue;
        }
        if line.trim_start().starts_with('#') && pending.is_none() {
            continue;
        }
        if pending.is_none()
            && (line.trim().starts_with("<<<<")
                || line.trim().starts_with("====")
                || line.trim().starts_with(">>>>"))
        {
            continue;
        }
        let header_candidate =
            line.trim_start().starts_with('[') && (pending.is_none() || line.contains('#'));
        if header_candidate && let Some((path, tag)) = parse_header(line)? {
            flush_pending(&mut section, &mut pending)?;
            if let Some(previous) = section.take() {
                push_section(&mut sections, previous)?;
            }
            section = Some(HashlineSection {
                path,
                tag,
                ops: Vec::new(),
                file_operation: HashlineFileOperation::Update,
                warnings: Vec::new(),
            });
            continue;
        }
        let Some(active) = section.as_mut() else {
            return Err(format!("line {line_number}: {HASHLINE_MISSING_TAG_HINT}"));
        };
        if let Some(file_operation) = parse_file_operation(line, line_number)? {
            flush_pending(&mut section, &mut pending)?;
            set_file_operation(
                section.as_mut().expect("section 已验证存在"),
                file_operation,
                line_number,
            )?;
            continue;
        }
        if let Some(kind) = parse_op_header(line, line_number)? {
            flush_pending(&mut section, &mut pending)?;
            pending = Some(PendingOp {
                line_number,
                kind,
                body: Vec::new(),
                deferred_blanks: Vec::new(),
            });
            continue;
        }
        if pending.is_none() && active.file_operation == HashlineFileOperation::Update {
            pending = Some(PendingOp {
                line_number,
                kind: PendingKind::InsertTail,
                body: Vec::new(),
                deferred_blanks: Vec::new(),
            });
        }
        if let Some(payload) = line.strip_prefix('+') {
            let Some(pending) = pending.as_mut() else {
                let operation = match &active.file_operation {
                    HashlineFileOperation::Delete => "REM",
                    HashlineFileOperation::Move { .. } => "MV",
                    HashlineFileOperation::Update => "文件级操作",
                };
                return Err(format!("line {line_number}: {operation} 不接收 body 行"));
            };
            if matches!(
                pending.kind,
                PendingKind::Delete { .. } | PendingKind::DeleteBlock { .. }
            ) {
                return Err(format!("line {line_number}: DEL 操作不接收 body 行"));
            }
            commit_deferred_blanks(pending);
            pending.body.push(PendingBodyRow {
                text: payload.to_owned(),
                bare: false,
            });
            continue;
        }
        if matches!(
            pending.as_ref().map(|pending| pending.kind),
            Some(PendingKind::Delete { .. } | PendingKind::DeleteBlock { .. })
        ) && line.trim_start().starts_with('-')
        {
            continue;
        }
        let Some(pending) = pending.as_mut() else {
            return Err(format!(
                "line {line_number}: Hashline body 行必须以 + 开头；先写操作行，例如 INS.TAIL:"
            ));
        };
        if matches!(
            pending.kind,
            PendingKind::Delete { .. } | PendingKind::DeleteBlock { .. }
        ) {
            return Err(format!("line {line_number}: DEL 操作不接收 body 行"));
        }
        if !active
            .warnings
            .iter()
            .any(|warning| warning == BARE_BODY_WARNING)
        {
            active.warnings.push(BARE_BODY_WARNING.to_owned());
        }
        commit_deferred_blanks(pending);
        pending.body.push(PendingBodyRow {
            text: line.to_owned(),
            bare: true,
        });
    }
    flush_pending(&mut section, &mut pending)?;
    if let Some(last) = section {
        push_section(&mut sections, last)?;
    }
    if sections.is_empty() {
        return Err(HASHLINE_MISSING_TAG_HINT.to_owned());
    }
    merge_same_path_sections(sections)
}

fn merge_same_path_sections(
    sections: Vec<HashlineSection>,
) -> Result<Vec<HashlineSection>, String> {
    let mut merged = Vec::<HashlineSection>::with_capacity(sections.len());
    let mut indexes = HashMap::<String, usize>::with_capacity(sections.len());
    for section in sections {
        let Some(index) = indexes.get(&section.path).copied() else {
            indexes.insert(section.path.clone(), merged.len());
            merged.push(section);
            continue;
        };
        let existing = &mut merged[index];
        if existing.tag != section.tag {
            return Err(format!(
                "同一 Hashline 文件 {} 使用了冲突的快照标签 #{} 和 #{}；请重新读取后只使用一个当前 header",
                section.path, existing.tag, section.tag
            ));
        }
        match section.file_operation {
            HashlineFileOperation::Update => {}
            operation if existing.file_operation == HashlineFileOperation::Update => {
                existing.file_operation = operation;
            }
            _ => {
                return Err(format!(
                    "同一 Hashline 文件 {} 最多包含一个 REM 或 MV",
                    section.path
                ));
            }
        }
        existing.ops.extend(section.ops);
        existing.warnings.extend(section.warnings);
        if existing.file_operation == HashlineFileOperation::Delete && !existing.ops.is_empty() {
            return Err(format!(
                "Hashline section {} 的 REM 不能与行编辑组合",
                existing.path
            ));
        }
    }
    Ok(merged)
}

fn push_section(
    sections: &mut Vec<HashlineSection>,
    section: HashlineSection,
) -> Result<(), String> {
    if section.ops.is_empty() && section.file_operation == HashlineFileOperation::Update {
        return Err(format!(
            "Hashline section {} 没有可应用的操作",
            section.path
        ));
    }
    if section.file_operation == HashlineFileOperation::Delete && !section.ops.is_empty() {
        return Err(format!(
            "Hashline section {} 的 REM 不能与行编辑组合",
            section.path
        ));
    }
    sections.push(section);
    Ok(())
}

fn parse_header(line: &str) -> Result<Option<(String, String)>, String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return Ok(None);
    }
    if !trimmed.ends_with(']') {
        return Err(HASHLINE_MISSING_TAG_HINT.to_owned());
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let Some((path, tag)) = inner.rsplit_once('#') else {
        return Err(HASHLINE_MISSING_TAG_HINT.to_owned());
    };
    let path = normalize_header_path(path);
    let tag = tag.trim().to_ascii_uppercase();
    if path.is_empty() || path.contains('#') || tag.is_empty() {
        return Err(HASHLINE_MISSING_TAG_HINT.to_owned());
    }
    let is_hex = tag.chars().all(|ch| ch.is_ascii_hexdigit());
    // 旧格式（位数提升前的历史宽度）先单独识别：它必然来自过期快照，
    // 给模型一个可直接执行的迁移动作，而不是让它反复猜测位数。
    if tag.len() == HASHLINE_LEGACY_TAG_LEN && is_hex {
        return Err(hashline_legacy_tag_hint());
    }
    if tag.len() != HASHLINE_TAG_LEN || !is_hex {
        return Err(format!(
            "Hashline 快照标签 {tag} 无效，应为 {HASHLINE_TAG_LEN} 位十六进制大写标签"
        ));
    }
    Ok(Some((path, tag)))
}

fn parse_file_operation(
    line: &str,
    line_number: usize,
) -> Result<Option<HashlineFileOperation>, String> {
    let trimmed = line.trim();
    if trimmed == "REM" {
        return Ok(Some(HashlineFileOperation::Delete));
    }
    if let Some(destination) = trimmed.strip_prefix("MV ") {
        let destination = normalize_path(destination);
        if destination.is_empty() {
            return Err(format!("line {line_number}: MV 需要非空目标路径"));
        }
        return Ok(Some(HashlineFileOperation::Move { destination }));
    }
    Ok(None)
}

fn set_file_operation(
    section: &mut HashlineSection,
    operation: HashlineFileOperation,
    line_number: usize,
) -> Result<(), String> {
    if section.file_operation != HashlineFileOperation::Update {
        return Err(format!(
            "line {line_number}: 每个 Hashline section 最多包含一个 REM 或 MV"
        ));
    }
    if operation == HashlineFileOperation::Delete && !section.ops.is_empty() {
        return Err(format!("line {line_number}: REM 不能与行编辑组合"));
    }
    section.file_operation = operation;
    Ok(())
}

fn parse_op_header(line: &str, line_number: usize) -> Result<Option<PendingKind>, String> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("SWAP.BLK ") {
        return parse_block_anchor(rest, line_number)
            .map(|line| Some(PendingKind::SwapBlock { line }));
    }
    if let Some(rest) = trimmed.strip_prefix("DEL.BLK ") {
        if rest.ends_with(':') {
            return Err(format!("line {line_number}: DEL.BLK N 不接收冒号或 body"));
        }
        return parse_line_number(rest.trim(), line_number)
            .map(|line| Some(PendingKind::DeleteBlock { line }));
    }
    if let Some(rest) = trimmed.strip_prefix("INS.BLK.POST ") {
        return parse_block_anchor(rest, line_number)
            .map(|line| Some(PendingKind::InsertAfterBlock { line }));
    }
    if let Some(rest) = trimmed.strip_prefix("SWAP ") {
        let range = trim_optional_hunk_suffix(rest);
        let (start, end) = parse_range(range, line_number)?;
        return Ok(Some(PendingKind::Swap { start, end }));
    }
    if let Some(rest) = trimmed.strip_prefix("DEL ") {
        if rest.ends_with(':') {
            return Err(format!("line {line_number}: DEL N.=M 不接收冒号或 body"));
        }
        let (start, end) = parse_range(rest.trim_end_matches('.'), line_number)?;
        return Ok(Some(PendingKind::Delete { start, end }));
    }
    if let Some(rest) = trimmed.strip_prefix("DEL.RANGE") {
        let range = rest.trim().trim_start_matches(':').trim();
        let (start, end) = parse_range(range, line_number)?;
        return Ok(Some(PendingKind::Delete { start, end }));
    }
    if let Some(rest) = trimmed.strip_prefix("INS.PRE ") {
        return parse_insert_anchor(trim_optional_hunk_suffix(rest), line_number)
            .map(|line| Some(PendingKind::InsertBefore { line }));
    }
    if let Some(rest) = trimmed.strip_prefix("INS.POST ") {
        return parse_insert_anchor(trim_optional_hunk_suffix(rest), line_number)
            .map(|line| Some(PendingKind::InsertAfter { line }));
    }
    if matches!(trimmed, "INS.HEAD" | "INS.HEAD:" | "INS.HEAD.:") {
        return Ok(Some(PendingKind::InsertHead));
    }
    if matches!(trimmed, "INS.TAIL" | "INS.TAIL:" | "INS.TAIL.:") {
        return Ok(Some(PendingKind::InsertTail));
    }
    if looks_like_patch_or_diff(trimmed) {
        return Err(format!(
            "line {line_number}: 检测到不完整或混入 Hashline section 的 diff/patch 语法"
        ));
    }
    Ok(None)
}

fn parse_block_anchor(raw: &str, line_number: usize) -> Result<usize, String> {
    let Some(value) = raw.trim().strip_suffix(':') else {
        return Err(format!("line {line_number}: BLK 操作需要以冒号结尾"));
    };
    parse_line_number(value.trim(), line_number)
}

fn parse_range(raw: &str, line_number: usize) -> Result<(usize, usize), String> {
    let raw = raw.trim();
    if raw.chars().all(|ch| ch.is_ascii_digit()) {
        let line = parse_line_number(raw, line_number)?;
        return Ok((line, line));
    }
    let range = [".=", "..", "…", "-"]
        .into_iter()
        .find_map(|separator| raw.split_once(separator))
        .or_else(|| {
            let mut parts = raw.split_whitespace();
            let start = parts.next()?;
            let end = parts.next()?;
            parts.next().is_none().then_some((start, end))
        });
    let (start, end) =
        range.ok_or_else(|| format!("line {line_number}: range 应为 N、N.=M 或等价范围"))?;
    let start = parse_line_number(start.trim(), line_number)?;
    let end = parse_line_number(end.trim(), line_number)?;
    if end < start {
        return Err(format!(
            "line {line_number}: range {start}.={end} 结束行早于开始行"
        ));
    }
    Ok((start, end))
}

fn parse_insert_anchor(raw: &str, line_number: usize) -> Result<usize, String> {
    parse_line_number(raw.trim(), line_number)
}

fn trim_optional_hunk_suffix(raw: &str) -> &str {
    raw.trim()
        .strip_suffix(':')
        .unwrap_or(raw.trim())
        .trim_end_matches('.')
        .trim()
}

fn parse_line_number(raw: &str, line_number: usize) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("line {line_number}: 行号必须是正整数"))?;
    if value == 0 {
        return Err(format!("line {line_number}: 行号必须从 1 开始"));
    }
    Ok(value)
}

fn looks_like_patch_or_diff(line: &str) -> bool {
    line.starts_with("*** ")
        || line.starts_with("@@")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
}

fn flush_pending(
    section: &mut Option<HashlineSection>,
    pending: &mut Option<PendingOp>,
) -> Result<(), String> {
    let Some(mut pending) = pending.take() else {
        return Ok(());
    };
    let op = match pending.kind {
        PendingKind::Swap { start, end } => {
            let lines = body_lines(&mut pending);
            if lines.is_empty() {
                HashlineOp::Delete { start, end }
            } else {
                HashlineOp::Swap { start, end, lines }
            }
        }
        PendingKind::Delete { start, end } => HashlineOp::Delete { start, end },
        PendingKind::InsertBefore { line } => HashlineOp::InsertBefore {
            line,
            lines: required_body(&mut pending)?,
        },
        PendingKind::InsertAfter { line } => HashlineOp::InsertAfter {
            line,
            lines: required_body(&mut pending)?,
            block_start: None,
        },
        PendingKind::InsertHead => HashlineOp::InsertHead {
            lines: required_body(&mut pending)?,
        },
        PendingKind::InsertTail => HashlineOp::InsertTail {
            lines: required_body(&mut pending)?,
        },
        PendingKind::SwapBlock { line } => HashlineOp::SwapBlock {
            line,
            lines: required_body(&mut pending)?,
        },
        PendingKind::DeleteBlock { line } => HashlineOp::DeleteBlock { line },
        PendingKind::InsertAfterBlock { line } => HashlineOp::InsertAfterBlock {
            line,
            lines: required_body(&mut pending)?,
        },
    };
    section
        .as_mut()
        .ok_or_else(|| HASHLINE_MISSING_TAG_HINT.to_owned())?
        .ops
        .push(op);
    Ok(())
}

fn required_body(pending: &mut PendingOp) -> Result<Vec<String>, String> {
    let body = body_lines(pending);
    if body.is_empty() {
        return Err(format!(
            "line {}: SWAP/INS 操作需要至少一行 +TEXT body",
            pending.line_number
        ));
    }
    Ok(body)
}

fn body_lines(pending: &mut PendingOp) -> Vec<String> {
    strip_uniform_read_prefixes(&mut pending.body);
    pending.body.drain(..).map(|row| row.text).collect()
}

fn commit_deferred_blanks(pending: &mut PendingOp) {
    pending.body.append(&mut pending.deferred_blanks);
}

fn strip_uniform_read_prefixes(rows: &mut [PendingBodyRow]) {
    let bare = rows
        .iter()
        .filter(|row| row.bare && !row.text.trim().is_empty())
        .collect::<Vec<_>>();
    if bare.is_empty() {
        return;
    }
    let stripped = bare
        .iter()
        .map(|row| strip_read_prefix(&row.text))
        .collect::<Option<Vec<_>>>();
    let Some(stripped) = stripped else {
        return;
    };
    if stripped.iter().all(|value| looks_like_literal_value(value)) {
        return;
    }
    for row in rows.iter_mut().filter(|row| row.bare) {
        if let Some(value) = strip_read_prefix(&row.text) {
            row.text = value.to_owned();
        }
    }
}

fn strip_read_prefix(value: &str) -> Option<&str> {
    let (prefix, content) = value.split_once(':')?;
    (!prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit())).then_some(content)
}

fn looks_like_literal_value(value: &str) -> bool {
    let value = value.trim().trim_end_matches(',').trim();
    value.parse::<f64>().is_ok()
        || value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
}

fn normalize_header_path(path: &str) -> String {
    let mut value = path.trim();
    let without_stars = value.trim_start_matches('*').trim_start();
    let lower = without_stars.to_ascii_lowercase();
    for action in ["update", "add", "delete", "move"] {
        let Some(rest) = lower.strip_prefix(action) else {
            continue;
        };
        let Some(colon) = rest.find(':') else {
            break;
        };
        let qualifier = rest[..colon]
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>();
        if matches!(qualifier.as_str(), "" | "file" | "to") {
            value = without_stars[action.len() + colon + 1..].trim_start();
        }
        break;
    }
    normalize_path(value.trim_start_matches('*').trim_start())
}

fn normalize_path(path: &str) -> String {
    let path = path.trim();
    let path = if path.len() >= 2 {
        let first = path.as_bytes()[0];
        let last = path.as_bytes()[path.len() - 1];
        if matches!(first, b'\'' | b'"') && first == last {
            &path[1..path.len() - 1]
        } else {
            path
        }
    } else {
        path
    };
    path.replace('\\', "/")
}

#[cfg(test)]
#[path = "hashline_edit_parser_tests.rs"]
mod tests;
