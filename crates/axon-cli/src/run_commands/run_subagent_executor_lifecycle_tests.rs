use super::*;

fn lifecycle_input() -> RunSubagentExecutionInput {
    RunSubagentExecutionInput {
        task_path: AgentPath::parse("/root/review").unwrap(),
        message: "审查当前改动".to_owned(),
        fork_context: SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        started_at_ms: crate::tool_orchestrator::now_ms(),
        max_runtime_ms: None,
        model_intent: RunSubagentModelIntent::Inherit,
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
    }
}

#[test]
fn late_completed_output_preserves_interrupted_authoritative_status() {
    let lifecycle = subagent_terminal_lifecycle(
        &lifecycle_input(),
        &RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: "迟到完成".to_owned(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        },
        &AgentStatus::Interrupted,
    );

    assert_eq!(lifecycle.phase, "interrupted");
    assert_eq!(lifecycle.status, WireLifecycleStatus::Cancelled);
}

#[test]
fn removed_agent_projects_closed_instead_of_late_completion() {
    let lifecycle = subagent_terminal_lifecycle(
        &lifecycle_input(),
        &RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: "迟到完成".to_owned(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        },
        &AgentStatus::NotFound,
    );

    assert_eq!(lifecycle.phase, "closed");
    assert_eq!(lifecycle.status, WireLifecycleStatus::Cancelled);
    assert!(lifecycle.summary.contains("/root/review 已关闭"));
}

#[test]
fn late_completion_cleanup_does_not_remove_new_execution() {
    let task_path = AgentPath::parse("/root/review").unwrap();
    let registry = SubagentLifecycleRegistry::new(None, "parent-session".to_owned());
    let first = registry.begin_execution(&task_path);
    let second = registry.begin_execution(&task_path);

    registry.remove(&task_path, &first);

    assert_eq!(
        registry
            .current(&task_path)
            .map(|publisher| publisher.run_id().to_owned()),
        Some(second.run_id().to_owned())
    );
}

/// 六类退出必须投影为**同一结构**的不同取值（合同 §5.2.3）。
#[test]
fn six_terminal_kinds_share_one_projection() {
    let task_path = AgentPath::parse("/root/review").unwrap();
    let cases = [
        (
            JobTerminalKind::Completed,
            "completed",
            WireLifecycleStatus::Completed,
        ),
        (
            JobTerminalKind::Refusal,
            "refused",
            WireLifecycleStatus::Blocked,
        ),
        (
            JobTerminalKind::Failed,
            "errored",
            WireLifecycleStatus::Failed,
        ),
        (
            JobTerminalKind::Killed,
            "killed",
            WireLifecycleStatus::Cancelled,
        ),
        (
            JobTerminalKind::Timeout,
            "timeout",
            WireLifecycleStatus::Cancelled,
        ),
        (
            JobTerminalKind::Unresponsive,
            "unresponsive",
            WireLifecycleStatus::Cancelled,
        ),
    ];
    assert_eq!(cases.len(), 6, "合同 §8.1 第 6 条要求六类退出");

    for (terminal, phase, status) in cases {
        let lifecycle = subagent_explicit_terminal_lifecycle(&task_path, terminal);
        assert_eq!(lifecycle.phase, phase, "{terminal:?} 的 phase 必须唯一");
        assert_eq!(
            lifecycle.status, status,
            "{terminal:?} 的 wire 状态必须一致"
        );
        assert!(
            lifecycle.summary.contains("/root/review"),
            "{terminal:?} 的摘要必须自证归属：{}",
            lifecycle.summary
        );
        assert_eq!(
            lifecycle.id,
            subagent_explicit_terminal_lifecycle(&task_path, JobTerminalKind::Killed).id,
            "同一子代理的六类终态必须落在同一个 lifecycle part 上"
        );
    }
}

/// 拒绝必须投影为 `Blocked` 而不是 `Failed`：拒绝发生在执行之前，
/// 画成失败会让 TUI/Wire 的读者以为子代理真的跑过一轮（合同 §5.2.3）。
#[test]
fn refusal_projects_as_blocked_and_states_the_task_was_not_accepted() {
    let task_path = AgentPath::parse("/root/review").unwrap();
    let lifecycle = subagent_explicit_terminal_lifecycle(&task_path, JobTerminalKind::Refusal);

    assert_eq!(lifecycle.status, WireLifecycleStatus::Blocked);
    assert_ne!(lifecycle.status, WireLifecycleStatus::Failed);
    assert_ne!(lifecycle.status, WireLifecycleStatus::Completed);
    assert!(
        lifecycle.summary.contains("未被接受"),
        "拒绝摘要必须明说任务未被接受：{}",
        lifecycle.summary
    );
}

#[test]
fn provider_executor_interrupt_then_new_execution_uses_new_stream_and_terminal() {
    let config_path = temp_config_path("execution-generation-lifecycle");
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let mut environment = fixture_environment(config_path);
    environment.subagent_event_bus = Some(Arc::new(move |event| {
        captured_events.lock().unwrap().push(event);
        Ok(())
    }));
    let executor = ProviderRunSubagentExecutor::new(environment);
    let task_path = AgentPath::parse("/root/review").unwrap();
    let first_input = RunSubagentExecutionInput {
        task_path: task_path.clone(),
        message: "第一轮审查".to_owned(),
        fork_context: SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        started_at_ms: 100,
        max_runtime_ms: None,
        model_intent: RunSubagentModelIntent::Inherit,
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
    };
    let first = executor.lifecycle_registry.begin_execution(&task_path);
    let first_run_id = first.run_id().to_owned();
    assert!(
        first
            .publish(subagent_started_lifecycle(
                &first_input,
                Some("fixture-chat")
            ))
            .unwrap()
    );
    executor.notify_explicit_terminal(std::slice::from_ref(&task_path), JobTerminalKind::Killed);
    assert!(executor.lifecycle_registry.current(&task_path).is_none());

    let second_input = RunSubagentExecutionInput {
        message: "第二轮审查".to_owned(),
        started_at_ms: 200,
        ..first_input
    };
    let second = executor.lifecycle_registry.begin_execution(&task_path);
    let second_run_id = second.run_id().to_owned();
    assert_ne!(first_run_id, second_run_id);
    assert!(
        second
            .publish(subagent_started_lifecycle(
                &second_input,
                Some("fixture-chat")
            ))
            .unwrap()
    );
    // 墙钟超时与显式终止走同一条终态通路，但投影出的 phase 不同。
    executor.notify_explicit_terminal(std::slice::from_ref(&task_path), JobTerminalKind::Timeout);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    let first_events = events
        .iter()
        .filter(|event| event.event.session.run_id.as_deref() == Some(&first_run_id))
        .collect::<Vec<_>>();
    let second_events = events
        .iter()
        .filter(|event| event.event.session.run_id.as_deref() == Some(&second_run_id))
        .collect::<Vec<_>>();
    assert_eq!(first_events.len(), 2);
    assert_eq!(second_events.len(), 2);
    assert!(matches!(
        &first_events[1].event.kind,
        WireEventKind::TurnLifecycle(lifecycle)
            if lifecycle.status == WireLifecycleStatus::Cancelled
    ));
    assert!(matches!(
        &second_events[1].event.kind,
        WireEventKind::TurnLifecycle(lifecycle)
            if lifecycle.status == WireLifecycleStatus::Cancelled
    ));
}
