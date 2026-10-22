//! OpenCode 对标的 LSP catalog 与本机可用性探测。

#[path = "lsp_support_data.rs"]
mod lsp_support_data;

use std::path::{Path, PathBuf};

use globset::Glob;

use self::lsp_support_data::{
    CORE_DIAGNOSTICS_SERVERS, LSP_SERVERS, OPEN_CODE_EXPOSED_CAPABILITIES,
};
use crate::{command_resolver::command_path, language_registry::LanguageRegistry};
use serde::Serialize;

/// 单个 LSP server 描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LspServerDescriptor {
    /// 与 OpenCode 对齐的 server id。
    pub id: &'static str,
    /// PATH 中优先探测的二进制名称。
    pub binary: &'static str,
    /// 主要覆盖的文件扩展名。
    pub extensions: &'static [&'static str],
    /// 与 OpenCode 对齐的根目录探测线索。
    pub root_markers: &'static [&'static str],
    /// 启动 server 的典型命令。
    pub launch_hint: &'static str,
    /// 是否优先依赖本机已安装工具。
    pub requires_local_runtime: bool,
    /// OpenCode 是否包含下载/安装兜底逻辑。
    pub opencode_can_bootstrap: bool,
}

/// 单个 LSP server 的本机可用性。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerAvailability {
    /// server id。
    pub id: String,
    /// PATH 探测使用的二进制名。
    pub binary: String,
    /// 是否已在本机 PATH 中找到。
    pub available: bool,
    /// 命中的真实路径。
    pub resolved_path: Option<String>,
    /// 主要覆盖的文件扩展名。
    pub extensions: Vec<String>,
}

/// LSP catalog 汇总。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspCatalogSummary {
    /// OpenCode catalog 中的 server 总数。
    pub total_servers: usize,
    /// 所有对齐的 server id。
    pub server_ids: Vec<String>,
    /// P1-3 第一阶段要求的核心 diagnostics server。
    pub core_diagnostics_servers: Vec<String>,
    /// 当前机器 PATH 中可直接探测到的 server。
    pub available_servers: Vec<LspServerAvailability>,
    /// OpenCode LSP 对外暴露的能力集合。
    pub exposed_capabilities: Vec<String>,
}

/// 某个工作区内按文件解析出的 LSP 命中结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceMatch {
    /// 目标文件路径。
    pub file: String,
    /// 目标扩展名。
    pub extension: String,
    /// 实际命中的 server。
    pub matched_servers: Vec<String>,
    /// 因缺少根标记而被排除的 server。
    pub filtered_by_root_markers: Vec<String>,
}

/// LSP catalog/runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspCatalogRuntime;

impl LspCatalogRuntime {
    /// 返回 OpenCode 对标的完整 LSP catalog。
    #[must_use]
    pub const fn catalog() -> &'static [LspServerDescriptor] {
        &LSP_SERVERS
    }

    /// 返回 P1-3 第一阶段优先保证 diagnostics 的核心 server。
    #[must_use]
    pub fn core_diagnostics_servers() -> Vec<String> {
        CORE_DIAGNOSTICS_SERVERS
            .iter()
            .map(|server| (*server).to_owned())
            .collect()
    }

    /// 返回与 OpenCode 对齐的对外能力清单（纯静态 catalog 数据）。
    #[must_use]
    pub fn exposed_capabilities() -> Vec<String> {
        OPEN_CODE_EXPOSED_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_owned())
            .collect()
    }

    /// 基于本机 PATH 探测当前可直接调用的 LSP server。
    ///
    /// 探测全程不启动外部进程，保证 `doctor`、`lsp servers` 等命令的响应预算。
    #[must_use]
    pub fn detect_available_servers() -> Vec<LspServerAvailability> {
        Self::catalog()
            .iter()
            .map(|descriptor| {
                let resolved = command_path(descriptor.binary);
                LspServerAvailability {
                    id: descriptor.id.to_owned(),
                    binary: descriptor.binary.to_owned(),
                    available: resolved.is_some(),
                    resolved_path: resolved.map(|path| path.display().to_string()),
                    extensions: descriptor
                        .extensions
                        .iter()
                        .map(|extension| (*extension).to_owned())
                        .collect(),
                }
            })
            .collect()
    }

    /// 生成可序列化的 LSP catalog 摘要。
    #[must_use]
    pub fn summary() -> LspCatalogSummary {
        let available_servers = Self::detect_available_servers()
            .into_iter()
            .filter(|server| server.available)
            .collect::<Vec<_>>();
        LspCatalogSummary {
            total_servers: Self::catalog().len(),
            server_ids: Self::catalog()
                .iter()
                .map(|server| server.id.to_owned())
                .collect(),
            core_diagnostics_servers: Self::core_diagnostics_servers(),
            available_servers,
            exposed_capabilities: OPEN_CODE_EXPOSED_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_owned())
                .collect(),
        }
    }

    /// 判断给定文件是否在 OpenCode catalog 中有对应 server。
    #[must_use]
    pub fn supports_path(path: &Path) -> bool {
        !Self::servers_for_path(path).is_empty()
    }

    /// 按共享语言注册表返回匹配的 server 规格。
    #[must_use]
    pub(crate) fn servers_for_path(path: &Path) -> Vec<LspServerDescriptor> {
        let Some(language) = LanguageRegistry::resolve(path) else {
            return Vec::new();
        };
        Self::catalog()
            .iter()
            .copied()
            .filter(|server| {
                server
                    .extensions
                    .iter()
                    .any(|key| key.eq_ignore_ascii_case(language.lookup_key()))
            })
            .collect()
    }

    /// 按共享语言注册表与 root marker 规则解析某个文件在当前工作区可命中的 LSP。
    #[must_use]
    pub fn resolve_workspace_match(
        workspace_root: &Path,
        file_path: &Path,
    ) -> Option<LspWorkspaceMatch> {
        let language = LanguageRegistry::resolve(file_path)?;
        let candidates = Self::servers_for_path(file_path);
        if candidates.is_empty() {
            return None;
        }
        let deno_root = candidates
            .iter()
            .find(|server| server.id == "deno")
            .and_then(|server| {
                nearest_project_root(workspace_root, file_path, server.root_markers)
            });
        let mut matched_servers = Vec::new();
        let mut filtered_by_root_markers = Vec::new();
        for server in candidates {
            let root = nearest_project_root(workspace_root, file_path, server.root_markers);
            let excluded_by_deno = server.id == "typescript" && deno_root.is_some();
            if root.is_some() && !excluded_by_deno {
                matched_servers.push(server.id.to_owned());
            } else {
                filtered_by_root_markers.push(server.id.to_owned());
            }
        }
        Some(LspWorkspaceMatch {
            file: file_path.display().to_string(),
            extension: language.lookup_key().to_owned(),
            matched_servers,
            filtered_by_root_markers,
        })
    }
}

pub(crate) fn nearest_project_root(
    workspace_root: &Path,
    file_path: &Path,
    markers: &[&str],
) -> Option<PathBuf> {
    let file_parent = file_path.parent()?;
    if markers.is_empty() {
        let workspace_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let file_parent = file_parent
            .canonicalize()
            .unwrap_or_else(|_| file_parent.to_path_buf());
        return Some(if file_parent.starts_with(&workspace_root) {
            workspace_root
        } else {
            file_parent
        });
    }

    let mut current = Some(file_parent);
    while let Some(directory) = current {
        if markers
            .iter()
            .any(|marker| root_marker_exists(directory, marker))
        {
            return Some(
                directory
                    .canonicalize()
                    .unwrap_or_else(|_| directory.to_path_buf()),
            );
        }
        current = directory.parent();
    }
    None
}

fn root_marker_exists(directory: &Path, marker: &str) -> bool {
    if !marker.contains(['*', '?', '[', ']']) {
        return directory.join(marker).exists();
    }
    let Ok(glob) = Glob::new(marker) else {
        return false;
    };
    let matcher = glob.compile_matcher();
    directory.read_dir().is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .any(|entry| matcher.is_match(entry.file_name()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_catalog_paths_have_concrete_language_ids() {
        for server in LspCatalogRuntime::catalog() {
            for key in server.extensions {
                let path = key.strip_prefix('.').map_or_else(
                    || (*key).to_owned(),
                    |extension| format!("fixture.{extension}"),
                );
                let language = LanguageRegistry::resolve(Path::new(&path))
                    .expect("catalog 路径必须能被共享语言注册表解析");
                assert_ne!(
                    language.lsp_language_id(),
                    "plaintext",
                    "{path} 已注册到 {}，不能以 plaintext 打开",
                    server.id
                );
            }
        }
    }

    #[test]
    fn lsp_catalog_resolves_workspace_match_by_extension_and_root_markers() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-workspace-match");
        let src = workspace.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        let file = src.join("main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let result = LspCatalogRuntime::resolve_workspace_match(&workspace, &file)
            .expect("rust 文件应命中 LSP");

        assert_eq!(result.extension, ".rs");
        assert!(result.matched_servers.contains(&"rust-analyzer".to_owned()));
    }

    #[test]
    fn nearest_project_root_prefers_closest_nested_project() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-nearest-root");
        let nested = workspace.join("packages").join("web");
        let src = nested.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(workspace.join("package.json"), "{}").unwrap();
        std::fs::write(nested.join("package.json"), "{}").unwrap();
        let file = src.join("main.ts");
        std::fs::write(&file, "export const value = 1;\n").unwrap();

        let root = nearest_project_root(&workspace, &file, &["package.json"]).unwrap();

        assert_eq!(root, nested.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn nearest_project_root_supports_glob_markers() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-glob-root");
        let src = workspace.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(workspace.join("demo.sln"), "").unwrap();
        let file = src.join("Program.cs");
        std::fs::write(&file, "class Program {}\n").unwrap();

        let root = nearest_project_root(&workspace, &file, &["*.sln"]).unwrap();

        assert_eq!(root, workspace.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn lsp_catalog_excludes_typescript_server_inside_deno_project() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-deno-root");
        let src = workspace.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(workspace.join("package.json"), "{}").unwrap();
        std::fs::write(workspace.join("deno.json"), "{}").unwrap();
        let file = src.join("main.ts");
        std::fs::write(&file, "export const value = 1;\n").unwrap();

        let result = LspCatalogRuntime::resolve_workspace_match(&workspace, &file).unwrap();

        assert!(result.matched_servers.contains(&"deno".to_owned()));
        assert!(!result.matched_servers.contains(&"typescript".to_owned()));
        assert!(
            result
                .filtered_by_root_markers
                .contains(&"typescript".to_owned())
        );
        let _ = std::fs::remove_dir_all(workspace);
    }
}
