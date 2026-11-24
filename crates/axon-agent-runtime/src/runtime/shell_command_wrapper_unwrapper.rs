//! shell_command 外部包装（如 pwsh/powershell/bash/sh/zsh/cmd/wsl 等）智能解包与裸脚本归一化。

/// 识别并剥除可能存在的多层外层 shell wrapper，将其归一化为纯净的裸脚本。
pub(crate) fn unwrap_outer_shell_wrapper(script: &str) -> String {
    let (_, command) = extract_windows_cd_prefix(script);
    let mut current = command;
    for _ in 0..3 {
        let Some(unwrapped) = unwrap_single_shell_wrapper(&current) else {
            break;
        };
        let candidate = unwrapped.trim();
        if candidate.is_empty() || candidate == current {
            break;
        }
        current = candidate.to_owned();
    }
    current
}

/// 提取命令开头的 `cd /d <path> &&` 前缀，并返回工作目录与剩余脚本。
pub(crate) fn extract_windows_cd_prefix(script: &str) -> (Option<String>, String) {
    let trimmed = strip_markdown_code_fence(script).trim();
    let Some(after_cd) = consume_keyword(trimmed, "cd") else {
        return (None, trimmed.to_owned());
    };
    let Some(after_drive_flag) = consume_keyword(after_cd.trim_start(), "/d") else {
        return (None, trimmed.to_owned());
    };
    let Some((raw_path, command)) = after_drive_flag.split_once("&&") else {
        return (None, trimmed.to_owned());
    };
    let path = raw_path.trim().trim_matches(['"', '\'']).trim();
    if path.is_empty() || command.trim().is_empty() {
        return (None, trimmed.to_owned());
    }
    (Some(path.to_owned()), command.trim().to_owned())
}

fn consume_keyword<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    input
        .get(..keyword.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(keyword))
        .filter(|_| input[keyword.len()..].starts_with(char::is_whitespace))
        .map(|_| &input[keyword.len()..])
}

fn strip_markdown_code_fence(input: &str) -> &str {
    let trimmed = input.trim();
    if trimmed.starts_with("```") && trimmed.ends_with("```") && trimmed.len() >= 6 {
        let without_prefix = trimmed
            .strip_prefix("```powershell")
            .or_else(|| trimmed.strip_prefix("```pwsh"))
            .or_else(|| trimmed.strip_prefix("```ps1"))
            .or_else(|| trimmed.strip_prefix("```bash"))
            .or_else(|| trimmed.strip_prefix("```zsh"))
            .or_else(|| trimmed.strip_prefix("```sh"))
            .or_else(|| trimmed.strip_prefix("```cmd"))
            .or_else(|| trimmed.strip_prefix("```shell"))
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        let without_suffix = without_prefix.strip_suffix("```").unwrap_or(without_prefix);
        without_suffix.trim()
    } else {
        trimmed
    }
}

fn unwrap_single_shell_wrapper(script: &str) -> Option<String> {
    let mut trimmed = script.trim();
    if trimmed.ends_with(';') {
        trimmed = trimmed[..trimmed.len() - 1].trim();
    }
    if trimmed.starts_with('"') || trimmed.starts_with('\'') || trimmed.starts_with('{') {
        return None;
    }
    let (program_raw, after_prog) = trimmed.split_once(char::is_whitespace)?;
    let program = shell_program_name(program_raw)?;
    match program.as_str() {
        "bash" | "sh" | "zsh" => unwrap_posix_shell_wrapper(after_prog),
        "pwsh" | "powershell" => unwrap_powershell_wrapper(after_prog),
        "cmd" => unwrap_cmd_wrapper(after_prog),
        "wsl" => unwrap_wsl_wrapper(after_prog),
        _ => None,
    }
}

fn unwrap_posix_shell_wrapper(args_str: &str) -> Option<String> {
    let mut rest = args_str.trim_start();
    while rest.starts_with('-') {
        let (flag, after_flag) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let flag_lower = flag.to_ascii_lowercase();
        if matches!(flag_lower.as_str(), "-c" | "-lc" | "-cl") {
            return extract_script_body(after_flag);
        }
        rest = after_flag.trim_start();
    }
    None
}

fn unwrap_powershell_wrapper(args_str: &str) -> Option<String> {
    let mut rest = args_str.trim_start();
    while !rest.is_empty() {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('-') && !trimmed.starts_with('/') {
            break;
        }

        if let Some(body) = strip_powershell_command_prefix(trimmed) {
            return extract_script_body(body);
        }

        let (flag, after_flag) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        let flag_clean = flag.trim_start_matches(['-', '/']).to_ascii_lowercase();
        let (flag_name, flag_has_colon_val) = match flag_clean.split_once(':') {
            Some((name, _)) => (name, true),
            None => (flag_clean.as_str(), false),
        };

        if matches!(
            flag_name,
            "executionpolicy"
                | "ep"
                | "inputformat"
                | "if"
                | "outputformat"
                | "of"
                | "version"
                | "v"
                | "windowstyle"
                | "w"
                | "configurationname"
                | "config"
                | "file"
                | "f"
        ) {
            if flag_has_colon_val {
                rest = after_flag;
            } else {
                let (_, after_val) = after_flag
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .unwrap_or((after_flag, ""));
                rest = after_val;
            }
            continue;
        }

        rest = after_flag;
    }
    None
}

fn strip_powershell_command_prefix(trimmed: &str) -> Option<&str> {
    for prefix in ["-command:", "/command:", "-c:", "/c:"] {
        if trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return Some(&trimmed[prefix.len()..]);
        }
    }
    for prefix in ["-command", "/command", "-c", "/c"] {
        if trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
            let after = &trimmed[prefix.len()..];
            if after.starts_with(char::is_whitespace) {
                return Some(after.trim_start());
            }
        }
    }
    None
}

fn unwrap_cmd_wrapper(args_str: &str) -> Option<String> {
    let trimmed = args_str.trim_start();
    for prefix in ["/c:", "-c:", "/k:", "-k:"] {
        if trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return extract_script_body(&trimmed[prefix.len()..]);
        }
    }
    for prefix in ["/c", "-c", "/k", "-k"] {
        if trimmed.len() >= prefix.len() && trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
            let after = &trimmed[prefix.len()..];
            if after.starts_with(char::is_whitespace) {
                return extract_script_body(after.trim_start());
            }
        }
    }
    None
}

fn unwrap_wsl_wrapper(args_str: &str) -> Option<String> {
    let mut rest = args_str.trim_start();
    while !rest.is_empty() {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('-') {
            return unwrap_single_shell_wrapper(trimmed);
        }
        let (flag, after_flag) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        let flag_lower = flag.to_ascii_lowercase();
        if matches!(flag_lower.as_str(), "-e" | "--exec") {
            return unwrap_single_shell_wrapper(after_flag.trim_start())
                .or_else(|| extract_script_body(after_flag));
        }
        if matches!(
            flag_lower.as_str(),
            "-d" | "--distribution" | "-u" | "--user" | "--cd"
        ) {
            let (_, after_val) = after_flag
                .trim_start()
                .split_once(char::is_whitespace)
                .unwrap_or((after_flag, ""));
            rest = after_val;
            continue;
        }
        rest = after_flag;
    }
    None
}

fn extract_script_body(raw: &str) -> Option<String> {
    let mut trimmed = raw.trim();
    if trimmed.ends_with(';') {
        trimmed = trimmed[..trimmed.len() - 1].trim();
    }
    if trimmed.is_empty() {
        return None;
    }
    if let Some(content) = strip_enclosing_quotes(trimmed, '\'') {
        return Some(content.replace("''", "'"));
    }
    if let Some(content) = strip_enclosing_quotes(trimmed, '"') {
        return Some(
            content
                .replace(r#"\""#, "\"")
                .replace("\"\"", "\"")
                .replace(r#"\$"#, "$"),
        );
    }
    if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() >= 2 {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        return Some(inner.to_owned());
    }
    Some(trimmed.to_owned())
}

fn strip_enclosing_quotes(s: &str, quote: char) -> Option<&str> {
    if s.starts_with(quote) && s.ends_with(quote) && s.len() >= 2 {
        Some(&s[1..s.len() - 1])
    } else {
        None
    }
}

fn shell_program_name(program: &str) -> Option<String> {
    std::path::Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_outer_shell_wrappers_correctly() {
        assert_eq!(
            unwrap_outer_shell_wrapper("pwsh -Command 'Get-ChildItem'"),
            "Get-ChildItem"
        );
        assert_eq!(
            unwrap_outer_shell_wrapper("powershell -NoLogo -NoProfile -Command \"git status\""),
            "git status"
        );
        assert_eq!(unwrap_outer_shell_wrapper("bash -lc 'pwd; ls'"), "pwd; ls");
        assert_eq!(
            unwrap_outer_shell_wrapper("sh -c \"echo hello\""),
            "echo hello"
        );
        assert_eq!(
            unwrap_outer_shell_wrapper("cargo test --workspace"),
            "cargo test --workspace"
        );
        let multiline_pwsh = "pwsh -Command '\n$files = Get-ChildItem -Path crates -Recurse -Filter \"*.rs\"\n$results = @()\n\nforeach ($file in $files) {\n    $results += $file\n}\n'";
        let unwrapped_multiline = unwrap_outer_shell_wrapper(multiline_pwsh);
        assert!(unwrapped_multiline.starts_with("$files = Get-ChildItem"));
        assert!(unwrapped_multiline.contains("foreach ($file in $files)"));
        assert!(!unwrapped_multiline.contains("pwsh -Command"));

        assert_eq!(
            unwrap_outer_shell_wrapper("pwsh -Command { Get-ChildItem -Path crates }"),
            "Get-ChildItem -Path crates"
        );
        assert_eq!(
            unwrap_outer_shell_wrapper("cmd /c \"cargo check\""),
            "cargo check"
        );
        assert_eq!(
            unwrap_outer_shell_wrapper("powershell /c:\"git status\""),
            "git status"
        );
        assert_eq!(
            unwrap_outer_shell_wrapper("pwsh -ExecutionPolicy Bypass -Command 'Get-Date'"),
            "Get-Date"
        );
        assert_eq!(
            unwrap_outer_shell_wrapper("```powershell\npwsh -Command \"Get-Date\"\n```"),
            "Get-Date"
        );
        assert_eq!(
            unwrap_outer_shell_wrapper("```bash\ncargo test\n```"),
            "cargo test"
        );
    }

    #[test]
    fn extracts_windows_cd_prefix_before_unwrapping_shell_wrapper() {
        let (cwd, command) = extract_windows_cd_prefix(
            r#"cd /d E:\zbg\axon-code && powershell -Command "Get-Content README.md""#,
        );

        assert_eq!(cwd.as_deref(), Some(r#"E:\zbg\axon-code"#));
        assert_eq!(command, r#"powershell -Command "Get-Content README.md""#);
        assert_eq!(
            unwrap_outer_shell_wrapper(&format!("cd /d E:\\zbg\\axon-code && {command}")),
            "Get-Content README.md"
        );
    }
}
