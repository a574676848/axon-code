//! 上下文感知的「纯休眠占位」命令检测。
//!
//! 当当前会话存在尚未结束的后台作业且模型试图通过前台 `shell_command` 运行纯休眠命令
//! （如 `sleep`、`Start-Sleep`、`timeout /t`、`ping -n N 127.0.0.1 >nul` 等）拖延空等时，
//! 本模块拦截该占位调用；正常的复合脚本（如休眠后紧跟健康检查、循环探测、条件等待或
//! 带子命令的 Linux `timeout`）一律放行，避免误杀。

#[path = "shell_sleep_guard_matchers.rs"]
mod matchers;

use matchers::{
    is_dotnet_thread_sleep, is_duration_literal, is_loopback_ping, is_pure_node_sleep,
    is_pure_posix_sleep, is_pure_python_sleep, is_pure_start_sleep, is_pure_wait_event_timeout,
    is_pure_windows_timeout,
};

use crate::builtin_shell_script::{
    POWERSHELL_UTF8_PREFIX, is_canonical_powershell_script_command, is_canonical_wsl_script_command,
};

/// 检测到的纯休眠占位模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PureSleepPlaceholder {
    /// 命中的代表性休眠方式说明。
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellToken {
    text: String,
    quoted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClauseKind {
    EmptyOrAssignment,
    HarmlessFiller,
    PureSleep(&'static str),
    RealCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteMode {
    None,
    Single,
    Double,
}

/// 从 Axon 规范化后的 shell 命令 argv 提取原始裸脚本文本。
#[must_use]
pub fn extract_raw_script_from_command(command: &[String]) -> Option<&str> {
    let last = command.last()?.as_str();
    if is_canonical_powershell_script_command(command) {
        Some(last.strip_prefix(POWERSHELL_UTF8_PREFIX).unwrap_or(last))
    } else if is_canonical_wsl_script_command(command) || is_canonical_posix_wrapper(command) {
        Some(last)
    } else {
        None
    }
}

/// 从规范化后的 shell 命令 argv 检测纯休眠占位。
#[must_use]
pub fn detect_pure_sleep_in_command(command: &[String]) -> Option<PureSleepPlaceholder> {
    if let Some(script) = extract_raw_script_from_command(command) {
        return detect_pure_sleep_placeholder(script);
    }
    if command.is_empty() {
        return None;
    }
    let tokens = command
        .iter()
        .map(|part| ShellToken {
            text: part.clone(),
            quoted: false,
        })
        .collect::<Vec<_>>();
    match classify_tokens(&tokens, false) {
        ClauseKind::PureSleep(reason) => Some(PureSleepPlaceholder { reason }),
        ClauseKind::EmptyOrAssignment | ClauseKind::HarmlessFiller | ClauseKind::RealCommand => {
            None
        }
    }
}

/// 判断一段裸脚本是否全部由纯休眠占位（及无副作用空输出）构成。
#[must_use]
pub fn detect_pure_sleep_placeholder(raw_script: &str) -> Option<PureSleepPlaceholder> {
    let clauses = split_clauses_quote_aware(raw_script)?;
    let mut first_sleep_reason: Option<&'static str> = None;
    for clause in clauses {
        match classify_clause(&clause) {
            ClauseKind::EmptyOrAssignment | ClauseKind::HarmlessFiller => {}
            ClauseKind::PureSleep(reason) => {
                if first_sleep_reason.is_none() {
                    first_sleep_reason = Some(reason);
                }
            }
            ClauseKind::RealCommand => return None,
        }
    }
    first_sleep_reason.map(|reason| PureSleepPlaceholder { reason })
}

fn is_canonical_posix_wrapper(command: &[String]) -> bool {
    if command.len() != 3 {
        return false;
    }
    let Some(stem) = std::path::Path::new(&command[0])
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    matches!(stem.as_str(), "bash" | "sh" | "zsh") && matches!(command[1].as_str(), "-c" | "-lc")
}

/// 引号感知切分：出现命令替换、脚本块或未闭合引号时返回 `None`（保守放行）。
fn split_clauses_quote_aware(script: &str) -> Option<Vec<String>> {
    let mut clauses = Vec::new();
    let mut current = String::new();
    let mut mode = QuoteMode::None;
    let bytes = script.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        let ch = script[index..].chars().next()?;
        let ch_len = ch.len_utf8();
        match mode {
            QuoteMode::Single => {
                current.push(ch);
                if ch == '\'' {
                    mode = QuoteMode::None;
                }
                index += ch_len;
            }
            QuoteMode::Double => {
                current.push(ch);
                if matches!(ch, '\\' | '`') && index + ch_len < bytes.len() {
                    let next = script[index + ch_len..].chars().next()?;
                    current.push(next);
                    index += ch_len + next.len_utf8();
                    continue;
                }
                if (ch == '$' && bytes.get(index + ch_len) == Some(&b'(')) || ch == '`' {
                    return None;
                }
                if ch == '"' {
                    mode = QuoteMode::None;
                }
                index += ch_len;
            }
            QuoteMode::None => {
                if ch == '\'' {
                    mode = QuoteMode::Single;
                    current.push(ch);
                    index += ch_len;
                    continue;
                }
                if ch == '"' {
                    mode = QuoteMode::Double;
                    current.push(ch);
                    index += ch_len;
                    continue;
                }
                if ch == '#' && is_comment_start(&current) {
                    while index < bytes.len() && bytes[index] != b'\n' {
                        index += 1;
                    }
                    continue;
                }
                if ch == '`' || matches!(ch, '{' | '}') {
                    return None;
                }
                if matches!(ch, '$' | '<' | '>') && bytes.get(index + ch_len) == Some(&b'(') {
                    return None;
                }
                if matches!(ch, ';' | '\n' | '\r') {
                    clauses.push(std::mem::take(&mut current));
                    index += ch_len;
                    continue;
                }
                if ch == '&' {
                    if is_stream_merge_ampersand(&current, bytes.get(index + 1).copied()) {
                        current.push(ch);
                        index += 1;
                        continue;
                    }
                    clauses.push(std::mem::take(&mut current));
                    index += if bytes.get(index + 1) == Some(&b'&') {
                        2
                    } else {
                        1
                    };
                    continue;
                }
                if ch == '|' && bytes.get(index + 1) == Some(&b'|') {
                    clauses.push(std::mem::take(&mut current));
                    index += 2;
                    continue;
                }
                current.push(ch);
                index += ch_len;
            }
        }
    }

    if mode != QuoteMode::None {
        return None;
    }
    if !current.trim().is_empty() {
        clauses.push(current);
    }
    Some(clauses)
}

fn is_comment_start(current: &str) -> bool {
    current.is_empty() || current.chars().last().is_some_and(char::is_whitespace)
}

fn is_stream_merge_ampersand(before: &str, after: Option<u8>) -> bool {
    before.ends_with('>') || after == Some(b'>')
}

fn classify_clause(clause: &str) -> ClauseKind {
    let trimmed = clause.trim();
    if trimmed.is_empty() {
        return ClauseKind::EmptyOrAssignment;
    }
    let Some(parsed) = parse_clause_tokens(trimmed) else {
        return ClauseKind::RealCommand;
    };
    if parsed.has_unsafe_redirection || parsed.has_real_pipe {
        return ClauseKind::RealCommand;
    }
    if parsed.tokens.is_empty() {
        return ClauseKind::EmptyOrAssignment;
    }
    classify_tokens(&parsed.tokens, parsed.has_null_output)
}

struct ParsedClause {
    tokens: Vec<ShellToken>,
    has_null_output: bool,
    has_unsafe_redirection: bool,
    has_real_pipe: bool,
}

fn parse_clause_tokens(clause: &str) -> Option<ParsedClause> {
    let raw_tokens = tokenize_with_operators(clause)?;
    let mut tokens = Vec::new();
    let mut has_null_output = false;
    let mut index = 0;

    while index < raw_tokens.len() {
        let token = &raw_tokens[index];
        if !token.quoted && token.text == "|" {
            if is_trailing_out_null(&raw_tokens[index + 1..]) {
                has_null_output = true;
                break;
            }
            return Some(ParsedClause {
                tokens,
                has_null_output,
                has_unsafe_redirection: false,
                has_real_pipe: true,
            });
        }
        if !token.quoted && token.text == "<" {
            return Some(ParsedClause {
                tokens,
                has_null_output,
                has_unsafe_redirection: true,
                has_real_pipe: false,
            });
        }
        if !token.quoted && is_stream_merge_token(&token.text) {
            index += 1;
            continue;
        }
        if !token.quoted && is_output_redirect_operator(&token.text) {
            let target = raw_tokens.get(index + 1)?;
            if is_null_sink_target(&target.text) {
                has_null_output = true;
                index += 2;
                continue;
            }
            return Some(ParsedClause {
                tokens,
                has_null_output,
                has_unsafe_redirection: true,
                has_real_pipe: false,
            });
        }
        tokens.push(token.clone());
        index += 1;
    }

    Some(ParsedClause {
        tokens,
        has_null_output,
        has_unsafe_redirection: false,
        has_real_pipe: false,
    })
}

fn is_trailing_out_null(remaining: &[ShellToken]) -> bool {
    let Some(first) = remaining.first() else {
        return false;
    };
    if first.quoted || !first.text.eq_ignore_ascii_case("out-null") {
        return false;
    }
    remaining[1..]
        .iter()
        .all(|token| !token.quoted && is_stream_merge_token(&token.text))
}

fn is_stream_merge_token(token: &str) -> bool {
    matches!(token, "2>&1" | "1>&2" | ">&2" | ">&1")
}

fn is_output_redirect_operator(token: &str) -> bool {
    matches!(
        token,
        ">" | ">>" | "1>" | "1>>" | "2>" | "2>>" | "*>" | "*>>" | "&>" | "&>>"
    )
}

fn is_null_sink_target(target: &str) -> bool {
    matches!(
        target.trim().to_ascii_lowercase().as_str(),
        "nul" | "$null" | "/dev/null"
    )
}

fn tokenize_with_operators(input: &str) -> Option<Vec<ShellToken>> {
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut quoted = false;
    let mut mode = QuoteMode::None;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    let flush_buf = |tokens: &mut Vec<ShellToken>, buf: &mut String, quoted: &mut bool| {
        if !buf.is_empty() || *quoted {
            tokens.push(ShellToken {
                text: std::mem::take(buf),
                quoted: *quoted,
            });
            *quoted = false;
        }
    };

    while i < chars.len() {
        let ch = chars[i];
        match mode {
            QuoteMode::Single => {
                if ch == '\'' {
                    mode = QuoteMode::None;
                } else {
                    buf.push(ch);
                }
                i += 1;
            }
            QuoteMode::Double => {
                if matches!(ch, '\\' | '`') && i + 1 < chars.len() {
                    let next = chars[i + 1];
                    if matches!(next, '"' | '\\' | '`' | '$') {
                        buf.push(next);
                        i += 2;
                        continue;
                    }
                }
                if ch == '"' {
                    mode = QuoteMode::None;
                } else {
                    buf.push(ch);
                }
                i += 1;
            }
            QuoteMode::None => {
                if ch.is_whitespace() {
                    flush_buf(&mut tokens, &mut buf, &mut quoted);
                    i += 1;
                    continue;
                }
                if ch == '\'' {
                    mode = QuoteMode::Single;
                    quoted = true;
                    i += 1;
                    continue;
                }
                if ch == '"' {
                    mode = QuoteMode::Double;
                    quoted = true;
                    i += 1;
                    continue;
                }
                if ch == '|' || ch == '<' {
                    flush_buf(&mut tokens, &mut buf, &mut quoted);
                    tokens.push(ShellToken {
                        text: ch.to_string(),
                        quoted: false,
                    });
                    i += 1;
                    continue;
                }
                if let Some((op_len, op_text)) = match_redirect_at(&chars[i..], &buf)
                    && ((op_text.starts_with(['1', '2', '*', '&']) && buf.is_empty())
                        || chars[i] == '>')
                {
                    flush_buf(&mut tokens, &mut buf, &mut quoted);
                    tokens.push(ShellToken {
                        text: op_text,
                        quoted: false,
                    });
                    i += op_len;
                    continue;
                }
                buf.push(ch);
                i += 1;
            }
        }
    }

    if mode != QuoteMode::None {
        return None;
    }
    flush_buf(&mut tokens, &mut buf, &mut quoted);
    Some(tokens)
}

fn match_redirect_at(slice: &[char], current_buf: &str) -> Option<(usize, String)> {
    if !current_buf.is_empty() {
        return if slice.starts_with(&['>', '>']) {
            Some((2, ">>".to_owned()))
        } else if slice.starts_with(&['>']) {
            Some((1, ">".to_owned()))
        } else {
            None
        };
    }
    let s4: String = slice.iter().take(4).collect();
    if matches!(s4.as_str(), "2>&1" | "1>&2") {
        return Some((4, s4));
    }
    let s3: String = slice.iter().take(3).collect();
    if matches!(s3.as_str(), "1>>" | "2>>" | "*>>" | "&>>" | ">&1" | ">&2") {
        return Some((3, s3));
    }
    let s2: String = slice.iter().take(2).collect();
    if matches!(s2.as_str(), ">>" | "1>" | "2>" | "*>" | "&>") {
        return Some((2, s2));
    }
    if slice.first() == Some(&'>') {
        return Some((1, ">".to_owned()));
    }
    None
}

fn classify_tokens(tokens: &[ShellToken], has_null_output: bool) -> ClauseKind {
    let mut slice = tokens;
    while let Some(first) = slice.first() {
        if !first.quoted && is_env_assignment(&first.text) {
            slice = &slice[1..];
            continue;
        }
        break;
    }
    if slice.is_empty() || (!slice[0].quoted && is_powershell_scalar_assignment(slice)) {
        return ClauseKind::EmptyOrAssignment;
    }
    while slice.len() >= 2
        && !slice[0].quoted
        && matches!(
            slice[0].text.to_ascii_lowercase().as_str(),
            "builtin" | "command" | "exec"
        )
        && !slice[1].text.starts_with('-')
    {
        slice = &slice[1..];
    }

    let first_lower = slice[0].text.to_ascii_lowercase();
    if !slice[0].quoted && is_control_flow_keyword(&first_lower) {
        return ClauseKind::RealCommand;
    }

    let raw_compact = slice
        .iter()
        .map(|token| token.text.as_str())
        .collect::<Vec<_>>()
        .join("");
    if is_dotnet_thread_sleep(&raw_compact) {
        return ClauseKind::PureSleep("[Threading.Thread]::Sleep");
    }

    let cmd = normalize_command_name(&slice[0].text);
    let args = &slice[1..];
    match cmd.as_str() {
        "sleep" if is_pure_posix_sleep(args) => ClauseKind::PureSleep("sleep"),
        "start-sleep" if is_pure_start_sleep(args) => ClauseKind::PureSleep("Start-Sleep"),
        "timeout" if is_pure_windows_timeout(args) => ClauseKind::PureSleep("timeout /t"),
        "ping" if has_null_output && is_loopback_ping(args) => {
            ClauseKind::PureSleep("ping localhost > null")
        }
        "wait-event" if is_pure_wait_event_timeout(args) => {
            ClauseKind::PureSleep("Wait-Event -Timeout")
        }
        "python" | "python3" | "py" if is_pure_python_sleep(args) => {
            ClauseKind::PureSleep("python -c sleep")
        }
        "node" | "nodejs" if is_pure_node_sleep(args) => {
            ClauseKind::PureSleep("node -e setTimeout")
        }
        "echo" | "write-output" | "write-host" | "printf" | "true" | ":" | "out-null" | "cd"
        | "set-location" | "push-location" => ClauseKind::HarmlessFiller,
        _ => ClauseKind::RealCommand,
    }
}

fn normalize_command_name(raw: &str) -> String {
    let file = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let lower = file.to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat"] {
        if let Some(stripped) = lower.strip_suffix(suffix) {
            return stripped.to_owned();
        }
    }
    lower
}

fn is_control_flow_keyword(token: &str) -> bool {
    matches!(
        token,
        "if" | "then"
            | "elif"
            | "else"
            | "fi"
            | "for"
            | "foreach"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "select"
            | "function"
            | "try"
            | "catch"
            | "finally"
            | "switch"
            | "trap"
    )
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_powershell_scalar_assignment(slice: &[ShellToken]) -> bool {
    slice.len() == 3
        && slice[0].text.starts_with('$')
        && slice[1].text == "="
        && (slice[2].quoted || is_duration_literal(&slice[2].text))
}

#[cfg(test)]
#[path = "shell_sleep_guard_tests.rs"]
mod tests;
