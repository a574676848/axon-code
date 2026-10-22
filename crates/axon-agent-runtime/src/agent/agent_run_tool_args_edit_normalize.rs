//! builtin.edit 输入文本的规范化与语法容错。

const APPLY_PATCH_BEGIN_MARKER: &str = "*** Begin Patch";
const APPLY_PATCH_END_MARKER: &str = "*** End Patch";
const UPDATE_FILE_MARKER: &str = "*** Update File:";
const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";
const LAZY_EDIT_SLASH_PLACEHOLDER: &str = "// ... existing code ...";
const LAZY_EDIT_HASH_PLACEHOLDER: &str = "# ... existing code ...";

/// 误入 Update 块时不得补成上下文行的 CLI 命令首词。
///
/// 只收录几乎不会作为源码行首独立词出现的命令名：与源码语法重叠的 `go`、`echo`、`set`、
/// `source`、`export`、`head`、`tail`、`where` 等一律不收录，避免把真实代码行误判成 shell 命令。
const SHELL_COMMAND_TOKENS: &[&str] = &[
    "cargo",
    "rustc",
    "rustup",
    "python",
    "python3",
    "pip",
    "pip3",
    "uv",
    "pytest",
    "poetry",
    "node",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "dotnet",
    "mvn",
    "gradle",
    "make",
    "cmake",
    "ninja",
    "git",
    "gh",
    "docker",
    "docker-compose",
    "kubectl",
    "helm",
    "terraform",
    "systemctl",
    "pwsh",
    "powershell",
    "cmd",
    "bash",
    "zsh",
    "sh",
    "fish",
    "sudo",
    "curl",
    "wget",
    "ssh",
    "scp",
    "rsync",
    "tar",
    "zip",
    "unzip",
    "rm",
    "cp",
    "mv",
    "mkdir",
    "rmdir",
    "touch",
    "chmod",
    "chown",
    "ln",
    "cat",
    "ls",
    "wc",
    "sort",
    "uniq",
    "grep",
    "rg",
    "sed",
    "awk",
    "find",
    "xargs",
    "tee",
    "which",
    "sleep",
    "kill",
];

/// Canonicalize the user-facing PathDiff alias before shared execution and receipt code.
pub(crate) fn normalize_pathdiff_envelope(input: &str) -> String {
    let unquoted = strip_markdown_code_fence(input);
    let denoised = strip_apply_patch_header_noise(unquoted);
    let trimmed = denoised.trim();
    if trimmed.is_empty() {
        return input.to_owned();
    }

    // 1. 语法混淆容错：若纯 Hashline 编辑被错误套上了 *** Begin Patch / *** End Patch，自动解包为纯 Hashline
    if let Some(inner) = strip_patch_envelope_if_pure_hashline(trimmed) {
        return inner;
    }

    // 2. 语法混淆容错：清理在 *** Update File: 下方错误混入的 [path#TAG] 行
    let cleaned = strip_hashline_headers_inside_update_file(trimmed);
    let padded = pad_lazy_update_block_lines(&cleaned);
    let normalized_lazy = normalize_lazy_edit_placeholders(&padded);
    let trimmed = normalized_lazy.as_str();

    let has_patch_file_marker = trimmed.contains("*** Add File:")
        || trimmed.contains("*** Update File:")
        || trimmed.contains("*** Delete File:")
        || trimmed.contains("*** Create File:");
    let is_hashline = trimmed.starts_with('[') && trimmed.contains('#');
    let needs_auto_wrap = !is_hashline && has_patch_file_marker;

    let has_begin =
        trimmed.starts_with(APPLY_PATCH_BEGIN_MARKER) || trimmed.starts_with("*** Begin PathDiff");
    let has_end =
        trimmed.ends_with(APPLY_PATCH_END_MARKER) || trimmed.ends_with("*** End PathDiff");

    if !has_begin && !has_end && !needs_auto_wrap {
        return if denoised == input {
            input.to_owned()
        } else {
            trimmed.to_owned()
        };
    }

    let mut envelope = trimmed.to_owned();
    if !has_begin && needs_auto_wrap {
        envelope = format!("{APPLY_PATCH_BEGIN_MARKER}\n{envelope}");
    }
    if !has_end && (needs_auto_wrap || has_begin) {
        envelope = format!("{envelope}\n{APPLY_PATCH_END_MARKER}");
    }

    let envelope = envelope
        .replacen("*** Begin PathDiff", APPLY_PATCH_BEGIN_MARKER, 1)
        .replacen("*** End PathDiff", APPLY_PATCH_END_MARKER, 1)
        .replace("*** Create File:", "*** Add File:");
    normalize_add_file_lines(&envelope)
}

fn strip_patch_envelope_if_pure_hashline(trimmed: &str) -> Option<String> {
    let has_begin =
        trimmed.starts_with(APPLY_PATCH_BEGIN_MARKER) || trimmed.starts_with("*** Begin PathDiff");
    let has_end =
        trimmed.ends_with(APPLY_PATCH_END_MARKER) || trimmed.ends_with("*** End PathDiff");
    if !has_begin && !has_end {
        return None;
    }
    let mut inner = trimmed;
    if let Some(rest) = inner.strip_prefix(APPLY_PATCH_BEGIN_MARKER) {
        inner = rest.trim_start();
    } else if let Some(rest) = inner.strip_prefix("*** Begin PathDiff") {
        inner = rest.trim_start();
    }
    if let Some(rest) = inner.strip_suffix(APPLY_PATCH_END_MARKER) {
        inner = rest.trim_end();
    } else if let Some(rest) = inner.strip_suffix("*** End PathDiff") {
        inner = rest.trim_end();
    }
    let is_hashline = inner.starts_with('[')
        && inner.contains('#')
        && (inner.contains("SWAP")
            || inner.contains("DEL")
            || inner.contains("INS.")
            || inner.contains("REM")
            || inner.contains("MV "));
    let has_pathdiff = inner.contains("*** Add File:")
        || inner.contains("*** Update File:")
        || inner.contains("*** Delete File:");
    if is_hashline && !has_pathdiff {
        Some(inner.to_owned())
    } else {
        None
    }
}

fn strip_hashline_headers_inside_update_file(trimmed: &str) -> String {
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_was_update = false;
    for segment in trimmed.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']).trim();
        if prev_was_update && is_hashline_header(line) {
            prev_was_update = false;
            continue;
        }
        if line.starts_with("*** Update File:") {
            prev_was_update = true;
        } else if !line.is_empty() {
            prev_was_update = false;
        }
        result.push_str(segment);
    }
    result
}

/// 剥掉模型反射式加在 Hashline 头里的 apply_patch 噪声。
///
/// 模型常把上一套补丁格式的习惯带过来，写出 `[*** Update File:src/a.ts#1A2B3C]`
/// 这类杂交头。若不修复，解析器只能给出泛化的「头格式非法」，模型无从得知真实原因。
/// 这里按真实观测到的变体做**保守**修复：只有当方括号内确实剥离出噪声关键字块、
/// 且剥离后仍有内容时才改写，其余一律原样保留交给解析器硬失败。
fn strip_apply_patch_header_noise(input: &str) -> String {
    if !input.contains('[') {
        return input.to_owned();
    }
    let mut changed = false;
    let mut output = String::with_capacity(input.len());
    for segment in input.split_inclusive('\n') {
        let content = line_content(segment);
        match repair_header_line(content) {
            Some(repaired) => {
                changed = true;
                output.push_str(&repaired);
                output.push_str(&segment[content.len()..]);
            }
            None => output.push_str(segment),
        }
    }
    if changed { output } else { input.to_owned() }
}

/// 修复单个方括号头行；无可剥离噪声时返回 `None`。
fn repair_header_line(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    let cleaned = strip_apply_patch_path_noise(inner);
    if cleaned == inner.trim() || cleaned.is_empty() {
        return None;
    }
    let prefix_len = raw.len() - raw.trim_start().len();
    let mut repaired = String::with_capacity(raw.len());
    repaired.push_str(&raw[..prefix_len]);
    repaired.push('[');
    repaired.push_str(cleaned);
    repaired.push(']');
    Some(repaired)
}

/// 剥离头部的 `***` 与 `Update File:` / `Add File:` / `Move to:` 等关键字块。
fn strip_apply_patch_path_noise(text: &str) -> &str {
    let mut rest = strip_leading_stars(text.trim_start());
    if let Some(after) = strip_header_keyword_block(rest) {
        rest = strip_leading_stars(after.trim_start());
    }
    rest
}

fn strip_leading_stars(text: &str) -> &str {
    let stripped = text.trim_start_matches('*');
    if stripped.len() == text.len() {
        text
    } else {
        stripped.trim_start()
    }
}

/// 匹配 `(update|add|delete|move)[分隔符]*(file|to)?[分隔符]*:` 前缀，返回冒号之后的部分。
fn strip_header_keyword_block(text: &str) -> Option<&str> {
    const KEYWORDS: [&str; 4] = ["update", "add", "delete", "move"];
    let lower = text.to_ascii_lowercase();
    let keyword_len = KEYWORDS
        .iter()
        .find(|keyword| lower.starts_with(**keyword))?
        .len();
    let colon = text[keyword_len..].find(':')? + keyword_len;
    if !is_header_keyword_separator(&text[keyword_len..colon]) {
        return None;
    }
    Some(&text[colon + 1..])
}

/// 关键字与冒号之间只允许分隔符与 `file`/`to` 两个可选词。
fn is_header_keyword_separator(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let stripped = lower.replace("file", "").replace("to", "");
    stripped
        .chars()
        .all(|character| !character.is_ascii_alphanumeric())
}

fn normalize_lazy_edit_placeholders(input: &str) -> String {
    if !input.contains(LAZY_EDIT_SLASH_PLACEHOLDER) && !input.contains(LAZY_EDIT_HASH_PLACEHOLDER) {
        return input.to_owned();
    }

    let mut normalized = String::with_capacity(input.len());
    let mut in_update_file = false;
    for segment in input.split_inclusive('\n') {
        let newline_len = if segment.ends_with("\r\n") {
            2
        } else if segment.ends_with('\n') {
            1
        } else {
            0
        };
        let content_len = segment.len() - newline_len;
        let raw_line = &segment[..content_len];
        let line = raw_line.trim();
        let has_pathdiff_prefix = raw_line.starts_with([' ', '+', '-']);
        if line.starts_with(UPDATE_FILE_MARKER) {
            in_update_file = true;
        } else if line.starts_with("*** ") {
            in_update_file = false;
        }

        if in_update_file && !has_pathdiff_prefix && is_lazy_edit_placeholder(line) {
            normalized.push_str(EMPTY_CHANGE_CONTEXT_MARKER);
            normalized.push_str(&segment[content_len..]);
        } else {
            normalized.push_str(segment);
        }
    }
    normalized
}

fn is_lazy_edit_placeholder(line: &str) -> bool {
    line == LAZY_EDIT_SLASH_PLACEHOLDER || line == LAZY_EDIT_HASH_PLACEHOLDER
}

/// 补正 Lazy Edit 块内漏写行前缀的正文行。
///
/// 模型在 Lazy Edit 语义下常直接抄写原文而漏掉行首前缀，解析器会在该块首个正文行处硬失败。
/// 仅当整个 Update 块内存在独立 Lazy 边界、且该行不是 Lazy 占位符、变更上下文标记、代码围栏、
/// Hashline 头或 shell 命令原文时，才补一个空格按上下文行处理；其余行原样保留继续由解析器硬失败，
/// 以免吞掉「把命令移出 PathDiff」的定向诊断。
fn pad_lazy_update_block_lines(input: &str) -> String {
    let segments: Vec<&str> = input.split_inclusive('\n').collect();
    let mut padded = String::with_capacity(input.len());
    let mut index = 0;
    while index < segments.len() {
        if !is_update_file_header(segments[index]) {
            padded.push_str(segments[index]);
            index += 1;
            continue;
        }
        padded.push_str(segments[index]);
        let block_end = update_block_end(&segments, index + 1);
        let body = &segments[index + 1..block_end];
        let lazy_boundary = body
            .iter()
            .any(|segment| is_lazy_edit_placeholder(line_content(segment).trim()));
        for segment in body {
            if lazy_boundary && needs_context_prefix(line_content(segment)) {
                padded.push(' ');
            }
            padded.push_str(segment);
        }
        index = block_end;
    }
    padded
}

/// 判断是否为 `*** Update File:` 块头（容忍行首空白）。
fn is_update_file_header(segment: &str) -> bool {
    line_content(segment).trim().starts_with(UPDATE_FILE_MARKER)
}

/// 返回块正文的结束下标：下一个 `*** ` 结构标记或输入末尾。
fn update_block_end(segments: &[&str], start: usize) -> usize {
    segments[start..]
        .iter()
        .position(|segment| line_content(segment).trim().starts_with("*** "))
        .map_or(segments.len(), |offset| start + offset)
}

/// 判断漏写前缀的正文行是否应补成上下文行。
fn needs_context_prefix(line: &str) -> bool {
    if line.trim().is_empty() || line.starts_with([' ', '+', '-']) {
        return false;
    }
    let trimmed = line.trim();
    !is_lazy_edit_placeholder(trimmed)
        && !trimmed.starts_with("@@")
        && !trimmed.starts_with("```")
        && !is_hashline_header(trimmed)
        && !is_shell_command_line(trimmed)
}

/// 判断是否为误入 Update 块的 shell / CLI 命令原文。
fn is_shell_command_line(line: &str) -> bool {
    if ["$ ", "> ", "PS "]
        .iter()
        .any(|prompt| line.starts_with(prompt))
    {
        return true;
    }
    line.split_whitespace()
        .next()
        .is_some_and(|token| SHELL_COMMAND_TOKENS.contains(&token))
}

/// 去除单行末尾的换行符（兼容 CRLF）。
fn line_content(segment: &str) -> &str {
    segment.trim_end_matches(['\r', '\n'])
}

/// 判断是否为 Hashline 头（`[path#TAG]`）。
fn is_hashline_header(line: &str) -> bool {
    line.starts_with('[') && line.ends_with(']') && line.contains('#')
}

fn strip_markdown_code_fence(input: &str) -> &str {
    let trimmed = input.trim();
    if (trimmed.starts_with("```diff")
        || trimmed.starts_with("```patch")
        || trimmed.starts_with("```"))
        && trimmed.ends_with("```")
        && trimmed.len() >= 6
    {
        let without_prefix = if let Some(after) = trimmed.strip_prefix("```diff") {
            after
        } else if let Some(after) = trimmed.strip_prefix("```patch") {
            after
        } else if let Some(after) = trimmed.strip_prefix("```") {
            after
        } else {
            trimmed
        };
        let without_suffix = without_prefix.strip_suffix("```").unwrap_or(without_prefix);
        without_suffix.trim()
    } else {
        trimmed
    }
}

/// PathDiff models occasionally omit the conventional `+` marker in a new-file body.
fn normalize_add_file_lines(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    let mut in_add_file = false;
    for segment in input.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']);
        if line.starts_with("*** Add File:") {
            in_add_file = true;
        } else if line.starts_with("*** ") {
            in_add_file = false;
        }
        if in_add_file && !line.starts_with("*** Add File:") && !line.starts_with('+') {
            normalized.push('+');
        }
        normalized.push_str(segment);
    }
    normalized
}
