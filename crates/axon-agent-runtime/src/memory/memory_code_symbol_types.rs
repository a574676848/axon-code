//! Code symbol memory contract types.

use std::path::PathBuf;

/// 摘要 hash 最大字符数。
pub const MEMORY_CODE_SYMBOL_HASH_MAX_CHARS: usize = 64;
/// 单个路径本机源码读取最大字节数。
pub const MEMORY_CODE_SYMBOL_INDEXER_MAX_SOURCE_BYTES: usize = 512 * 1024;

/// 代码符号类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryCodeSymbolKind {
    /// 函数。
    Function,
    /// 类型、结构体、类或枚举。
    Type,
    /// 模块。
    Module,
    /// API route 或 command。
    EntryPoint,
}

/// 代码符号记忆记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolRecord {
    /// 稳定符号引用。
    pub symbol_ref: String,
    /// 符号名。
    pub symbol_name: String,
    /// 符号类型。
    pub kind: MemoryCodeSymbolKind,
    /// 文件路径引用。
    pub path_ref: String,
    /// source 摘要 hash，不保存代码正文。
    pub source_summary_hash: String,
    /// 依赖符号引用。
    pub dependency_refs: Vec<String>,
    /// 定义/调用/测试覆盖引用。
    pub reference_refs: Vec<String>,
    /// 更新时间。
    pub updated_at_ms: u64,
}

/// 代码符号召回查询。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolQuery {
    /// 查询文本。
    pub query_text: String,
    /// 返回上限。
    pub limit: usize,
}

/// 代码符号召回命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolHit {
    /// 稳定符号引用。
    pub symbol_ref: String,
    /// 符号名。
    pub symbol_name: String,
    /// 符号类型。
    pub kind: MemoryCodeSymbolKind,
    /// 文件路径引用。
    pub path_ref: String,
    /// source 摘要 hash。
    pub source_summary_hash: String,
    /// 命中的关系 ref。
    pub reference_refs: Vec<String>,
    /// 命中分数，单位 basis point。
    pub score_basis_points: u16,
    /// 是否为 ref-only 投影。
    pub ref_only: bool,
}

/// 代码符号增量失效输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolInvalidationInput {
    /// 发生内容变化的路径引用。
    pub changed_path_refs: Vec<String>,
    /// 已删除的路径引用。
    pub deleted_path_refs: Vec<String>,
    /// 计划生成时间。
    pub now_ms: u64,
}

/// 代码符号增量失效计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolInvalidationPlan {
    /// 扫描记录数。
    pub scanned_records: usize,
    /// 需要重建索引的路径引用。
    pub reindex_path_refs: Vec<String>,
    /// 需要移除的路径引用。
    pub deleted_path_refs: Vec<String>,
    /// 直接受路径变更影响的符号引用。
    pub direct_stale_symbol_refs: Vec<String>,
    /// 因依赖已失效符号而受影响的符号引用。
    pub dependent_stale_symbol_refs: Vec<String>,
    /// 合并去重后的全部失效符号引用。
    pub stale_symbol_refs: Vec<String>,
    /// 是否保持 ref-only。
    pub ref_only: bool,
    /// 是否允许远控读取源码正文。
    pub remote_source_read_allowed: bool,
    /// 计划生成时间。
    pub planned_at_ms: u64,
}

/// 代码符号增量索引执行输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolIncrementalIndexInput {
    /// 当前索引快照。
    pub existing_records: Vec<MemoryCodeSymbolRecord>,
    /// 发生内容变化的路径引用。
    pub changed_path_refs: Vec<String>,
    /// 已删除的路径引用。
    pub deleted_path_refs: Vec<String>,
    /// 项目根目录。
    pub project_root: PathBuf,
    /// 执行时间。
    pub now_ms: u64,
}

/// 代码符号增量索引执行报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolIncrementalIndexReport {
    /// 失效计划。
    pub plan: MemoryCodeSymbolInvalidationPlan,
    /// 应用后的索引快照。
    pub records: Vec<MemoryCodeSymbolRecord>,
    /// 重建路径数。
    pub rebuilt_path_count: usize,
    /// 重建符号数。
    pub rebuilt_symbol_count: usize,
    /// 移除旧符号数。
    pub removed_symbol_count: usize,
    /// 是否保持 ref-only。
    pub ref_only: bool,
    /// 是否允许远控读取源码正文。
    pub remote_source_read_allowed: bool,
}

/// 代码符号记忆召回报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCodeSymbolRecallReport {
    /// 扫描记录数。
    pub scanned_records: usize,
    /// 返回命中数。
    pub hit_count: usize,
    /// 是否因 limit 截断。
    pub truncated: bool,
    /// 是否保持 ref-only。
    pub ref_only: bool,
    /// 是否允许远控读取代码正文。
    pub remote_source_read_allowed: bool,
    /// 命中列表。
    pub hits: Vec<MemoryCodeSymbolHit>,
}
