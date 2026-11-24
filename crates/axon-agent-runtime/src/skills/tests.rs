use std::fs;

use super::{
    SkillLoader, SkillRenderBudget, SkillRoot, SkillScope, SkillsManager, SkillsRenderer,
    build_skill_injection,
};

fn write_skill(root: &std::path::Path, name: &str, description: &str, body: &str) {
    let directory = root.join(name);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
    )
    .unwrap();
}

#[test]
fn loader_accepts_only_official_skill_manifest_and_directory_layout() {
    let temp = axon_test_support::test_temp_dir();
    write_skill(temp.path(), "review", "Review code", "Review body");

    let outcome = SkillLoader::load_roots(&[SkillRoot::new(temp.path(), SkillScope::Workspace, 1)]);

    assert_eq!(outcome.skills.len(), 1);
    assert_eq!(outcome.skills[0].metadata.name, "review");
    assert_eq!(
        outcome.skills[0].metadata.source_ref(),
        "skill://workspace/review"
    );
    assert_eq!(outcome.skills[0].content, "Review body");
    assert!(outcome.errors.is_empty());
}

#[test]
fn loader_accepts_mixed_frontmatter_line_endings() {
    let temp = axon_test_support::test_temp_dir();
    let directory = temp.path().join("epaas-development-guide");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        "---\r\nname: epaas-development-guide\r\ndescription: EPAAS 开发指南\n---\r\n正文",
    )
    .unwrap();

    let outcome = SkillLoader::load_roots(&[SkillRoot::new(temp.path(), SkillScope::User, 1)]);

    assert!(outcome.errors.is_empty());
    assert_eq!(outcome.skills.len(), 1);
    assert_eq!(outcome.skills[0].metadata.name, "epaas-development-guide");
    assert_eq!(outcome.skills[0].content, "正文");
}

#[test]
fn loader_accepts_additional_manifest_fields_and_rejects_invalid_names() {
    let temp = axon_test_support::test_temp_dir();
    let extended = temp.path().join("extended");
    fs::create_dir_all(&extended).unwrap();
    fs::write(
        extended.join("SKILL.md"),
        "---\nname: extended\ndescription: Extended\nlicense: MIT\nmetadata:\n  author: axon\n---\nBody",
    )
    .unwrap();
    let invalid = temp.path().join("Invalid");
    fs::create_dir_all(&invalid).unwrap();
    fs::write(
        invalid.join("SKILL.md"),
        "---\nname: Invalid_Name\ndescription: Invalid\n---\nBody",
    )
    .unwrap();

    let outcome = SkillLoader::load_roots(&[SkillRoot::new(temp.path(), SkillScope::User, 1)]);

    assert_eq!(outcome.skills.len(), 1);
    assert_eq!(outcome.skills[0].metadata.name, "extended");
    assert_eq!(outcome.errors.len(), 1);
}

#[test]
fn manager_uses_user_precedence_and_exact_names() {
    let temp = axon_test_support::test_temp_dir();
    let user = temp.path().join("user");
    let workspace = temp.path().join("workspace");
    write_skill(&user, "ship", "Ship", "user");
    write_skill(&workspace, "ship", "Ship", "workspace");

    let manager = SkillsManager::with_roots(vec![
        SkillRoot::new(&user, SkillScope::User, 30),
        SkillRoot::new(&workspace, SkillScope::Workspace, 20),
    ]);

    assert_eq!(manager.find("ship").unwrap().content, "user");
    assert!(manager.find("SHIP").is_none());
}

#[test]
fn injection_keeps_exact_skill_name_and_digest() {
    let temp = axon_test_support::test_temp_dir();
    write_skill(temp.path(), "review", "Review code", "Review body");
    let outcome = SkillLoader::load_roots(&[SkillRoot::new(temp.path(), SkillScope::Workspace, 1)]);

    let report = build_skill_injection(&outcome, "请使用 skill://workspace/review");
    assert_eq!(report.selected_skill_name(), Some("review"));
    assert_eq!(
        report.selected.as_ref().unwrap().content_digest,
        outcome.skills[0].metadata.content_digest
    );
    assert_eq!(report.user_prompt, "请使用");
    assert!(
        build_skill_injection(&outcome, "skill://workspace/REVIEW")
            .selected
            .is_none()
    );
}

#[test]
fn renderer_lists_official_skill_contract() {
    let temp = axon_test_support::test_temp_dir();
    write_skill(temp.path(), "review", "Review code", "Review body");
    let outcome = SkillLoader::load_roots(&[SkillRoot::new(temp.path(), SkillScope::Workspace, 1)]);

    let report = SkillsRenderer::render_available(&outcome, SkillRenderBudget::default());
    assert!(report.text.contains("digest:"));
    assert!(report.text.contains("skill://workspace/review"));
    assert!(
        !report.text.contains("load_errors="),
        "没有加载失败时不得出现计数行：{}",
        report.text
    );
}

/// 技能加载失败必须在渲染文本里留痕：模型看不到日志，只能读这段清单。
#[test]
fn renderer_surfaces_skill_load_error_count() {
    let temp = axon_test_support::test_temp_dir();
    write_skill(temp.path(), "review", "Review code", "Review body");
    // 缺 frontmatter 的目录会被加载器判为无效并记一条错误。
    let broken = temp.path().join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("SKILL.md"), "没有 frontmatter 的正文").unwrap();

    let outcome = SkillLoader::load_roots(&[SkillRoot::new(temp.path(), SkillScope::Workspace, 1)]);
    assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);

    let report = SkillsRenderer::render_available(&outcome, SkillRenderBudget::default());
    assert_eq!(report.errors, 1);
    assert!(
        report.text.trim_end().ends_with("load_errors=1"),
        "计数行必须落在渲染文本尾部：{}",
        report.text
    );
}

/// 一张技能都没加载成功时同样要留下计数，不能渲染成空串。
#[test]
fn renderer_surfaces_skill_load_error_without_any_listed_skill() {
    let temp = axon_test_support::test_temp_dir();
    let broken = temp.path().join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("SKILL.md"), "没有 frontmatter 的正文").unwrap();

    let outcome = SkillLoader::load_roots(&[SkillRoot::new(temp.path(), SkillScope::Workspace, 1)]);
    let report = SkillsRenderer::render_available(&outcome, SkillRenderBudget::default());

    assert_eq!(report.listed, 0);
    assert_eq!(report.text, "load_errors=1\n");
}
