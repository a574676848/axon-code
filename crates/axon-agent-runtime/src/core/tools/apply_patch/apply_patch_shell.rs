//! apply_patch shell/heredoc 调用解析。

use crate::apply_patch_invocation::ExtractHeredocError;

/// Shell 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyPatchShell {
    Unix,
    PowerShell,
}

/// 解析 Shell 脚本参数，返回 (shell 类型, 脚本内容)。
pub(crate) fn parse_shell_script(argv: &[String]) -> Option<(ApplyPatchShell, &str)> {
    if argv.is_empty() {
        return None;
    }

    let shell = &argv[0];
    let shell_name = std::path::Path::new(shell)
        .file_stem()
        .and_then(|s| s.to_str())?;

    let shell_name_lower = shell_name.to_ascii_lowercase();

    if argv.len() >= 3 {
        let flag = &argv[1];
        let script = &argv[2];
        if let Some(shell_type) = classify_shell(&shell_name_lower, flag) {
            return Some((shell_type, script.as_str()));
        }
    }

    if matches!(shell_name_lower.as_str(), "bash" | "zsh" | "sh")
        && argv.len() >= 3
        && argv[1] == "-c"
    {
        return Some((ApplyPatchShell::Unix, argv[2].as_str()));
    }

    if matches!(shell_name_lower.as_str(), "pwsh" | "powershell")
        && let Some(script) = powershell_command_script(argv)
    {
        return Some((ApplyPatchShell::PowerShell, script));
    }

    None
}

fn powershell_command_script(argv: &[String]) -> Option<&str> {
    let command_index = argv
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("-command"))?;
    argv.get(command_index + 1).map(String::as_str)
}

fn classify_shell(shell_name: &str, flag: &str) -> Option<ApplyPatchShell> {
    match shell_name {
        "bash" | "zsh" | "sh" if flag == "-lc" || flag == "-c" => Some(ApplyPatchShell::Unix),
        "pwsh" | "powershell" if flag.eq_ignore_ascii_case("-command") => {
            Some(ApplyPatchShell::PowerShell)
        }
        _ => None,
    }
}

/// 从 Shell 脚本中提取 apply_patch 调用。
pub(crate) fn extract_apply_patch_from_shell(
    shell: ApplyPatchShell,
    script: &str,
) -> Result<(String, Option<String>), ExtractHeredocError> {
    match shell {
        ApplyPatchShell::Unix => extract_apply_patch_from_bash(script),
        ApplyPatchShell::PowerShell => extract_apply_patch_from_powershell(script),
    }
}

/// 从 bash heredoc 脚本中提取 apply_patch 的 heredoc body 和可选的 cd 路径。
///
/// 支持的形式：
/// - `apply_patch <<'PATCH'\n...\nPATCH`
/// - `apply_patch <<"PATCH"\n...\nPATCH`
/// - `apply_patch <<PATCH\n...\nPATCH`
/// - `cd <path> && apply_patch <<'PATCH'\n...\nPATCH`
pub(crate) fn extract_apply_patch_from_bash(
    src: &str,
) -> Result<(String, Option<String>), ExtractHeredocError> {
    let script = src.trim();
    let (marker, quote_style) = find_heredoc_header(script)?;

    let heredoc_start = match quote_style {
        QuoteStyle::Single => format!("<<'{marker}'"),
        QuoteStyle::Double => format!("<<\"{marker}\""),
        QuoteStyle::None => format!("<<{marker}"),
    };

    if let Some((_, body)) = script.split_once(&heredoc_start)
        && let Some((content, _)) = body.split_once(marker.as_str())
    {
        let content = content.trim();
        if content.starts_with("*** Begin Patch") {
            let leading_script = &script[..script.find(&heredoc_start).unwrap_or(0)];
            let workdir = extract_leading_cd_path(leading_script.trim());
            return Ok((content.to_string(), workdir));
        }
    }

    Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch)
}

/// 从 PowerShell here-string 管道中提取 apply_patch body。
///
/// 支持 `@'... '@ | apply_patch` 与 UTF-8 前缀后的同形态脚本。
pub(crate) fn extract_apply_patch_from_powershell(
    src: &str,
) -> Result<(String, Option<String>), ExtractHeredocError> {
    let script = strip_powershell_utf8_prefix(src.trim());
    let Some(after_start) = script.strip_prefix("@'") else {
        return Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch);
    };
    let Some((body, tail)) = after_start
        .split_once("\n'@")
        .or_else(|| after_start.split_once("\r\n'@"))
    else {
        return Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch);
    };
    let command = tail.trim_start().strip_prefix('|').map(str::trim_start);
    if !command.is_some_and(|command| command.starts_with("apply_patch")) {
        return Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch);
    }
    let body = body.trim();
    if body.starts_with("*** Begin Patch") {
        return Ok((body.to_owned(), None));
    }
    Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch)
}

fn strip_powershell_utf8_prefix(script: &str) -> &str {
    const LEGACY_UTF8_PREFIX: &str =
        "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}";
    script
        .strip_prefix(crate::builtin_shell_script::POWERSHELL_UTF8_PREFIX.trim_end())
        .or_else(|| script.strip_prefix(LEGACY_UTF8_PREFIX))
        .map(str::trim_start)
        .unwrap_or(script)
}

enum QuoteStyle {
    Single,
    Double,
    None,
}

fn find_heredoc_header(script: &str) -> Result<(String, QuoteStyle), ExtractHeredocError> {
    for line in script.lines() {
        if line.contains("<<'") {
            if let Some(after_heredoc) = line.split("<<'").nth(1)
                && let Some(marker) = after_heredoc.split('\'').next()
            {
                let marker = marker.trim();
                if !marker.is_empty() {
                    return Ok((marker.to_string(), QuoteStyle::Single));
                }
            }
        } else if line.contains("<<\"") {
            if let Some(after_heredoc) = line.split("<<\"").nth(1)
                && let Some(marker) = after_heredoc.split('"').next()
            {
                let marker = marker.trim();
                if !marker.is_empty() {
                    return Ok((marker.to_string(), QuoteStyle::Double));
                }
            }
        } else if line.contains("<<")
            && let Some(after_heredoc) = line.split("<<").nth(1)
        {
            let marker = after_heredoc
                .split_whitespace()
                .next()
                .unwrap_or(after_heredoc);
            let marker = marker.trim().trim_end_matches('\'');
            if !marker.is_empty() {
                return Ok((marker.to_string(), QuoteStyle::None));
            }
        }
    }
    Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch)
}

/// 从脚本前缀中提取 cd 路径（可选）。
///
/// 支持: "cd path && " 形式，支持单引号、双引号或不带引号。
pub(crate) fn extract_leading_cd_path(script: &str) -> Option<String> {
    let trimmed = script.trim();
    if !trimmed.starts_with("cd ") {
        return None;
    }

    let after_cd = trimmed.strip_prefix("cd ")?.trim();
    let path_part = if after_cd.contains(" && ") {
        after_cd.split(" && ").next().unwrap_or(after_cd)
    } else if after_cd.ends_with("&&") {
        after_cd.trim_end_matches("&&").trim()
    } else {
        return None;
    };

    let path = path_part.trim();
    if path.is_empty() || path.contains(';') {
        return None;
    }

    let path = if (path.starts_with('\'') && path.ends_with('\''))
        || (path.starts_with('"') && path.ends_with('"'))
    {
        &path[1..path.len() - 1]
    } else {
        path
    };

    Some(path.to_string())
}
