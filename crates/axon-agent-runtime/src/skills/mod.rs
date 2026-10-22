//! First-class skill discovery, rendering, and invocation support.

pub mod injection;
pub mod invocation;
pub mod loader;
pub mod manager;
pub mod model;
pub mod render;

#[cfg(test)]
mod tests;

pub use injection::{
    SkillInjectionReport, SkillInvocationRecord, SkillSelectionKind, build_skill_injection,
};
pub use loader::SkillLoader;
pub use manager::SkillsManager;
pub use model::{Skill, SkillLoadError, SkillLoadOutcome, SkillMetadata, SkillRoot, SkillScope};
pub use render::{SkillRenderBudget, SkillRenderReport, SkillsRenderer};
