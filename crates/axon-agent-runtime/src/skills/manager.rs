//! 官方 Skill catalog 发现入口。

use std::path::{Path, PathBuf};

use super::loader::SkillLoader;
use super::model::{Skill, SkillLoadOutcome, SkillRoot, SkillScope};
use super::render::{SkillRenderBudget, SkillsRenderer};

const USER_PRECEDENCE: u8 = 30;
const WORKSPACE_PRECEDENCE: u8 = 20;
const SKILLS_DIRECTORY: &str = "skills";

/// Cached manager for skill roots.
#[derive(Debug, Clone, Default)]
pub struct SkillsManager {
    roots: Vec<SkillRoot>,
}

impl SkillsManager {
    /// Build an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self { roots: Vec::new() }
    }

    /// Build a manager from explicit roots.
    #[must_use]
    pub fn with_roots(roots: Vec<SkillRoot>) -> Self {
        Self { roots }
    }

    /// 从用户目录和当前工作区发现 Skill。
    #[must_use]
    pub fn from_runtime(cwd: Option<&Path>) -> Self {
        let mut roots = Vec::new();
        if let Some(home) = runtime_user_home_dir() {
            push_user_root(&mut roots, &home);
        }
        if let Some(cwd) = cwd {
            push_workspace_root(&mut roots, cwd);
        }
        Self { roots }
    }

    /// Add a root and return the manager for chaining.
    #[must_use]
    pub fn with_root(mut self, root: SkillRoot) -> Self {
        self.roots.push(root);
        self
    }

    /// Access configured roots.
    #[must_use]
    pub fn roots(&self) -> &[SkillRoot] {
        &self.roots
    }

    /// Load all skills from configured roots.
    #[must_use]
    pub fn load(&self) -> SkillLoadOutcome {
        SkillLoader::load_roots(&self.roots)
    }

    /// 按官方名称精确查找 Skill。
    #[must_use]
    pub fn find(&self, name: &str) -> Option<Skill> {
        self.load()
            .skills
            .into_iter()
            .find(|skill| skill.metadata.name == name)
    }

    /// Render concise available skills.
    #[must_use]
    pub fn render_available(&self, budget: SkillRenderBudget) -> String {
        SkillsRenderer::render_available(&self.load(), budget).text
    }
}

fn runtime_user_home_dir() -> Option<PathBuf> {
    // 用户级 Skill 必须与运行态数据同源：Daemon 服务账户下不能从 USERPROFILE 推断。
    axon_config::user_home_dir().ok()
}

/// 添加用户 Skill 根目录。
pub fn push_user_root(roots: &mut Vec<SkillRoot>, home: &Path) {
    roots.push(SkillRoot::new(
        home.join(".axon").join(SKILLS_DIRECTORY),
        SkillScope::User,
        USER_PRECEDENCE,
    ));
}

/// 添加工作区 Skill 根目录。
pub fn push_workspace_root(roots: &mut Vec<SkillRoot>, workspace: &Path) {
    roots.push(SkillRoot::new(
        workspace.join(".axon").join(SKILLS_DIRECTORY),
        SkillScope::Workspace,
        WORKSPACE_PRECEDENCE,
    ));
}
