use super::super::child_persistence::SUBAGENT_RESULT_INLINE_CHAR_LIMIT;
use super::*;
use std::path::Path;

pub(super) fn assert_completed_child_thread(config_path: &Path, task_path: &AgentPath) {
    let child_session_id = child_session_id("parent-session", task_path);
    let root = super::super::super::run_session_store::resolved_axon_root(config_path).unwrap();
    let detail = ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, &child_session_id)
        .unwrap();
    assert_eq!(detail.thread.status, ThreadStatus::Idle);
    assert_eq!(detail.turns.len(), 1);
    assert_eq!(detail.turns[0].status, TurnStatus::Completed);
}

#[test]
fn child_thread_is_active_before_provider_turn_and_closes_interrupted() {
    let config_path = temp_config_path("child-thread-lifecycle");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let executor = ProviderRunSubagentExecutor::new(fixture_environment(config_path.clone()));
    let input = fixture_execution_input("/root/thread_lifecycle");
    let child_request = executor.child_request(&input, None).unwrap();

    persist_child_turn_started(&child_request);

    let root = super::super::super::run_session_store::resolved_axon_root(&config_path).unwrap();
    let service = ThreadService::new(JsonlThreadEventStore);
    let active = service
        .read_thread(&root, &child_request.session_id)
        .unwrap();
    assert_eq!(active.thread.status, ThreadStatus::Active);
    assert_eq!(
        active.thread.parent_thread_id.as_deref(),
        Some("parent-session")
    );
    assert_eq!(active.turns.len(), 1);
    assert_eq!(active.turns[0].status, TurnStatus::Active);

    persist_child_turn_terminal(&child_request, "interrupted", "用户中断", None);
    persist_child_turn_terminal(&child_request, "interrupted", "用户中断", None);

    let interrupted = service
        .read_thread(&root, &child_request.session_id)
        .unwrap();
    assert_eq!(interrupted.thread.status, ThreadStatus::Idle);
    assert_eq!(interrupted.turns.len(), 1);
    assert_eq!(interrupted.turns[0].status, TurnStatus::Interrupted);
    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}

#[test]
fn short_subagent_result_stays_inline() {
    assert!(!should_persist_subagent_result(
        &"a".repeat(SUBAGENT_RESULT_INLINE_CHAR_LIMIT)
    ));
    assert!(should_persist_subagent_result(
        &"a".repeat(SUBAGENT_RESULT_INLINE_CHAR_LIMIT + 1)
    ));
    assert!(!should_persist_subagent_result(
        &"中".repeat(SUBAGENT_RESULT_INLINE_CHAR_LIMIT)
    ));
}

#[test]
fn archived_result_path_and_preview_round_trip() {
    // continuation 需要**同时**保存正文与引用：strip 取正文、extract 取磁盘路径，
    // 两者必须来自同一行标记，否则摘要级恢复会丢掉结果的落盘凭据（合同 §5.3.3）。
    let path = "E:\\axon\\data\\subagents\\results\\review.md";
    let summary = append_result_path("审查完成：发现 2 处问题", path);
    assert_eq!(strip_result_path(&summary), "审查完成：发现 2 处问题");
    assert_eq!(extract_result_path(&summary).as_deref(), Some(path));
    assert_eq!(
        extract_result_path("审查完成"),
        None,
        "未归档的摘要没有引用，必须如实返回 None 而不是凭空造路径"
    );
    assert_eq!(
        extract_result_path(&append_result_path("审查完成", "   ")),
        None,
        "空路径不是可解析凭据"
    );
}

#[test]
fn long_subagent_result_is_archived_as_markdown() {
    let home = axon_test_support::test_temp_root()
        .join(format!("axon-subagent-result-{}", std::process::id()));
    let archive_root = home.join("data");
    fs::create_dir_all(&archive_root).unwrap();
    let task_path = AgentPath::parse("/root/review").unwrap();
    let output = RunSubagentExecutionOutput {
        status: RunSubagentExecutionStatus::Completed,
        summary: "审查完成".to_owned(),
        memory_write_count: 2,
        token_usage: None,
        model: Some("fixture".to_owned()),
        budget_exhausted: false,
    };

    let archived = persist_subagent_result(&archive_root, &task_path, 42, &output).unwrap();

    let path = Path::new(&archived.path);
    assert!(path.starts_with(&archive_root));
    assert_eq!(
        path.extension().and_then(|value| value.to_str()),
        Some("md")
    );
    assert_eq!(archived.preview, "审查完成");
    let content = fs::read_to_string(path).unwrap();
    assert!(content.contains("# 子代理回执：/root/review"));
    assert!(content.contains("- 状态：completed"));
    assert!(content.contains("- 模型：fixture"));
    assert!(content.contains("- 记忆写入：2"));
    assert!(content.contains("## 结果正文\n\n审查完成"));
    let _ = fs::remove_dir_all(&home);
}
