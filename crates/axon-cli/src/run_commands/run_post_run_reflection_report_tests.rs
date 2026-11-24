//! 反思重试的报告复用：重试必须读取首次落盘的正文，而不是重新调模型。

use super::*;

/// 反思重试必须复用首次落盘的报告正文，不重新调模型。
#[test]
fn reflection_retry_reuses_the_stashed_report() {
    let root = test_root("reflection-report-reuse");
    let config_path = root.join("config.toml");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let request = request_fixture_with_executor(
        root.clone(),
        config_path,
        Arc::new(CapturingSelfIterationExecutor {
            captured: captured.clone(),
        }),
    );
    let response = response_with_receipts(&[
        receipt(
            "call-edit",
            "builtin.edit",
            "completed",
            "patched files",
            true,
        ),
        receipt(
            "call-test",
            "builtin.shell_command",
            "completed",
            "验证通过",
            false,
        ),
    ]);
    let envelope = build_turn_outcome(&request, &response);
    let assessment = assessment_for_outcome(&envelope);
    let cards = reflection_cards(
        &TurnTrace::from_outcome(
            &envelope,
            "trace:reuse:1",
            1_000,
            ExplicitSignals::default(),
        ),
        Some(&assessment),
        &request.cwd,
    );
    let plan = SelfIterationRuntime
        .plan_with_delegation(
            SessionId(1),
            false,
            &self_iteration_input(&request, envelope, Some(assessment), cards),
        )
        .expect("self-iteration 计划应有效");
    let entry = plan
        .delegation_plan
        .as_ref()
        .and_then(|plan| {
            plan.entries
                .iter()
                .find(|entry| entry.label == "memory_update")
        })
        .expect("反思计划必须包含 memory_update");

    let first = execute_self_iteration_subagent(
        &request,
        entry,
        SelfIterationExecutionOptions::reflection(),
    )
    .expect("首次执行必须产出报告");
    let second = execute_self_iteration_subagent(
        &request,
        entry,
        SelfIterationExecutionOptions::reflection(),
    )
    .expect("重试必须复用暂存报告");

    assert_eq!(first.summary, second.summary, "重试必须拿到同一份正文");
    assert_eq!(
        captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1,
        "重试不得重新调用子代理模型"
    );
    let _ = std::fs::remove_dir_all(root);
}
