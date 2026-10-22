use axon_agent_runtime::{SkillRoot, SkillScope, SkillsManager};

use super::*;

const VALID_SKILL: &str =
    "---\nname: release-review\ndescription: 验证发布流程\n---\n\n# 发布流程\n";

#[test]
fn existing_skill_update_preserves_revision_and_passes_eval() {
    let root = axon_test_support::test_temp_dir();
    let skill_file = root.path().join("release-review/SKILL.md");
    fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
    fs::write(&skill_file, VALID_SKILL).unwrap();

    let result = update_existing_skill(&skill_file, "release-review", "完成后校验产物")
        .unwrap()
        .unwrap();

    assert!(
        fs::read_to_string(&skill_file)
            .unwrap()
            .contains("完成后校验产物")
    );
    assert!(skill_file.parent().unwrap().join(".revisions").is_dir());
    assert_eq!(result.revision.len(), 64);
}

#[test]
fn invalid_update_leaves_existing_skill_unchanged() {
    let root = axon_test_support::test_temp_dir();
    let skill_file = root.path().join("release-review/SKILL.md");
    fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
    fs::write(&skill_file, "# invalid\n").unwrap();

    assert!(update_existing_skill(&skill_file, "release-review", "不能写入").is_err());
    assert_eq!(fs::read_to_string(skill_file).unwrap(), "# invalid\n");
}

#[test]
fn promoted_skill_appears_only_after_catalog_reload() {
    let root = axon_test_support::test_temp_dir();
    let store = SkillEvolutionStore::open_at(root.path()).unwrap();
    let user_skills = root.path().join("user-skills");
    let manager =
        SkillsManager::with_roots(vec![SkillRoot::new(&user_skills, SkillScope::User, 1)]);
    let current_session = manager.load();
    let name = draft_skill_name("每次发布前执行格式、编译和测试验证");
    write_validated_draft(&store, &name, "每次发布前执行格式、编译和测试验证").unwrap();
    promote_validated_draft(&store, &name, &user_skills).unwrap();

    assert!(current_session.skills.is_empty());
    assert!(
        manager
            .load()
            .skills
            .iter()
            .any(|skill| skill.metadata.name == name)
    );
}
