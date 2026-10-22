//! Git diff semantic analyzer contract types.

/// Git 操作错误类型，对标 codex GitToolingError
#[derive(Clone, PartialEq, Eq)]
pub enum GitAnalyzerError {
    /// Git 命令执行超时
    Timeout {
        /// 超时时长（毫秒）
        duration_ms: u64,
    },
    /// Git 命令执行失败
    CommandFailed {
        /// 错误消息
        message: String,
    },
    /// Git 输出解析失败
    ParseError {
        /// 错误消息
        message: String,
    },
    /// 路径不存在
    PathNotFound {
        /// 文件路径
        path: String,
    },
    /// 非 Git 仓库
    NotARepository {
        /// 仓库路径
        path: String,
    },
    /// 其他错误
    Other {
        /// 错误消息
        message: String,
    },
}

impl core::fmt::Display for GitAnalyzerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Timeout { duration_ms } => {
                write!(
                    f,
                    "Git command timed out after {}",
                    axon_display::format_duration_millis_compact(*duration_ms)
                )
            }
            Self::CommandFailed { message } => write!(f, "Git command failed: {}", message),
            Self::ParseError { message } => write!(f, "Failed to parse git output: {}", message),
            Self::PathNotFound { path } => write!(f, "Path not found: {}", path),
            Self::NotARepository { path } => write!(f, "Not a git repository: {}", path),
            Self::Other { message } => write!(f, "{}", message),
        }
    }
}

impl core::fmt::Debug for GitAnalyzerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self)
    }
}

impl std::error::Error for GitAnalyzerError {}

/// Result type alias for this module
pub type GitAnalyzerResult<T> = Result<T, GitAnalyzerError>;

/// Baseline 变更类型，对标 codex GitBaselineChange
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitBaselineChange {
    /// 新增文件
    Added,
    /// 修改文件
    Modified,
    /// 删除文件
    Deleted,
}

/// Baseline 追踪器，对标 codex gix crate 的 baseline 功能
#[derive(Debug, Clone)]
pub struct GitBaseline {
    /// 当前 commit 的 SHA
    pub head_sha: String,
    /// 基准 commit 的 SHA
    pub base_sha: String,
    /// 变更的文件列表
    pub changed_files: Vec<BaselineFileChange>,
}

/// Baseline 文件变更
#[derive(Debug, Clone)]
pub struct BaselineFileChange {
    /// 文件路径
    pub path: String,
    /// 变更类型
    pub change_type: GitBaselineChange,
}

/// 变更语义类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSemantic {
    /// 功能添加
    FeatureAddition,
    /// Bug 修复
    BugFix,
    /// 性能优化
    PerformanceOptimization,
    /// 代码重构
    Refactoring,
    /// 配置变更
    ConfigurationChange,
    /// 文档更新
    DocumentationUpdate,
    /// 测试变更
    TestChange,
    /// 依赖变更
    DependencyChange,
    /// UI/样式变更
    UiStyleChange,
    /// 未知类型
    Unknown,
}

/// 风险等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// 低风险
    Low,
    /// 中等风险
    Medium,
    /// 高风险
    High,
    /// 极高风险
    Critical,
}

impl core::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Low => write!(f, "低风险"),
            Self::Medium => write!(f, "中等风险"),
            Self::High => write!(f, "高风险"),
            Self::Critical => write!(f, "极高风险"),
        }
    }
}

/// 单个文件的语义分析结果。
#[derive(Debug, Clone)]
pub struct FileSemanticAnalysis {
    /// 文件路径
    pub path: String,
    /// 语义类型
    pub semantic: ChangeSemantic,
    /// 风险等级
    pub risk_level: RiskLevel,
    /// 增加行数
    pub additions: u32,
    /// 删除行数
    pub deletions: u32,
    /// Hunk 块数
    pub hunks: u32,
    /// 受影响的模块
    pub affected_modules: Vec<String>,
    /// 影响评分
    pub impact_score: f32,
}

/// 文件统计信息。
#[derive(Debug, Clone, Default)]
pub struct FileStats {
    /// 增加行数
    pub additions: u32,
    /// 删除行数
    pub deletions: u32,
    /// 变更文件数
    pub files_changed: u32,
    /// Hunk 块数
    pub hunks: u32,
}

/// 模块影响分析。
#[derive(Debug, Clone)]
pub struct ModuleImpact {
    /// 模块名称
    pub module: String,
    /// 模块内文件列表
    pub files: Vec<String>,
    /// 总增加行数
    pub total_additions: u32,
    /// 总删除行数
    pub total_deletions: u32,
    /// 影响评分
    pub impact_score: f32,
}

/// 语义化差异分析结果。
#[derive(Debug, Clone)]
pub struct SemanticDiff {
    /// 文件分析结果列表
    pub files: Vec<FileSemanticAnalysis>,
    /// 整体风险等级
    pub overall_risk: RiskLevel,
    /// 统计信息
    pub stats: FileStats,
    /// 模块影响分析
    pub module_impacts: Vec<ModuleImpact>,
    /// 汇总信息
    pub summary: String,
}

impl Default for SemanticDiff {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            overall_risk: RiskLevel::Low,
            stats: FileStats::default(),
            module_impacts: Vec::new(),
            summary: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RiskThresholds {
    /// 高风险文件扩展名
    pub high_risk_extensions: Vec<&'static str>,
    /// 关键路径
    pub critical_paths: Vec<&'static str>,
}

impl RiskThresholds {
    /// 检查文件扩展名是否为高风险。
    pub fn is_high_risk_extension(&self, ext: &str) -> bool {
        self.high_risk_extensions.contains(&ext)
    }
}
