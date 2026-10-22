//! apply_patch 调用层与预验证引擎。
//!
//! 对齐 Codex `apply-patch` 的 `MaybeApplyPatchVerified` 验证模式，在正式应用前
//! 校验补丁与文件系统的一致性，并检测隐式调用（直接传递补丁文本）。
//!
//! 核心能力：
//! - `MaybeApplyPatchVerified` 枚举：结构化验证结果
//! - 隐式调用检测：防止用户绕过 tool 接口直接传递 patch body
//! - Shell 脚本解析支持：Unix heredoc 与 PowerShell here-string 形式

use axon_patch_model::{PatchHunk, PatchParseError, parse_patch};

#[path = "apply_patch_shell.rs"]
mod apply_patch_shell;

use apply_patch_shell::{extract_apply_patch_from_shell, parse_shell_script};

/// apply_patch 解析结果（初始阶段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaybeApplyPatch {
    /// 有效的 apply_patch 调用，包含解析后的参数。
    Body(ApplyPatchArgs),
    /// Shell 脚本解析失败。
    ShellParseError(ExtractHeredocError),
    /// 补丁文本解析失败。
    PatchParseError(PatchParseError),
    /// 不是一个 apply_patch 调用。
    NotApplyPatch,
}

/// Shell patch 提取错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractHeredocError {
    /// 命令未以 apply_patch 开头。
    CommandDidNotStartWithApplyPatch,
    /// 补丁文本解析失败。
    FailedToParsePatchIntoAst,
}

/// apply_patch 参数（解析后的中间表示）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPatchArgs {
    /// 原始补丁文本。
    pub patch: String,
    /// 解析后的 hunk 列表。
    pub hunks: Vec<PatchHunk>,
    /// 可选的工作目录（相对路径）。
    pub workdir: Option<String>,
}

/// apply_patch 命令名称列表。
const APPLY_PATCH_COMMANDS: &[&str] = &["apply_patch", "applypatch"];

/// 解析命令行参数，判断是否为 apply_patch 调用。
///
/// 支持两种形式：
/// 1. 直接调用：`apply_patch "*** Begin Patch\n..."`
/// 2. Shell 脚本：Unix `bash -lc "apply_patch <<'EOF'\n..."` 或 PowerShell `@'...'@ | apply_patch`
pub fn maybe_parse_apply_patch(argv: &[String]) -> MaybeApplyPatch {
    if argv.len() < 2 {
        return MaybeApplyPatch::NotApplyPatch;
    }

    // 直接调用：apply_patch <patch> 或 applypatch <patch>
    let cmd = &argv[0];
    if APPLY_PATCH_COMMANDS.contains(&cmd.as_str()) {
        let body = &argv[1];
        return match parse_patch(body) {
            Ok(hunks) => MaybeApplyPatch::Body(ApplyPatchArgs {
                patch: body.clone(),
                hunks,
                workdir: None,
            }),
            Err(e) => MaybeApplyPatch::PatchParseError(e),
        };
    }

    // Shell 脚本形式
    match parse_shell_script(argv) {
        Some((shell, script)) => match extract_apply_patch_from_shell(shell, script) {
            Ok((body, workdir)) => match parse_patch(&body) {
                Ok(hunks) => MaybeApplyPatch::Body(ApplyPatchArgs {
                    patch: body,
                    hunks,
                    workdir,
                }),
                Err(e) => MaybeApplyPatch::PatchParseError(e),
            },
            Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch) => {
                MaybeApplyPatch::NotApplyPatch
            }
            Err(e) => MaybeApplyPatch::ShellParseError(e),
        },
        None => MaybeApplyPatch::NotApplyPatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply_patch_invocation::apply_patch_shell::{
        extract_apply_patch_from_bash, extract_apply_patch_from_powershell, extract_leading_cd_path,
    };

    #[test]
    fn test_direct_apply_patch_invocation() {
        let patch = r"*** Begin Patch
*** Add File: foo
+hello
*** End Patch";
        let args = vec!["apply_patch".to_string(), patch.to_string()];

        match maybe_parse_apply_patch(&args) {
            MaybeApplyPatch::Body(ApplyPatchArgs { hunks, .. }) => {
                assert_eq!(hunks.len(), 1);
            }
            _ => panic!("expected Body"),
        }
    }

    #[test]
    fn test_not_apply_patch() {
        let args = vec!["echo".to_string(), "hello".to_string()];

        match maybe_parse_apply_patch(&args) {
            MaybeApplyPatch::NotApplyPatch => {}
            _ => panic!("expected NotApplyPatch"),
        }
    }

    #[test]
    fn test_shell_heredoc_extraction() {
        let script = r"apply_patch <<'PATCH'
*** Begin Patch
*** Add File: bar
+baz
*** End Patch
PATCH";

        match extract_apply_patch_from_bash(script) {
            Ok((body, workdir)) => {
                assert!(body.contains("*** Add File: bar"));
                assert!(workdir.is_none());
            }
            Err(e) => panic!("expected Ok, got {:?}", e),
        }
    }

    #[test]
    fn test_powershell_here_string_extraction_supports_lf_and_crlf() {
        for script in [
            r"@'
*** Begin Patch
*** Add File: bar
+baz
*** End Patch
'@ | apply_patch",
            "@'\r\n*** Begin Patch\r\n*** Add File: bar\r\n+baz\r\n*** End Patch\r\n'@ | apply_patch",
        ] {
            match extract_apply_patch_from_powershell(script) {
                Ok((body, workdir)) => {
                    assert!(body.contains("*** Add File: bar"));
                    assert!(workdir.is_none());
                }
                Err(e) => panic!("expected Ok, got {:?}", e),
            }
        }
    }

    #[test]
    fn test_shell_heredoc_with_cd() {
        let script = r"cd /workspace && apply_patch <<'PATCH'
*** Begin Patch
*** Add File: bar
+baz
*** End Patch
PATCH";

        match extract_apply_patch_from_bash(script) {
            Ok((body, workdir)) => {
                assert!(body.contains("*** Add File: bar"));
                assert_eq!(workdir, Some("/workspace".to_string()));
            }
            Err(e) => panic!("expected Ok, got {:?}", e),
        }
    }

    #[test]
    fn test_extract_leading_cd_path() {
        assert_eq!(
            extract_leading_cd_path("cd /foo && "),
            Some("/foo".to_string())
        );
        assert_eq!(
            extract_leading_cd_path("cd 'foo bar' && "),
            Some("foo bar".to_string())
        );
        assert_eq!(extract_leading_cd_path("echo hello"), None);
        assert_eq!(extract_leading_cd_path("cd foo; "), None);
    }
}
