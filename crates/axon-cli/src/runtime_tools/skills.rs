use std::path::Path;

use sha2::{Digest as _, Sha256};

use axon_agent_runtime::{
    InvokeSkillInput, SkillInjectionReport, SkillLoadOutcome, SkillRenderBudget, SkillsManager,
    SkillsRenderer, build_skill_injection,
    skills::{
        invocation::render_skill_invocation,
        manager::{push_user_root, push_workspace_root},
    },
};

use super::runtime_home;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeSkillsSnapshot {
    outcome: SkillLoadOutcome,
    revision: String,
}

impl RuntimeSkillsSnapshot {
    pub(crate) fn select_for_prompt(&self, prompt: &str) -> SkillInjectionReport {
        build_skill_injection(&self.outcome, prompt)
    }

    pub(crate) fn render_available(&self) -> String {
        SkillsRenderer::render_available(&self.outcome, SkillRenderBudget::default()).text
    }

    pub(crate) fn render_relevant(&self, query: &str, context_window: u64) -> String {
        SkillsRenderer::render_relevant(
            &self.outcome,
            query,
            SkillRenderBudget::for_context_window(context_window),
        )
        .text
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }
}

pub(crate) fn load_runtime_skills_snapshot(
    config_path: &Path,
    cwd: Option<&Path>,
) -> Result<RuntimeSkillsSnapshot, String> {
    let manager = build_runtime_skills_manager(config_path, cwd)?;
    let outcome = manager.load();
    let revision = skill_catalog_revision(&outcome);
    Ok(RuntimeSkillsSnapshot { outcome, revision })
}

pub(crate) fn execute_runtime_skill_invocation(
    config_path: &Path,
    cwd: Option<&Path>,
    input: &InvokeSkillInput,
) -> Result<String, String> {
    let snapshot = load_runtime_skills_snapshot(config_path, cwd)?;
    let skill = snapshot
        .outcome
        .skills
        .into_iter()
        .find(|skill| skill.metadata.name == input.skill_name)
        .ok_or_else(|| format!("未找到 Skill：{}", input.skill_name))?;
    render_skill_invocation(&skill, None)
}

pub(crate) fn build_runtime_skills_manager(
    config_path: &Path,
    cwd: Option<&Path>,
) -> Result<SkillsManager, String> {
    let workspace = super::resolve_workspace_cwd(config_path, cwd)?;
    build_runtime_skills_manager_with_home(workspace.as_deref(), runtime_home().as_deref())
}

fn build_runtime_skills_manager_with_home(
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Result<SkillsManager, String> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        push_user_root(&mut roots, home);
    }
    if let Some(cwd) = cwd {
        push_workspace_root(&mut roots, cwd);
    }
    Ok(SkillsManager::with_roots(roots))
}

fn skill_catalog_revision(outcome: &SkillLoadOutcome) -> String {
    let mut entries = outcome
        .skills
        .iter()
        .map(|skill| {
            format!(
                "{}:{}:{}",
                skill.metadata.source_ref(),
                skill.metadata.name,
                skill.metadata.content_digest
            )
        })
        .collect::<Vec<_>>();
    entries.sort();
    hex::encode(Sha256::digest(entries.join("\n").as_bytes()))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use axon_agent_runtime::{SkillRenderBudget, SkillsRenderer};

    use super::{build_runtime_skills_manager_with_home, skill_catalog_revision};

    #[test]
    fn runtime_manager_uses_only_supported_roots_with_user_precedence() {
        let temp = temp_root("official-skill-roots");
        let runtime_root = temp.join("runtime");
        let home = temp.join("home");
        let workspace = temp.join("workspace");
        write_skill(
            &runtime_root.join("docs/skills"),
            "release",
            "installation instructions",
        );
        write_skill(&home.join(".axon/skills"), "review", "user instructions");
        write_skill(&home.join(".axon/skills"), "release", "global instructions");
        write_skill(
            &workspace.join(".axon/skills"),
            "review",
            "workspace instructions",
        );
        write_skill(
            &workspace.join(".axon/skills"),
            "workspace-only",
            "workspace-only instructions",
        );

        let manager =
            build_runtime_skills_manager_with_home(Some(&workspace), Some(&home)).unwrap();
        let outcome = manager.load();

        assert_eq!(outcome.errors, []);
        let release = outcome
            .skills
            .iter()
            .find(|skill| skill.metadata.name == "release")
            .unwrap();
        assert_eq!(release.metadata.source_ref(), "skill://user/release");
        assert_eq!(release.content, "global instructions");
        let skill = outcome
            .skills
            .iter()
            .find(|skill| skill.metadata.name == "review")
            .unwrap();
        assert_eq!(skill.metadata.source_ref(), "skill://user/review");
        assert_eq!(skill.content, "user instructions");
        let workspace_only = outcome
            .skills
            .iter()
            .find(|skill| skill.metadata.name == "workspace-only")
            .unwrap();
        assert_eq!(
            workspace_only.metadata.source_ref(),
            "skill://workspace/workspace-only"
        );
        let context = SkillsRenderer::render_available(&outcome, SkillRenderBudget::default()).text;
        assert!(context.contains("skill://user/review"));
        assert_eq!(context.matches("name: review").count(), 1);
        assert!(context.contains("name: release"));
        assert!(context.contains("context_scope: global"));
        assert!(context.contains("context_scope: workspace"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn catalog_revision_changes_only_after_snapshot_reload() {
        let temp = temp_root("skill-catalog-revision");
        let home = temp.join("home");
        let manager = build_runtime_skills_manager_with_home(None, Some(&home)).unwrap();
        let current_outcome = manager.load();
        let current_revision = skill_catalog_revision(&current_outcome);

        write_skill(&home.join(".axon/skills"), "review", "first revision");
        assert_eq!(skill_catalog_revision(&current_outcome), current_revision);

        let next_outcome = manager.load();
        assert_ne!(skill_catalog_revision(&next_outcome), current_revision);
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn workspace_switch_replaces_workspace_skills_and_keeps_user_winners() {
        let temp = temp_root("workspace-skill-switch");
        let home = temp.join("home");
        let first = temp.join("first");
        let second = temp.join("second");
        write_skill(&home.join(".axon/skills"), "shared", "user shared");
        write_skill(&first.join(".axon/skills"), "shared", "first shared");
        write_skill(&first.join(".axon/skills"), "first-only", "first body");
        write_skill(&second.join(".axon/skills"), "shared", "second shared");
        write_skill(&second.join(".axon/skills"), "second-only", "second body");

        let first_outcome = build_runtime_skills_manager_with_home(Some(&first), Some(&home))
            .unwrap()
            .load();
        let second_outcome = build_runtime_skills_manager_with_home(Some(&second), Some(&home))
            .unwrap()
            .load();

        assert!(
            first_outcome
                .skills
                .iter()
                .any(|skill| skill.metadata.name == "first-only")
        );
        assert!(
            second_outcome
                .skills
                .iter()
                .all(|skill| skill.metadata.name != "first-only")
        );
        assert!(
            second_outcome
                .skills
                .iter()
                .any(|skill| skill.metadata.name == "second-only")
        );
        let shared = second_outcome
            .skills
            .iter()
            .find(|skill| skill.metadata.name == "shared")
            .unwrap();
        assert_eq!(shared.metadata.source_ref(), "skill://user/shared");
        assert_eq!(shared.content, "user shared");
        assert_eq!(
            second_outcome
                .skills
                .iter()
                .filter(|skill| skill.metadata.name == "shared")
                .count(),
            1
        );
        let _ = fs::remove_dir_all(temp);
    }

    fn write_skill(root: &Path, name: &str, body: &str) {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test Skill\n---\n{body}"),
        )
        .unwrap();
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        axon_test_support::test_temp_root()
            .join(format!("axon-{label}-{}-{nanos}", std::process::id()))
    }
}
