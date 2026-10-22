//! 官方 Skill 调用渲染。

use super::model::Skill;
use super::render::SkillsRenderer;

/// 渲染已解析的 Skill 指令；Skill 本身不执行外部动作。
pub fn render_skill_invocation(skill: &Skill, user_prompt: Option<&str>) -> Result<String, String> {
    Ok(SkillsRenderer::render_selected(skill, user_prompt))
}
