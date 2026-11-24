//! LSP 启动命令计划。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::lsp_document::file_path_from_uri;
use crate::lsp_support::nearest_project_root;
use crate::{
    LspCatalogRuntime, LspWorkspaceCoverage, command_resolver::command_path,
    runtime_limits::LSP_EXECUTABLE_CACHE_TTL_SECS,
};
use axon_domain::AxonResult;
use axon_process::configure_non_interactive_process;
use serde::Serialize;

fn executable_cache() -> &'static Mutex<HashMap<String, ExecutableCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ExecutableCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone)]
struct ResolvedLspCommand {
    program: String,
    prefix_args: Vec<String>,
}

struct ExecutableCacheEntry {
    resolved: Option<ResolvedLspCommand>,
    cached_at: Instant,
}

/// 单个 LSP server 的本机启动计划。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspLaunchPlan {
    /// server id。
    pub server_id: String,
    /// 启动程序。
    pub program: String,
    /// 启动参数。
    pub args: Vec<String>,
    /// 工作目录。
    pub cwd: String,
}

impl LspCatalogRuntime {
    /// 基于工作区覆盖结果，生成当前仓库可尝试启动的 LSP 计划。
    #[must_use]
    pub fn resolve_workspace_launch_plans(workspace_root: &Path) -> Vec<LspLaunchPlan> {
        let coverage = Self::resolve_workspace_coverage(workspace_root);
        launch_plans_from_coverage(workspace_root, &coverage)
    }

    /// 只返回与目标本机文档语言及 root marker 匹配的启动计划。
    pub fn resolve_document_launch_plans(
        workspace_root: &Path,
        text_document_uri: &str,
    ) -> AxonResult<Vec<LspLaunchPlan>> {
        let document_path = file_path_from_uri(text_document_uri)?;
        let candidates = Self::servers_for_path(&document_path);
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let deno_root = candidates
            .iter()
            .find(|server| server.id == "deno")
            .and_then(|server| {
                nearest_project_root(workspace_root, &document_path, server.root_markers)
            });

        Ok(candidates
            .into_iter()
            .filter(|server| !(server.id == "typescript" && deno_root.is_some()))
            .filter_map(|server| {
                let root = document_launch_workspace_root(
                    workspace_root,
                    &document_path,
                    server.root_markers,
                )?;
                launch_plan_for_server(server.id, &root)
            })
            .collect())
    }
}

fn document_launch_workspace_root(
    workspace_root: &Path,
    document_path: &Path,
    root_markers: &[&str],
) -> Option<PathBuf> {
    nearest_project_root(workspace_root, document_path, root_markers)?;
    Some(
        workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf()),
    )
}

fn launch_plans_from_coverage(
    workspace_root: &Path,
    coverage: &LspWorkspaceCoverage,
) -> Vec<LspLaunchPlan> {
    let mut plans = Vec::new();
    for server_id in &coverage.matched_servers {
        if let Some(plan) = launch_plan_for_server(server_id, workspace_root) {
            plans.push(plan);
        }
    }
    plans
}

fn launch_plan_for_server(server_id: &str, workspace_root: &Path) -> Option<LspLaunchPlan> {
    let cwd = workspace_root.display().to_string();
    let (program, args) = match server_id {
        "deno" => ("deno", vec!["lsp"]),
        "typescript" => ("typescript-language-server", vec!["--stdio"]),
        "vue" => ("vue-language-server", vec!["--stdio"]),
        "oxlint" => ("oxc_language_server", Vec::new()),
        "biome" => ("biome", vec!["lsp-proxy", "--stdio"]),
        "gopls" => ("gopls", Vec::new()),
        "ruby-lsp" => ("rubocop", vec!["--lsp"]),
        "ty" => ("ty", vec!["server"]),
        "pyright" => ("pyright-langserver", vec!["--stdio"]),
        "elixir-ls" => ("elixir-ls", Vec::new()),
        "zls" => ("zls", Vec::new()),
        "csharp" => ("csharp-ls", Vec::new()),
        "razor" => ("rzls", Vec::new()),
        "fsharp" => ("fsautocomplete", vec!["--adaptive-lsp-server-enabled"]),
        "sourcekit" => ("sourcekit-lsp", Vec::new()),
        "rust-analyzer" => ("rust-analyzer", Vec::new()),
        "clangd" => ("clangd", vec!["--background-index"]),
        "svelte" => ("svelte-language-server", vec!["--stdio"]),
        "astro" => ("astro-ls", vec!["--stdio"]),
        "jdtls" => ("jdtls", Vec::new()),
        "kotlin" => ("kotlin-lsp", vec!["--stdio"]),
        "yaml-ls" => ("yaml-language-server", vec!["--stdio"]),
        "lua-ls" => ("lua-language-server", Vec::new()),
        "php intelephense" => ("intelephense", vec!["--stdio"]),
        "prisma" => ("prisma", vec!["language-server"]),
        "dart" => ("dart", vec!["language-server", "--lsp"]),
        "ocaml-lsp" => ("ocamllsp", Vec::new()),
        "bash" => ("bash-language-server", vec!["start"]),
        "terraform" => ("terraform-ls", vec!["serve"]),
        "texlab" => ("texlab", Vec::new()),
        "dockerfile" => ("docker-langserver", vec!["--stdio"]),
        "gleam" => ("gleam", vec!["lsp"]),
        "clojure-lsp" => ("clojure-lsp", vec!["listen"]),
        "nixd" => ("nixd", Vec::new()),
        "tinymist" => ("tinymist", Vec::new()),
        "haskell-language-server" => ("haskell-language-server-wrapper", vec!["--lsp"]),
        "julials" => (
            "julia",
            vec![
                "--startup-file=no",
                "--history-file=no",
                "-e",
                "using LanguageServer; runserver()",
            ],
        ),
        _ => return None,
    };
    let resolved = resolve_lsp_command(program)?;
    let mut resolved_args = resolved.prefix_args;
    resolved_args.extend(args.into_iter().map(ToOwned::to_owned));
    Some(LspLaunchPlan {
        server_id: server_id.to_owned(),
        program: resolved.program,
        args: resolved_args,
        cwd,
    })
}

fn resolve_lsp_command(program: &str) -> Option<ResolvedLspCommand> {
    let now = Instant::now();
    let ttl = Duration::from_secs(LSP_EXECUTABLE_CACHE_TTL_SECS);
    if let Some(cached) = executable_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(program)
        .filter(|entry| now.duration_since(entry.cached_at) <= ttl)
    {
        return cached.resolved.clone();
    }
    let resolved = resolve_lsp_command_uncached(program);
    executable_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            program.to_owned(),
            ExecutableCacheEntry {
                resolved: resolved.clone(),
                cached_at: now,
            },
        );
    resolved
}

fn resolve_lsp_command_uncached(program: &str) -> Option<ResolvedLspCommand> {
    let resolved_path = preferred_windows_wrapper(command_path(program)?);
    if program == "rust-analyzer" && !rust_analyzer_is_usable(&resolved_path) {
        return None;
    }
    let extension = resolved_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "cmd" | "bat" => Some(windows_command_wrapper(&resolved_path)),
        "ps1" => Some(powershell_command_wrapper(&resolved_path)),
        _ => Some(ResolvedLspCommand {
            program: resolved_path.display().to_string(),
            prefix_args: Vec::new(),
        }),
    }
}

fn preferred_windows_wrapper(path: PathBuf) -> PathBuf {
    if !cfg!(windows) || path.extension().is_some() {
        return path;
    }
    for extension in ["exe", "cmd", "bat"] {
        let candidate = path.with_extension(extension);
        if candidate.is_file() {
            return candidate;
        }
    }
    path
}

fn rust_analyzer_is_usable(path: &Path) -> bool {
    let mut command = Command::new(path);
    configure_non_interactive_process(&mut command);
    command
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn windows_command_wrapper(path: &Path) -> ResolvedLspCommand {
    ResolvedLspCommand {
        program: std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned()),
        prefix_args: vec![
            "/d".to_owned(),
            "/s".to_owned(),
            "/c".to_owned(),
            path.display().to_string(),
        ],
    }
}

fn powershell_command_wrapper(path: &Path) -> ResolvedLspCommand {
    ResolvedLspCommand {
        program: "pwsh".to_owned(),
        prefix_args: vec![
            "-NoLogo".to_owned(),
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-File".to_owned(),
            path.display().to_string(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_launch_plan_respects_executable_health() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-launch-plan");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        std::fs::write(workspace.join("lib.rs"), "pub fn demo() {}\n").unwrap();

        LspCatalogRuntime::invalidate_workspace_coverage(&workspace);
        let coverage = LspCatalogRuntime::resolve_workspace_coverage(&workspace);
        let plans = LspCatalogRuntime::resolve_workspace_launch_plans(&workspace);
        let expected = command_path("rust-analyzer")
            .map(preferred_windows_wrapper)
            .is_some_and(|path| rust_analyzer_is_usable(&path));

        assert!(
            coverage
                .matched_servers
                .contains(&"rust-analyzer".to_owned())
        );
        assert_eq!(
            plans.iter().any(|plan| plan.server_id == "rust-analyzer"),
            expected
        );
    }

    #[test]
    fn document_launch_root_reuses_workspace_across_nested_projects() {
        let workspace = axon_test_support::test_temp_root()
            .join(format!("axon-lsp-shared-workspace-{}", std::process::id()));
        let nested = workspace.join("packages").join("web");
        let source = nested.join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(workspace.join("package.json"), "{}").unwrap();
        std::fs::write(nested.join("package.json"), "{}").unwrap();
        let document = source.join("main.ts");
        std::fs::write(&document, "export const value = 1;\n").unwrap();

        let root =
            document_launch_workspace_root(&workspace, &document, &["package.json"]).unwrap();

        assert_eq!(root, workspace.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(windows)]
    #[test]
    fn windows_wrapper_prefers_cmd_over_extensionless_npm_shim() {
        let workspace = axon_test_support::test_temp_root()
            .join(format!("axon-lsp-wrapper-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let extensionless = workspace.join("typescript-language-server");
        let command = workspace.join("typescript-language-server.cmd");
        std::fs::write(&extensionless, "#!/bin/sh\n").unwrap();
        std::fs::write(&command, "@echo off\n").unwrap();

        assert_eq!(preferred_windows_wrapper(extensionless), command);
        let _ = std::fs::remove_dir_all(workspace);
    }
}
