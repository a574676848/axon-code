//! 启动快照测试：Workspace 切换替换语义、稳定前缀复用与动态上下文边界。

use super::*;
use axon_config::{AxonConfig, ConfigLoader};
use axon_domain::{AxonOwnerId, CellId, SessionId};
use std::fs;

const TEST_OWNER_ID: &str = "owner_550e8400-e29b-41d4-a716-446655440000";
const FIRST_PREFERENCE: &str = "默认称呼长官";
const SECOND_PREFERENCE: &str = "偏好简体中文";
const FIRST_WORKSPACE_RULE: &str = "workspace-switch-first-rule";
const SECOND_WORKSPACE_RULE: &str = "workspace-switch-second-rule";
const FIRST_WORKSPACE_SKILL: &str = "workspace-switch-first-skill";
const SECOND_WORKSPACE_SKILL: &str = "workspace-switch-second-skill";
const HOME_RULE: &str = "home-must-not-be-workspace-rule";
const HOME_SKILL: &str = "home-must-not-be-workspace-skill";

#[test]
fn resolving_a_new_workspace_replaces_old_agents_and_skills() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-startup-workspace-switch-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ));
    let config_path = root.join("runtime").join("config.toml");
    let first_workspace = root.join("first");
    let second_workspace = root.join("second");
    fs::create_dir_all(config_path.parent().expect("配置路径应有父目录")).unwrap();
    fs::create_dir_all(first_workspace.join(".git")).unwrap();
    fs::create_dir_all(second_workspace.join(".git")).unwrap();
    fs::write(first_workspace.join("AGENTS.md"), FIRST_WORKSPACE_RULE).unwrap();
    fs::write(second_workspace.join("AGENTS.md"), SECOND_WORKSPACE_RULE).unwrap();
    write_workspace_skill(&first_workspace, FIRST_WORKSPACE_SKILL);
    write_workspace_skill(&second_workspace, SECOND_WORKSPACE_SKILL);
    ConfigLoader::save_to_path(&config_path, &AxonConfig::default()).unwrap();
    let first_workspace = first_workspace.canonicalize().unwrap();
    let second_workspace = second_workspace.canonicalize().unwrap();

    let first = Arc::new(RunStartupSnapshot::build(&config_path, &first_workspace).unwrap());
    let second = RunStartupSnapshot::resolve_for_run(
        &config_path,
        Some(&second_workspace),
        Some(Arc::clone(&first)),
    )
    .unwrap();

    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(
        second.workspace_root.as_deref(),
        Some(second_workspace.to_string_lossy().as_ref())
    );
    assert_eq!(
        first.prompt_context.stable_prefix,
        second.prompt_context.stable_prefix
    );
    assert_eq!(
        first.prompt_context.subagent_stable_prefix,
        second.prompt_context.subagent_stable_prefix
    );
    assert_eq!(first.personal_memory, second.personal_memory);
    assert_eq!(first.user_agents, second.user_agents);
    assert!(Arc::ptr_eq(
        &first.memory_write_runtime,
        &second.memory_write_runtime
    ));
    let dynamic_context = second
        .prompt_context
        .workspace_context_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(dynamic_context.contains(SECOND_WORKSPACE_RULE));
    assert!(dynamic_context.contains(SECOND_WORKSPACE_SKILL));
    assert!(!dynamic_context.contains(FIRST_WORKSPACE_RULE));
    assert!(!dynamic_context.contains(FIRST_WORKSPACE_SKILL));
    let available_skills = second.skills.render_available();
    assert!(available_skills.contains(SECOND_WORKSPACE_SKILL));
    assert!(!available_skills.contains(FIRST_WORKSPACE_SKILL));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn personal_snapshot_refreshes_only_at_new_session_boundary() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-startup-personal-refresh-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ));
    let config_path = root.join("runtime").join("config.toml");
    let cwd = root.join("workspace");
    let identity_dir = config_path
        .parent()
        .expect("配置路径应有父目录")
        .join(crate::runtime_principal_context::OWNER_IDENTITY_DIRECTORY);
    fs::create_dir_all(config_path.parent().expect("配置路径应有父目录")).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&identity_dir).unwrap();
    fs::write(
        identity_dir.join(crate::runtime_principal_context::OWNER_IDENTITY_FILE),
        format!(r#"{{"schemaVersion":1,"ownerId":"{TEST_OWNER_ID}"}}"#),
    )
    .unwrap();
    ConfigLoader::save_to_path(&config_path, &AxonConfig::default()).unwrap();
    let owner_id = AxonOwnerId::parse(TEST_OWNER_ID).unwrap();
    let runtime = AgentMemoryWriteRuntime::new(Some(owner_id), None);

    write_preference(&runtime, &config_path, &cwd, 1, FIRST_PREFERENCE);
    let first_session = RunStartupSnapshot::build(&config_path, &cwd).unwrap();
    let first_snapshot = first_session
        .personal_memory
        .as_ref()
        .expect("新会话应加载 Personal Snapshot");
    assert_eq!(first_snapshot.context, [FIRST_PREFERENCE]);

    write_preference(&runtime, &config_path, &cwd, 2, SECOND_PREFERENCE);
    assert_eq!(first_snapshot.context, [FIRST_PREFERENCE]);

    let second_session = RunStartupSnapshot::build(&config_path, &cwd).unwrap();
    let second_snapshot = second_session
        .personal_memory
        .as_ref()
        .expect("下一会话应刷新 Personal Snapshot");
    assert_eq!(
        second_snapshot.context,
        [SECOND_PREFERENCE.to_owned(), FIRST_PREFERENCE.to_owned()]
    );
    assert_ne!(first_snapshot.revision, second_snapshot.revision);
    let _ = fs::remove_dir_all(root);
}

fn write_preference(
    runtime: &AgentMemoryWriteRuntime,
    config_path: &Path,
    cwd: &Path,
    id: u64,
    summary: &str,
) {
    runtime
        .trigger(super::super::AgentMemoryWriteInput {
            config_path,
            cwd,
            session_id: SessionId(id),
            cell_id: CellId(id),
            kind: "preference",
            summary,
            evidence: "用户明确表达的稳定偏好",
            supersedes_memory_ids: &[],
            now_ms: id,
            is_root_agent: true,
        })
        .unwrap();
}

fn write_workspace_skill(workspace: &Path, name: &str) {
    let skill_dir = workspace.join(".axon").join("skills").join(name);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Workspace switch test\n---\n{name}"),
    )
    .unwrap();
}

fn dynamic_context(snapshot: &RunStartupSnapshot) -> String {
    snapshot
        .prompt_context
        .workspace_context_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_dynamic_context_includes(snapshot: &RunStartupSnapshot, markers: &[&str]) {
    let context = dynamic_context(snapshot);
    assert!(markers.iter().all(|marker| context.contains(marker)));
}

fn assert_dynamic_context_excludes(snapshot: &RunStartupSnapshot, markers: &[&str]) {
    let context = dynamic_context(snapshot);
    assert!(markers.iter().all(|marker| !context.contains(marker)));
}

#[test]
fn home_workspace_transitions_replace_only_workspace_context() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-startup-home-switch-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ));
    let home = root.join("home");
    let home_child = home.join("data");
    let workspace = root.join("workspace");
    let config_path = home.join("config.toml");
    fs::create_dir_all(&home_child).unwrap();
    fs::create_dir_all(workspace.join(".git")).unwrap();
    fs::write(home.join("AGENTS.md"), HOME_RULE).unwrap();
    fs::write(workspace.join("AGENTS.md"), SECOND_WORKSPACE_RULE).unwrap();
    write_workspace_skill(&home_child, HOME_SKILL);
    write_workspace_skill(&workspace, SECOND_WORKSPACE_SKILL);
    ConfigLoader::save_to_path(&config_path, &AxonConfig::default()).unwrap();
    let home_child = home_child.canonicalize().unwrap();
    let workspace = workspace.canonicalize().unwrap();

    let at_home = Arc::new(RunStartupSnapshot::build(&config_path, &home_child).unwrap());
    assert!(at_home.workspace_root.is_none());
    assert!(at_home.workspace_instructions.is_empty());
    assert_dynamic_context_excludes(&at_home, &[HOME_RULE, HOME_SKILL]);

    let at_workspace = RunStartupSnapshot::resolve_for_run(
        &config_path,
        Some(&workspace),
        Some(Arc::clone(&at_home)),
    )
    .unwrap();
    assert_eq!(
        at_workspace.workspace_root.as_deref(),
        Some(workspace.to_string_lossy().as_ref())
    );
    assert_dynamic_context_includes(
        &at_workspace,
        &[SECOND_WORKSPACE_RULE, SECOND_WORKSPACE_SKILL],
    );

    let back_home = RunStartupSnapshot::resolve_for_run(
        &config_path,
        Some(&home_child),
        Some(Arc::clone(&at_workspace)),
    )
    .unwrap();
    assert!(back_home.workspace_root.is_none());
    assert!(back_home.workspace_instructions.is_empty());
    assert_eq!(
        at_home.prompt_context.stable_prefix,
        back_home.prompt_context.stable_prefix
    );
    assert_eq!(
        at_home.prompt_context.subagent_stable_prefix,
        back_home.prompt_context.subagent_stable_prefix
    );
    assert_eq!(at_home.personal_memory, back_home.personal_memory);
    assert_eq!(at_home.user_agents, back_home.user_agents);
    assert_dynamic_context_excludes(
        &back_home,
        &[
            HOME_RULE,
            HOME_SKILL,
            SECOND_WORKSPACE_RULE,
            SECOND_WORKSPACE_SKILL,
        ],
    );
    let _ = fs::remove_dir_all(root);
}
