use std::path::Path;

use crate::{MemoryCodeSymbolInvalidationPlan, MemoryCodeSymbolRecord};

/// 仓库结构信息来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoStructureSourceKind {
    /// 静态文件扫描。
    StaticScan,
    /// LSP workspace symbol 结果。
    Lsp,
    /// 本机 memory code symbol 索引。
    MemoryIndex,
}

/// 仓库模块类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoModuleKind {
    /// workspace 根模块。
    WorkspaceRoot,
    /// Rust crate。
    RustCrate,
    /// Node package。
    NodePackage,
    /// Python project。
    PythonProject,
    /// Go module。
    GoModule,
    /// 普通目录模块。
    Directory,
}

/// 仓库文件角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoFileRole {
    /// manifest 文件。
    Manifest,
    /// 入口文件。
    EntryPoint,
    /// 源码文件。
    Source,
    /// 配置文件。
    Config,
    /// 其它文件。
    Other,
}

/// 仓库符号类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepoSymbolKind {
    /// 函数或方法符号。
    Function,
    /// 类型符号。
    Type,
    /// 模块符号。
    Module,
    /// 入口点符号。
    EntryPoint,
    /// 未知符号。
    Unknown,
}

/// 仓库结构关系边类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoEdgeKind {
    /// 模块包含文件。
    Contains,
    /// 文件声明符号。
    Declares,
    /// 符号依赖另一符号。
    DependsOn,
    /// 文件是模块入口。
    EntrypointOf,
    /// 引用关系。
    References,
}

/// 仓库结构来源标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSourceTag {
    /// 来源类型。
    pub kind: RepoStructureSourceKind,
    /// 来源细节。
    pub detail: String,
}

/// 仓库 workspace 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoWorkspace {
    /// workspace 引用。
    pub workspace_ref: String,
    /// workspace 根目录。
    pub workspace_root: String,
    /// 模块引用列表。
    pub module_refs: Vec<String>,
    /// 主要语言。
    pub primary_languages: Vec<String>,
    /// 入口文件引用。
    pub entrypoint_refs: Vec<String>,
    /// 已扫描文件数。
    pub scanned_files: usize,
    /// 是否被扫描上限截断。
    pub truncated: bool,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 来源标签。
    pub sources: Vec<RepoSourceTag>,
}

/// 仓库模块投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoModule {
    /// 模块引用。
    pub module_ref: String,
    /// 展示名称。
    pub display_name: String,
    /// 模块类型。
    pub kind: RepoModuleKind,
    /// 模块路径引用。
    pub path_ref: String,
    /// manifest 路径引用。
    pub manifest_path_ref: Option<String>,
    /// 入口文件路径引用。
    pub entrypoint_path_refs: Vec<String>,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 来源标签。
    pub sources: Vec<RepoSourceTag>,
}

/// 仓库文件投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFile {
    /// 文件引用。
    pub file_ref: String,
    /// 路径引用。
    pub path_ref: String,
    /// 所属模块引用。
    pub module_ref: String,
    /// 语言。
    pub language: String,
    /// 文件角色。
    pub role: RepoFileRole,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 来源标签。
    pub sources: Vec<RepoSourceTag>,
}

/// 仓库符号投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSymbol {
    /// 符号引用。
    pub symbol_ref: String,
    /// 符号名。
    pub symbol_name: String,
    /// 符号类型。
    pub kind: RepoSymbolKind,
    /// 路径引用。
    pub path_ref: String,
    /// 所属模块引用。
    pub module_ref: Option<String>,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 是否仅为引用投影。
    pub ref_only: bool,
    /// 来源标签。
    pub sources: Vec<RepoSourceTag>,
}

/// 仓库结构关系边。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEdge {
    /// 起点引用。
    pub from_ref: String,
    /// 终点引用。
    pub to_ref: String,
    /// 边类型。
    pub kind: RepoEdgeKind,
    /// 来源。
    pub source: RepoStructureSourceKind,
}

/// 仓库结构快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIndexSnapshot {
    /// workspace 投影。
    pub workspace: RepoWorkspace,
    /// 模块列表。
    pub modules: Vec<RepoModule>,
    /// 文件列表。
    pub files: Vec<RepoFile>,
    /// 符号列表。
    pub symbols: Vec<RepoSymbol>,
    /// 关系边列表。
    pub edges: Vec<RepoEdge>,
    /// 刷新时间。
    pub refreshed_at_ms: u64,
    /// 是否是部分快照。
    pub partial: bool,
}

/// 仓库结构查询输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStructureQuery {
    /// workspace 根目录。
    pub workspace_root: String,
    /// 查询文本。
    pub query_text: String,
    /// 最大扫描文件数。
    pub max_files: usize,
    /// 最大符号数。
    pub symbol_limit: usize,
    /// memory code symbol 输入记录。
    pub memory_records: Vec<MemoryCodeSymbolRecord>,
}

/// 仓库结构刷新输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStructureRefreshInput {
    /// memory code symbol 记录。
    pub memory_records: Vec<MemoryCodeSymbolRecord>,
    /// 变更路径引用。
    pub changed_path_refs: Vec<String>,
    /// 删除路径引用。
    pub deleted_path_refs: Vec<String>,
    /// 当前时间。
    pub now_ms: u64,
}

/// 仓库结构刷新计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStructureRefreshPlan {
    /// 刷新模式。
    pub mode: String,
    /// 需要重新索引的路径引用。
    pub changed_path_refs: Vec<String>,
    /// 已删除路径引用。
    pub deleted_path_refs: Vec<String>,
    /// memory symbol 失效计划。
    pub invalidation: Option<MemoryCodeSymbolInvalidationPlan>,
    /// 是否存在部分 stale 数据。
    pub partial_stale: bool,
}

/// 仓库结构查询报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStructureQueryReport {
    /// 仓库结构快照。
    pub snapshot: RepoIndexSnapshot,
    /// 有界摘要。
    pub summary: String,
    /// 来源引用。
    pub source_refs: Vec<String>,
    /// 可选刷新计划。
    pub refresh_plan: Option<RepoStructureRefreshPlan>,
}

#[derive(Debug)]
pub(super) struct RepoWorkspaceScanState {
    pub(super) workspace: RepoWorkspace,
    pub(super) modules: Vec<RepoModule>,
    pub(super) files: Vec<RepoFile>,
    pub(super) refreshed_at_ms: u64,
    pub(super) partial: bool,
}

#[derive(Debug)]
pub(super) struct RepoScanContext<'a> {
    pub(super) root: &'a Path,
    pub(super) max_files: usize,
    pub(super) refreshed_at_ms: u64,
}

#[derive(Debug, Default)]
pub(super) struct RepoScanAccumulator {
    pub(super) file_count: usize,
    pub(super) truncated: bool,
    pub(super) modules: Vec<RepoModule>,
    pub(super) files: Vec<RepoFile>,
}
