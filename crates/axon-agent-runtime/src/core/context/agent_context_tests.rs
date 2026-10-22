use axon_domain::{PaneId, PermissionSnapshotId, TokenBudget};

use crate::{
    AGENT_PANE_CHAT_HISTORY_SCHEMA_VERSION, AgentContextRuntime, AgentContextSectionKind,
    AgentContextSectionPayload, AgentContextSnapshotInput, AgentModelPayloadInput,
    AgentModelPayloadRuntime, AgentPaneChatHistoryRecord, AgentPaneGitState, AgentPaneProjection,
    AgentPaneProjectionInput, AgentPromptContext, AgentPromptEngineeringRuntime,
    CommandActivityContext, CommandActivityKind, CommandErrorKind, CommandErrorSpan,
    RecentCommandState, RepoStructureModelRuntime, RepoStructureQuery,
};

#[test]
fn agent_context_snapshot_summarizes_agent_state() {
    let runtime = AgentContextRuntime;

    let snapshot = runtime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(7),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: Some(AgentPaneGitState {
            branch: "master".to_owned(),
            dirty: true,
        }),
        shell_history: vec![
            RecentCommandState::new("cargo fmt --all --check", Some(0)),
            failed_rustc_command("cargo check", "src/main.rs", 42, 9, 900),
        ],
        command_activity: None,
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(2048).unwrap(),
        reserved_for_tools: TokenBudget::new(256).unwrap(),
    });

    assert_eq!(snapshot.pane_id, PaneId(7));
    assert_eq!(snapshot.cwd, "E:\\zbg\\axon-code");
    assert_eq!(snapshot.sections[0].kind(), AgentContextSectionKind::Cwd);
    assert!(matches!(
        &snapshot.sections[0].payload,
        AgentContextSectionPayload::Cwd {
            cwd,
            pane_id
        } if cwd == "E:\\zbg\\axon-code" && *pane_id == PaneId(7)
    ));
    assert!(snapshot.sections.iter().any(|section| {
        section.kind() == AgentContextSectionKind::Cwd && section.content() == "E:\\zbg\\axon-code"
    }));
    assert!(snapshot.sections.iter().any(|section| {
        section.kind() == AgentContextSectionKind::Git
            && section.content() == "branch=master status=dirty"
    }));
    assert!(snapshot.sections.iter().any(|section| {
        section.kind() == AgentContextSectionKind::ShellHistory
            && section.content().contains("cmd=cargo check exit=101")
    }));
    let command_event = snapshot
        .sections
        .iter()
        .find(|section| section.kind() == AgentContextSectionKind::CommandEvent)
        .expect("失败命令应生成 command_event 上下文");
    assert!(matches!(
        &command_event.payload,
        AgentContextSectionPayload::CommandEvent {
            commands,
            pane_id
        } if commands.len() == 1 && commands[0].command == "cargo check" && *pane_id == PaneId(7)
    ));
    let command_event_content = command_event.content();
    assert!(command_event_content.contains("cmd=cargo check"));
    assert!(command_event_content.contains("exit=101"));
    assert!(command_event_content.contains("elapsed=900ms"));
    assert!(!command_event_content.contains("duration_ms="));
    assert!(command_event_content.contains("error[E0308]"));
    assert!(command_event_content.contains("kind=rustc"));
    assert!(command_event_content.contains("file=src/main.rs"));
    assert_eq!(
        snapshot.budget_plan.sections.len(),
        snapshot.sections.len(),
        "所有 AgentContext section 必须进入预算计划"
    );
}

#[test]
fn agent_context_snapshot_preserves_chat_history_beyond_legacy_tail_limit() {
    let chat_history = (0..6)
        .map(|index| AgentPaneChatHistoryRecord {
            schema_version: AGENT_PANE_CHAT_HISTORY_SCHEMA_VERSION,
            pane_id: 7,
            cell_id: index,
            prompt_preview: format!("第 {index} 轮如何修复测试"),
            prompt_chars: 10,
            response_preview: "先运行 cargo test，再查看失败用例".to_owned(),
            response_chars: 18,
            response_truncated: false,
            delta_count: 3,
            source_ref: format!("agent-response://cell/{index}"),
            recorded_at_ms: index,
            remote_full_response_allowed: false,
            provider_raw_payload_persisted: false,
            request_body_persisted: false,
        })
        .collect();
    let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(7),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: None,
        shell_history: Vec::new(),
        command_activity: None,
        apply_history: Vec::new(),
        chat_history,
        total_limit: TokenBudget::new(16_384).unwrap(),
        reserved_for_tools: TokenBudget::new(256).unwrap(),
    });

    let chat_history = snapshot
        .sections
        .iter()
        .find(|section| section.kind() == AgentContextSectionKind::AgentChatHistory)
        .expect("聊天历史应进入上下文");
    let chat_history_content = chat_history.content();
    assert!(chat_history_content.contains("第 0 轮如何修复测试"));
    assert!(chat_history_content.contains("agent-response://cell/5"));
    assert_eq!(chat_history.source_ref(), "agent-chat-history://pane/7");
}

#[test]
fn agent_model_payload_includes_context_snapshot_and_agent_pane() {
    let context_snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(7),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: Some(AgentPaneGitState {
            branch: "master".to_owned(),
            dirty: true,
        }),
        shell_history: vec![failed_rustc_command(
            "cargo check",
            "src/main.rs",
            42,
            9,
            900,
        )],
        command_activity: None,
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(2048).unwrap(),
        reserved_for_tools: TokenBudget::new(256).unwrap(),
    });
    let pane_projection = agent_context_test_pane_projection();

    let payload = AgentModelPayloadRuntime
        .build(&AgentModelPayloadInput {
            user_prompt: "解释当前错误".to_owned(),
            stable_prefix: AgentModelPayloadRuntime.compile_stable_prefix(),
            stable_context_messages: Vec::new(),
            dynamic_context_messages: Vec::new(),
            history_messages: Vec::new(),
            prompt_context: AgentPromptContext::default(),
            runtime_constraints: Some("execution_mode=direct".to_owned()),
            pane_projection,
            context_snapshot,
        })
        .unwrap();

    assert_eq!(payload.messages[0].role, "system");
    let developer_messages = payload
        .messages
        .iter()
        .filter(|message| message.role == "developer")
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    let user_message = payload
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .expect("payload 应包含最终 user message");
    assert!(payload.agent_pane_included);
    assert!(payload.context_section_count >= 4);
    let runtime_constraints = developer_messages
        .iter()
        .find(|content| content.contains("<runtime_constraints>"))
        .expect("payload 应包含 Runtime Constraints XML");
    assert_eq!(
        runtime_constraints.matches("<runtime_constraints>").count(),
        1
    );
    assert!(runtime_constraints.contains("</runtime_constraints>"));
    assert!(
        developer_messages
            .iter()
            .any(|content| content.contains("execution_mode=direct"))
    );
    assert!(
        developer_messages
            .iter()
            .any(|content| content.contains("[Axon Prompt Context]"))
    );
    let delegation_message = payload
        .messages
        .iter()
        .find(|message| message.content.contains("<delegation-policy>"))
        .expect("payload 应包含稳定的子代理委托策略 fragment");
    assert_eq!(delegation_message.role, "developer");
    assert!(!delegation_message.cache_excluded);
    assert!(delegation_message.content.contains("fast 类子代理"));
    assert!(delegation_message.content.contains("持续协作类子代理"));
    assert!(delegation_message.content.contains("按 5W1H 工作契约"));
    assert!(delegation_message.content.contains("同轮并发调用"));
    assert!(
        delegation_message
            .content
            .contains("最终答复前必须用 builtin.job_output 收集仍未终态的任务")
    );
    // 子代理行为指引不得再提被移除的三个工具（名称按片段拼接以配合裁剪扫描）。
    for removed in [
        concat!("wait", "_agent"),
        concat!("interrupt", "_agent"),
        concat!("close", "_agent"),
    ] {
        assert!(
            !delegation_message.content.contains(removed),
            "指引不得再引用被移除的工具：{removed}"
        );
    }
    assert!(delegation_message.content.contains("不得提前发送最终回复"));
    assert!(
        delegation_message
            .content
            .contains("父 Agent 负责复杂推理、合并证据与最终 Check")
    );
    assert!(
        delegation_message
            .content
            .contains("fast 证据不足时由父 Agent 重新 Plan 并自行完成")
    );
    assert_eq!(user_message.content, "解释当前错误");
    let context_message = payload
        .messages
        .iter()
        .find(|message| message.content.contains("<agent-context-snapshot>"))
        .expect("payload 应包含独立的 agent context developer fragment");
    assert_eq!(context_message.role, "developer");
    assert!(
        context_message
            .content
            .contains("branch=master status=dirty")
    );
    assert!(context_message.content.contains("cmd=cargo check exit=101"));
    assert!(context_message.content.contains("kind=rustc"));
    let pane_message = payload
        .messages
        .iter()
        .find(|message| message.content.contains("<agent-pane-context>"))
        .expect("payload 应包含独立的 agent pane developer fragment");
    assert_eq!(pane_message.role, "developer");
    assert!(pane_message.content.contains("当前 pane 使用 powershell"));
}

#[test]
fn agent_context_snapshot_includes_running_command_activity() {
    let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(7),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: None,
        shell_history: Vec::new(),
        command_activity: Some(CommandActivityContext {
            command: "cargo check".to_owned(),
            activity: CommandActivityKind::Running,
            output_tail: "Compiling axon".to_owned(),
            remote_full_context_allowed: false,
            remote_trigger_allowed: false,
        }),
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(2048).unwrap(),
        reserved_for_tools: TokenBudget::new(256).unwrap(),
    });

    let activity = snapshot
        .sections
        .iter()
        .find(|section| section.kind() == AgentContextSectionKind::CommandActivity)
        .expect("运行中命令活动应进入 AgentContext");

    assert_eq!(activity.label(), "command_activity");
    assert_eq!(activity.source_ref(), "command-activity://pane/7");
    let activity_content = activity.content();
    assert!(activity_content.contains("status=running"));
    assert!(activity_content.contains("cmd=cargo check"));
    assert!(activity_content.contains("Compiling axon"));
    assert!(
        snapshot
            .budget_plan
            .sections
            .iter()
            .any(|section| section.label == "command_activity")
    );
}

#[test]
fn agent_context_snapshot_redacts_sensitive_command_context() {
    let secret = "sk-ant-test-context-fixture-invalid-0001";
    let context_snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(7),
        cwd: "/workspace/project".to_owned(),
        git_state: None,
        shell_history: vec![RecentCommandState {
            command: format!("curl --header Authorization:{secret}"),
            exit_code: Some(1),
            output_tail: "request failed api_key=secret-value".to_owned(),
            duration_ms: Some(100),
            detected_errors: vec![CommandErrorSpan {
                kind: CommandErrorKind::Generic,
                message: format!("failed token={secret}"),
                file: None,
                line: None,
                column: None,
            }],
        }],
        command_activity: Some(CommandActivityContext {
            command: format!("curl --header Authorization:{secret}"),
            activity: CommandActivityKind::Running,
            output_tail: "streaming token=ghp_test_context_fixture_0001".to_owned(),
            remote_full_context_allowed: false,
            remote_trigger_allowed: false,
        }),
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(2048).unwrap(),
        reserved_for_tools: TokenBudget::new(256).unwrap(),
    });

    let pane_projection = agent_context_test_pane_projection();
    let payload = AgentModelPayloadRuntime
        .build(&AgentModelPayloadInput {
            user_prompt: "解释当前错误".to_owned(),
            stable_prefix: AgentModelPayloadRuntime.compile_stable_prefix(),
            stable_context_messages: Vec::new(),
            dynamic_context_messages: Vec::new(),
            history_messages: Vec::new(),
            prompt_context: AgentPromptContext::default(),
            runtime_constraints: None,
            pane_projection,
            context_snapshot,
        })
        .unwrap();
    let text = payload
        .messages
        .iter()
        .find(|message| message.content.contains("<agent-context-snapshot>"))
        .expect("payload 应包含 agent context snapshot fragment")
        .content
        .clone();

    assert!(text.contains("[REDACTED]"));
    assert!(!text.contains(secret));
    assert!(!text.contains("secret-value"));
    assert!(!text.contains("ghp_test_context_fixture_0001"));
}

#[test]
fn agent_context_snapshot_includes_repo_structure_summary() {
    let workspace = axon_test_support::test_temp_root().join("axon-agent-context-repo-structure");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
    std::fs::write(workspace.join("main.rs"), "fn main() {}\n").unwrap();
    let report = RepoStructureModelRuntime
        .query_with_lsp_report(
            &RepoStructureQuery {
                workspace_root: workspace.display().to_string(),
                query_text: "main".to_owned(),
                max_files: 32,
                symbol_limit: 4,
                memory_records: Vec::new(),
            },
            None,
        )
        .unwrap();
    let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(7),
        cwd: workspace.display().to_string(),
        git_state: None,
        shell_history: Vec::new(),
        command_activity: None,
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(2048).unwrap(),
        reserved_for_tools: TokenBudget::new(256).unwrap(),
    });
    let snapshot = AgentContextRuntime.inject_repo_structure_summary(snapshot, &report);

    let repo_structure = snapshot
        .sections
        .iter()
        .find(|section| section.kind() == AgentContextSectionKind::RepoStructure)
        .expect("repo structure 摘要应进入上下文");
    assert_eq!(repo_structure.label(), "repo_structure");
    assert!(repo_structure.content().contains("workspace="));
    assert!(repo_structure.source_ref().contains("repo://"));
}

#[test]
fn agent_prompt_engineering_projects_prompt_context_block() {
    let prompt_context = AgentPromptEngineeringRuntime.prompt_context_block(&AgentPromptContext {
        selected_skill_name: Some("release-notes".to_owned()),
        workspace_instruction_paths: vec![
            "AGENTS.md".to_owned(),
            "crates/demo/AGENTS.md".to_owned(),
        ],
    });

    assert!(prompt_context.contains("[Axon Prompt Context]"));
    assert!(prompt_context.contains("selected_skill=release-notes"));
    assert!(prompt_context.contains("workspace_instructions=AGENTS.md, crates/demo/AGENTS.md"));
}

fn failed_rustc_command(
    command: &str,
    file: &str,
    line: u32,
    column: u32,
    duration_ms: u64,
) -> RecentCommandState {
    RecentCommandState {
        command: command.to_owned(),
        exit_code: Some(101),
        output_tail: "error[E0308]: mismatched types".to_owned(),
        duration_ms: Some(duration_ms),
        detected_errors: vec![CommandErrorSpan {
            kind: CommandErrorKind::Rustc,
            message: "mismatched types".to_owned(),
            file: Some(file.to_owned()),
            line: Some(line),
            column: Some(column),
        }],
    }
}

fn agent_context_test_pane_projection() -> AgentPaneProjection {
    AgentPaneProjection::from_input(AgentPaneProjectionInput {
        pane_id: PaneId(7),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        shell_kind: "powershell".to_owned(),
        git_state: Some(AgentPaneGitState {
            branch: "master".to_owned(),
            dirty: true,
        }),
        recent_command: Some(RecentCommandState::new("cargo check", Some(101))),
        permission_snapshot_id: PermissionSnapshotId(42),
        backend_badge: axon_domain::PaneSecurityBadge {
            level: axon_domain::PaneSecurityBadgeLevel::Normal,
            label: "normal".to_owned(),
            reasons: Vec::new(),
        },
        runtime_cell: None,
        response: None,
        apply_execution: None,
    })
}
