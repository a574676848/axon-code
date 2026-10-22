//! 官方 Skill 领域模型。

use std::fmt;
use std::path::PathBuf;

/// 官方 Skill 来源作用域，优先级依次为系统、用户、工作区。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillScope {
    /// 随 Axon 发布的系统 Skill。
    System,
    /// 当前用户安装的 Skill。
    User,
    /// 当前工作区声明的 Skill。
    Workspace,
}

impl SkillScope {
    /// 返回稳定的来源标签。
    #[must_use]
    pub const fn source_label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }

    /// 返回 Provider 动态上下文中的有效作用域。
    #[must_use]
    pub const fn context_scope(self) -> &'static str {
        match self {
            Self::System | Self::User => "global",
            Self::Workspace => "workspace",
        }
    }
}

/// Axon runtime 可扫描的官方 Skill 根目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    /// Skill 根目录路径。
    pub path: PathBuf,
    /// 根目录所属作用域。
    pub scope: SkillScope,
    /// 根目录覆盖优先级。
    pub precedence: u8,
}

impl SkillRoot {
    /// 创建 Skill 根目录描述。
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, scope: SkillScope, precedence: u8) -> Self {
        Self {
            path: path.into(),
            scope,
            precedence,
        }
    }
}

/// 已验证的官方 Skill 元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMetadata {
    /// registry 中的官方 Skill 名称。
    pub name: String,
    /// 官方 manifest 描述。
    pub description: String,
    /// `SKILL.md` 文件路径。
    pub path: PathBuf,
    /// Skill 所在目录。
    pub skill_dir: PathBuf,
    /// Skill 来源作用域。
    pub scope: SkillScope,
    /// Skill 正文 SHA-256 摘要。
    pub content_digest: String,
}

impl SkillMetadata {
    /// 返回稳定的 Skill 来源引用。
    #[must_use]
    pub fn source_ref(&self) -> String {
        format!("skill://{}/{}", self.scope.source_label(), self.name)
    }
}

/// 完整 Skill snapshot。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// 已验证的 Skill 元数据。
    pub metadata: SkillMetadata,
    /// 不可变 Skill 正文。
    pub content: String,
}

/// 单个 Skill 加载失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillLoadError {
    /// 加载失败的路径。
    pub path: PathBuf,
    /// 稳定的失败原因。
    pub message: String,
}

/// 根目录加载结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillLoadOutcome {
    /// 成功加载的 Skill snapshot。
    pub skills: Vec<Skill>,
    /// 各路径的加载错误。
    pub errors: Vec<SkillLoadError>,
}

impl fmt::Display for SkillScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.source_label())
    }
}
