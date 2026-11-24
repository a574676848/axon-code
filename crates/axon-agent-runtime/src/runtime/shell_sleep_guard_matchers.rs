//! 各平台与脚本形态的「纯休眠占位」命令匹配器。

use super::ShellToken;

pub(super) fn is_duration_literal(token: &str) -> bool {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('$')
        && trimmed.len() > 1
        && trimmed[1..]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return true;
    }
    let core = trimmed
        .strip_suffix(['s', 'm', 'h', 'd', 'S', 'M', 'H', 'D'])
        .unwrap_or(trimmed);
    !core.is_empty()
        && core.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && core.chars().any(|ch| ch.is_ascii_digit())
}

pub(super) fn is_pure_posix_sleep(args: &[ShellToken]) -> bool {
    !args.is_empty() && args.iter().all(|arg| is_duration_literal(&arg.text))
}

pub(super) fn is_pure_start_sleep(args: &[ShellToken]) -> bool {
    let mut has_duration = false;
    let mut index = 0;
    while index < args.len() {
        let lower = args[index].text.to_ascii_lowercase();
        if matches!(lower.as_str(), "-s" | "-seconds" | "-m" | "-milliseconds") {
            let Some(next) = args.get(index + 1) else {
                return false;
            };
            if !is_duration_literal(&next.text) {
                return false;
            }
            has_duration = true;
            index += 2;
            continue;
        }
        if let Some(val) = lower
            .strip_prefix("-seconds:")
            .or_else(|| lower.strip_prefix("-s:"))
            .or_else(|| lower.strip_prefix("-milliseconds:"))
            .or_else(|| lower.strip_prefix("-m:"))
        {
            if !is_duration_literal(val) {
                return false;
            }
            has_duration = true;
            index += 1;
            continue;
        }
        if is_duration_literal(&args[index].text) {
            has_duration = true;
            index += 1;
            continue;
        }
        return false;
    }
    has_duration
}

pub(super) fn is_pure_windows_timeout(args: &[ShellToken]) -> bool {
    if args.is_empty() {
        return false;
    }
    let mut saw_timeout_flag_or_num = false;
    let mut index = 0;
    while index < args.len() {
        let lower = args[index].text.to_ascii_lowercase();
        if lower == "/nobreak" {
            index += 1;
            continue;
        }
        if lower == "/t" {
            let Some(val) = args.get(index + 1) else {
                return false;
            };
            if !is_duration_literal(&val.text) {
                return false;
            }
            saw_timeout_flag_or_num = true;
            index += 2;
            continue;
        }
        if let Some(val) = lower.strip_prefix("/t:") {
            if !is_duration_literal(val) {
                return false;
            }
            saw_timeout_flag_or_num = true;
            index += 1;
            continue;
        }
        if !saw_timeout_flag_or_num && is_duration_literal(&args[index].text) {
            saw_timeout_flag_or_num = true;
            index += 1;
            continue;
        }
        // 若存在任何后续子命令（如 Linux `timeout 30 cargo test`），一律放行。
        return false;
    }
    saw_timeout_flag_or_num
}

pub(super) fn is_loopback_ping(args: &[ShellToken]) -> bool {
    let mut targets = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let raw = args[index].text.as_str();
        let lower = raw.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "-n" | "-c" | "-w" | "-l" | "-i" | "-s" | "-p" | "-f" | "-j" | "-k" | "-r" | "-v"
        ) {
            if index + 1 >= args.len() {
                return false;
            }
            index += 2;
            continue;
        }
        if raw.starts_with('-') || raw.starts_with('/') {
            index += 1;
            continue;
        }
        targets.push(lower);
        index += 1;
    }
    targets.len() == 1
        && matches!(
            targets[0].as_str(),
            "127.0.0.1" | "localhost" | "::1" | "[::1]"
        )
}

pub(super) fn is_dotnet_thread_sleep(compact: &str) -> bool {
    let lower: String = compact
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let Some(inner) = lower
        .strip_prefix("[threading.thread]::sleep(")
        .or_else(|| lower.strip_prefix("[system.threading.thread]::sleep("))
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    !inner.is_empty() && !inner.contains(['(', ')', ';']) && is_duration_literal(inner)
}

pub(super) fn is_pure_wait_event_timeout(args: &[ShellToken]) -> bool {
    let mut has_timeout = false;
    for arg in args {
        let lower = arg.text.to_ascii_lowercase();
        if lower.starts_with("-source") {
            return false;
        }
        if lower == "-timeout" || lower.starts_with("-timeout:") {
            has_timeout = true;
        }
    }
    has_timeout
}

pub(super) fn is_pure_python_sleep(args: &[ShellToken]) -> bool {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index].text;
        if arg == "-c" {
            let Some(code) = args.get(index + 1) else {
                return false;
            };
            return index + 2 == args.len() && is_python_sleep_body(&code.text);
        }
        if matches!(arg.as_str(), "-u" | "-q" | "-B" | "-E" | "-s" | "-S" | "-O") {
            index += 1;
            continue;
        }
        return false;
    }
    false
}

fn is_python_sleep_body(code: &str) -> bool {
    let mut saw_sleep = false;
    for line in code.split(['\n', '\r', ';']) {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() || trimmed == "pass" {
            continue;
        }
        if matches!(
            trimmed,
            "import time" | "from time import sleep" | "from time import *"
        ) {
            continue;
        }
        let compact: String = trimmed.chars().filter(|ch| !ch.is_whitespace()).collect();
        let inner = compact
            .strip_prefix("time.sleep(")
            .or_else(|| compact.strip_prefix("sleep("))
            .and_then(|rest| rest.strip_suffix(')'));
        if let Some(arg) = inner
            && is_simple_numeric_expr(arg)
        {
            saw_sleep = true;
            continue;
        }
        return false;
    }
    saw_sleep
}

pub(super) fn is_pure_node_sleep(args: &[ShellToken]) -> bool {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index].text;
        if matches!(arg.as_str(), "-e" | "--eval") {
            let Some(code) = args.get(index + 1) else {
                return false;
            };
            return index + 2 == args.len() && is_node_sleep_body(&code.text);
        }
        if let Some(code) = arg.strip_prefix("--eval=") {
            return index + 1 == args.len() && is_node_sleep_body(code);
        }
        if matches!(arg.as_str(), "--no-warnings" | "--experimental-modules") {
            index += 1;
            continue;
        }
        return false;
    }
    false
}

fn is_node_sleep_body(code: &str) -> bool {
    let mut saw_sleep = false;
    for line in code.split(['\n', '\r', ';']) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let compact: String = trimmed.chars().filter(|ch| !ch.is_whitespace()).collect();
        if is_node_sleep_statement(&compact) {
            saw_sleep = true;
            continue;
        }
        return false;
    }
    saw_sleep
}

fn is_node_sleep_statement(compact: &str) -> bool {
    if let Some(inner) = compact
        .strip_prefix("setTimeout(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return matches_empty_callback_and_delay(inner);
    }
    let promise_body = compact.strip_prefix("await").unwrap_or(compact);
    if let Some(inner) = promise_body
        .strip_prefix("newPromise(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return matches_promise_sleep_executor(inner);
    }
    false
}

fn matches_empty_callback_and_delay(inner: &str) -> bool {
    for prefix in ["()=>{},", "()=>void0,", "()=>undefined,", "function(){},"] {
        if let Some(delay) = inner.strip_prefix(prefix) {
            return is_simple_numeric_expr(delay);
        }
    }
    false
}

fn matches_promise_sleep_executor(inner: &str) -> bool {
    let Some((param_part, body)) = inner.split_once("=>") else {
        return false;
    };
    let resolver = param_part.trim_matches(['(', ')']);
    if resolver.is_empty()
        || !resolver
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return false;
    }
    let body_trimmed = body.trim_matches(['{', '}']);
    let Some(args) = body_trimmed
        .strip_prefix("setTimeout(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let Some((target, delay)) = args.split_once(',') else {
        return false;
    };
    target == resolver && is_simple_numeric_expr(delay)
}

fn is_simple_numeric_expr(expr: &str) -> bool {
    !expr.is_empty()
        && expr
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '+' | '-' | '*' | '/'))
        && expr.chars().any(|ch| ch.is_ascii_digit())
}
