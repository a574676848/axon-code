//! Git Apply - 对标 codex git-utils/apply.rs。
//!
//! 统一差异应用工具，支持 preflight/dry-run 模式。
//!
//! 核心能力：
//! - `ApplyGitRequest` - 应用请求
//! - `ApplyGitResult` - 应用结果
//! - `apply_git_patch()` - 应用补丁（含 preflight 支持）
//! - `parse_git_apply_output()` - 解析 git apply 输出
//! - `extract_paths_from_patch()` - 从补丁提取路径
//! - `stage_paths()` - 暂存路径

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use axon_process::non_interactive_command;

#[path = "git_apply_parse.rs"]
mod git_apply_parse;

pub use git_apply_parse::{extract_paths_from_patch, parse_git_apply_output};

/// 应用请求参数。
#[derive(Debug, Clone)]
pub struct ApplyGitRequest {
    /// 工作目录
    pub cwd: PathBuf,
    /// 统一差异文本
    pub diff: String,
    /// 是否回退
    pub revert: bool,
    /// 是否预检（dry-run）
    pub preflight: bool,
}

impl ApplyGitRequest {
    /// 创建新请求。
    pub fn new(cwd: impl Into<PathBuf>, diff: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            diff: diff.into(),
            revert: false,
            preflight: false,
        }
    }

    /// 设置回退模式。
    pub fn with_revert(mut self, revert: bool) -> Self {
        self.revert = revert;
        self
    }

    /// 设置预检模式（dry-run）。
    pub fn with_preflight(mut self, preflight: bool) -> Self {
        self.preflight = preflight;
        self
    }
}

/// 应用结果。
#[derive(Debug, Clone)]
pub struct ApplyGitResult {
    /// 退出码
    pub exit_code: i32,
    /// 成功应用的路径
    pub applied_paths: Vec<String>,
    /// 跳过的路径
    pub skipped_paths: Vec<String>,
    /// 冲突的路径
    pub conflicted_paths: Vec<String>,
    /// 标准输出
    pub stdout: String,
    /// 标准错误
    pub stderr: String,
    /// 用于日志的命令字符串
    pub cmd_for_log: String,
}

impl ApplyGitResult {
    /// 是否完全成功。
    pub fn is_success(&self) -> bool {
        self.exit_code == 0 && self.conflicted_paths.is_empty()
    }

    /// 是否有部分成功。
    pub fn has_partial_success(&self) -> bool {
        !self.applied_paths.is_empty()
    }

    /// 是否有冲突。
    pub fn has_conflicts(&self) -> bool {
        !self.conflicted_paths.is_empty()
    }

    /// 总变更路径数。
    pub fn total_changed_paths(&self) -> usize {
        self.applied_paths.len() + self.skipped_paths.len() + self.conflicted_paths.len()
    }
}

/// 应用统一差异到目标仓库，调用系统 `git apply`。
///
/// 当 `preflight` 为 `true` 时，行为类似 `git apply --check`，不修改工作树。
pub fn apply_git_patch(req: &ApplyGitRequest) -> io::Result<ApplyGitResult> {
    let git_root = resolve_git_root(&req.cwd)?;

    // 写入临时补丁文件
    let (tmpdir, patch_path) = write_temp_patch(&req.diff)?;
    let _guard = tmpdir;

    // 回退模式需先暂存工作区路径
    if req.revert && !req.preflight {
        stage_paths(&git_root, &req.diff)?;
    }

    // 构建 git 参数
    let mut args: Vec<String> = vec!["apply".into(), "--3way".into()];
    if req.revert {
        args.push("-R".into());
    }

    // 可选：通过环境变量传递额外 git 配置
    let mut cfg_parts: Vec<String> = Vec::new();
    if let Ok(cfg) = std::env::var("AXON_APPLY_GIT_CFG") {
        for pair in cfg.split(',') {
            let p = pair.trim();
            if p.is_empty() || !p.contains('=') {
                continue;
            }
            cfg_parts.push("-c".into());
            cfg_parts.push(p.to_string());
        }
    }

    args.push(patch_path.to_string_lossy().to_string());

    // 预检模式：dry-run，不修改工作树
    if req.preflight {
        let mut check_args = vec!["apply".to_string(), "--check".to_string()];
        if req.revert {
            check_args.push("-R".to_string());
        }
        check_args.push(patch_path.to_string_lossy().to_string());
        let rendered = render_command_for_log(&git_root, &cfg_parts, &check_args);
        let (c_code, c_out, c_err) = run_git(&git_root, &cfg_parts, &check_args)?;
        let (mut applied_paths, mut skipped_paths, mut conflicted_paths) =
            parse_git_apply_output(&c_out, &c_err);
        applied_paths.sort();
        applied_paths.dedup();
        skipped_paths.sort();
        skipped_paths.dedup();
        conflicted_paths.sort();
        conflicted_paths.dedup();
        return Ok(ApplyGitResult {
            exit_code: c_code,
            applied_paths,
            skipped_paths,
            conflicted_paths,
            stdout: c_out,
            stderr: c_err,
            cmd_for_log: rendered,
        });
    }

    let cmd_for_log = render_command_for_log(&git_root, &cfg_parts, &args);
    let (code, stdout, stderr) = run_git(&git_root, &cfg_parts, &args)?;

    let (mut applied_paths, mut skipped_paths, mut conflicted_paths) =
        parse_git_apply_output(&stdout, &stderr);
    applied_paths.sort();
    applied_paths.dedup();
    skipped_paths.sort();
    skipped_paths.dedup();
    conflicted_paths.sort();
    conflicted_paths.dedup();

    Ok(ApplyGitResult {
        exit_code: code,
        applied_paths,
        skipped_paths,
        conflicted_paths,
        stdout,
        stderr,
        cmd_for_log,
    })
}

/// 仅对磁盘上实际存在的文件进行暂存。
pub fn stage_paths(git_root: &Path, diff: &str) -> io::Result<()> {
    let paths = extract_paths_from_patch(diff);
    let mut existing: Vec<String> = Vec::new();
    for p in paths {
        let joined = git_root.join(&p);
        if std::fs::symlink_metadata(&joined).is_ok() {
            existing.push(p);
        }
    }
    if existing.is_empty() {
        return Ok(());
    }
    let mut cmd = non_interactive_command("git");
    cmd.arg("add");
    cmd.arg("--");
    for p in &existing {
        cmd.arg(OsStr::new(p));
    }
    let out = cmd.current_dir(git_root).output()?;
    let _code = out.status.code().unwrap_or(-1);
    // 暂存失败不硬性失败，尽力而为
    Ok(())
}

// ============ 内部辅助函数 ============

fn resolve_git_root(cwd: &Path) -> io::Result<PathBuf> {
    let out = non_interactive_command("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(cwd)
        .output()?;
    let code = out.status.code().unwrap_or(-1);
    if code != 0 {
        return Err(io::Error::other(format!(
            "not a git repository (exit {}): {}",
            code,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}

fn write_temp_patch(diff: &str) -> io::Result<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("patch.diff");
    std::fs::write(&path, diff)?;
    Ok((dir, path))
}

fn run_git(cwd: &Path, git_cfg: &[String], args: &[String]) -> io::Result<(i32, String, String)> {
    let mut cmd = non_interactive_command("git");
    for p in git_cfg {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }
    let out = cmd.current_dir(cwd).output()?;
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Ok((code, stdout, stderr))
}

fn quote_shell(s: &str) -> String {
    let simple = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.:/@%+".contains(c));
    if simple {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn render_command_for_log(cwd: &Path, git_cfg: &[String], args: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push("git".to_string());
    for a in git_cfg {
        parts.push(quote_shell(a));
    }
    for a in args {
        parts.push(quote_shell(a));
    }
    format!(
        "(cd {} && {})",
        quote_shell(&cwd.display().to_string()),
        parts.join(" ")
    )
}

#[cfg(test)]
#[path = "git_apply_tests.rs"]
mod tests;