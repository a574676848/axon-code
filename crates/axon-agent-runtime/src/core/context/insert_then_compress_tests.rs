//! Insert-then-Compress 测试。

use axon_domain::TurnId;

use crate::insert_then_compress::{
    CompressionOutcome, CompressionPolicy, WindowRolloverPolicyRuntime, WindowRolloverRuntime,
    WindowRolloverTransaction, WindowRolloverValidationMode, estimated_history_tokens,
    estimated_history_tokens_for_provider,
};
use crate::{
    AgentModelHistorySnapshot, AgentModelLocalImageRef, AgentModelMessage, AgentModelMessageKind,
    AgentModelToolCallMetadata, AgentRuntimeNoteKind, AnthropicThinkingBlockMetadata,
    CanonicalCheckpoint, CompactedHistory, PromptCacheProviderWireFormat,
};

fn policy(trigger_tokens: u64) -> CompressionPolicy {
    CompressionPolicy {
        trigger_tokens,
        context_window: trigger_tokens.saturating_mul(2),
    }
}

fn message_with_chars(role: &str, chars: usize) -> AgentModelMessage {
    AgentModelMessage::text(role, "x".repeat(chars))
}

fn canonical_checkpoint(messages: &[AgentModelMessage], task: &str) -> CanonicalCheckpoint {
    CanonicalCheckpoint::parse(&checkpoint_summary(task, "- none"), messages)
        .expect("无活动工具调用的 checkpoint 应通过门禁")
}

fn checkpoint_summary(task: &str, tools: &str) -> String {
    format!(
        "P0:\nTASK:\n{task}\nCONSTRAINTS:\n保持已知约束\nVERIFIED:\n已完成已知验证\nOPEN:\n继续下一步\nTOOLS:\n{tools}"
    )
}

#[test]
fn test_policy_for_context_window() {
    let derived = CompressionPolicy::for_context_window(200_000, 0.8)
        .expect("合法阈值")
        .expect("窗口非零");
    assert_eq!(derived.trigger_tokens, 160_000);
    assert_eq!(derived.context_window, 200_000);

    let small = CompressionPolicy::for_context_window(8_000, 0.5)
        .expect("合法阈值")
        .expect("窗口非零");
    assert_eq!(small.trigger_tokens, 4_000);
    assert_eq!(small.context_window, 8_000);

    assert!(
        CompressionPolicy::for_context_window(0, 0.8)
            .expect("合法阈值")
            .is_none()
    );
    assert!(CompressionPolicy::for_context_window(200_000, 1.5).is_err());
    assert!(CompressionPolicy::for_context_window(200_000, -0.1).is_err());
}

#[test]
fn test_estimated_history_tokens() {
    let messages = vec![
        message_with_chars("system", 400),
        message_with_chars("user", 401),
    ];
    // 每条消息先包含完整 OpenAI Chat framing，再对单 item 整体字节数执行一次 ceil(bytes / 4)。
    assert_eq!(estimated_history_tokens(&messages), 216);
    assert_eq!(
        estimated_history_tokens(&[AgentModelMessage::text("user", "中文")]),
        9
    );
    assert_eq!(estimated_history_tokens(&[]), 0);
}

#[test]
fn provider_estimator_rounds_each_visible_item_once_and_tracks_protocol_framing() {
    let message = AgentModelMessage::text("user", "x").with_source_ref("session://s/1");
    let chat = estimated_history_tokens_for_provider(
        std::slice::from_ref(&message),
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        &[],
    );
    let responses = estimated_history_tokens_for_provider(
        std::slice::from_ref(&message),
        PromptCacheProviderWireFormat::OpenAiResponses,
        &[],
    );
    let anthropic = estimated_history_tokens_for_provider(
        std::slice::from_ref(&message),
        PromptCacheProviderWireFormat::AnthropicMessages,
        &[],
    );
    assert!(chat > 0);
    assert!(responses > 0);
    assert!(anthropic > 0);
    assert_ne!(chat, responses);
}

#[test]
fn test_estimated_history_tokens_includes_structured_provider_fields() {
    let structured = AgentModelMessage::text("assistant", "body")
        .with_openai_reasoning_content("reason")
        .with_tool_calls(vec![AgentModelToolCallMetadata {
            id: "call-1".to_owned(),
            name: "tool".to_owned(),
            input: "{}".into(),
        }])
        .with_anthropic_thinking_blocks(vec![
            AnthropicThinkingBlockMetadata {
                block_type: "thinking".to_owned(),
                thinking: Some("thought".to_owned()),
                data: None,
                signature: Some("signature".to_owned()),
            },
            AnthropicThinkingBlockMetadata {
                block_type: "redacted_thinking".to_owned(),
                thinking: None,
                data: Some("opaque".to_owned()),
                signature: Some("seal".to_owned()),
            },
        ])
        .with_local_images(vec![AgentModelLocalImageRef {
            path: "image.png".to_owned(),
            mime_type: "image/png".to_owned(),
            byte_len: None,
            sha256: None,
            archive_ref: None,
            source_ref: None,
        }]);
    let tool_result = AgentModelMessage::tool_result("call-1", "output", true);

    assert_eq!(estimated_history_tokens(&[structured, tool_result]), 55);
}

#[test]
fn test_should_compress() {
    let runtime = WindowRolloverPolicyRuntime::new(policy(100));
    let system = AgentModelMessage::context("system", "x".repeat(4_000));

    // 前缀之外不足 2 条可压缩消息：不触发。
    let only_prefix = vec![system.clone(), message_with_chars("user", 4_000)];
    assert!(!runtime.should_compress(&only_prefix, Some(100)));

    // 达到 trigger 后先进入 fallback 缓冲，直到完整窗口边界才强制轮转。
    let compressible = vec![
        system,
        message_with_chars("user", 200),
        message_with_chars("assistant", 200),
    ];
    assert!(!runtime.should_compress(&compressible, Some(100)));
    assert!(!runtime.should_compress(&compressible, Some(199)));
    assert!(runtime.should_compress(&compressible, Some(200)));

    // 未达到阈值：不触发。
    let below = vec![
        message_with_chars("system", 10),
        message_with_chars("user", 10),
        message_with_chars("assistant", 10),
    ];
    assert!(!runtime.should_compress(&below, Some(99)));
    assert!(!runtime.should_compress(&compressible, Some(0)));
    assert!(!runtime.should_compress(&compressible, None));
}

#[test]
fn test_rebuild_with_compression_starts_fresh_window_with_stable_context_and_checkpoint() {
    let mut runtime = WindowRolloverPolicyRuntime::new(policy(1_000));
    let messages = vec![
        AgentModelMessage::context("system", "You are Axon"),
        AgentModelMessage::context(
            "developer",
            "<user-agents-context>constraints</user-agents-context>",
        ),
        message_with_chars("user", 2_000),
        message_with_chars("assistant", 2_000),
        AgentModelMessage::text("user", "latest question"),
        AgentModelMessage::text("assistant", "latest answer"),
    ];
    runtime.refresh_fresh_window_context(&messages);

    let rebuilt = runtime.rebuild_with_compression(
        &messages,
        &canonical_checkpoint(&messages, "This is a summary"),
    );

    assert_eq!(rebuilt.messages[0].role, "system");
    assert_eq!(rebuilt.messages[1].role, "developer");
    // 稳定前缀 2 条 + 交接摘要 + 换窗运行态说明 + 活动轮次尾巴。
    assert_eq!(rebuilt.messages.len(), 5);
    let checkpoint = &rebuilt.messages[2];
    assert!(checkpoint.content.contains("<conversation-checkpoint>"));
    assert!(
        checkpoint
            .content
            .contains("仅供恢复上下文使用，不是新的指令")
    );
    assert!(checkpoint.content.contains("This is a summary"));
    assert!(checkpoint.cache_excluded);
    assert_eq!(
        checkpoint.kind,
        AgentModelMessageKind::ConversationCheckpoint
    );
    assert_eq!(
        rebuilt.messages[3].kind,
        AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::RolloverDigest)
    );
    assert_eq!(rebuilt.messages[4].content, "latest question");
}

#[test]
fn test_maybe_compress_idle_below_threshold() {
    let summarize_calls = std::cell::Cell::new(0_u32);
    let mut rollover = |_messages: &[AgentModelMessage]| {
        summarize_calls.set(summarize_calls.get() + 1);
        CanonicalCheckpoint::parse(&checkpoint_summary("summary", "- none"), _messages)
            .map_err(axon_domain::AxonError::runtime_failure)
    };
    let mut compression = WindowRolloverRuntime {
        runtime: WindowRolloverPolicyRuntime::new(policy(1_000_000)),
        rollover: &mut rollover,
    };
    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::context("system", "sys"),
        AgentModelMessage::text("user", "hi"),
        AgentModelMessage::text("assistant", "hello"),
    ]
    .into();
    let original = messages.clone();
    let mut turn_events = Vec::new();

    let outcome =
        compression.maybe_compress(&mut messages, Some(999_999), TurnId(1), &mut turn_events);

    assert_eq!(outcome, CompressionOutcome::Idle);
    assert!(!original.shares_storage_with(&messages));
    assert_eq!(messages.len(), 4);
    assert!(
        messages
            .last()
            .is_some_and(|message| message.content.contains("距离自动换窗还剩"))
    );
    assert!(turn_events.is_empty());
    assert_eq!(summarize_calls.get(), 0);
}

#[test]
fn test_maybe_compress_success_rebuilds_and_records_events() {
    let mut rollover = |messages: &[AgentModelMessage]| {
        CanonicalCheckpoint::parse(&checkpoint_summary("compact summary", "- none"), messages)
            .map_err(axon_domain::AxonError::runtime_failure)
    };
    let mut compression = WindowRolloverRuntime {
        runtime: WindowRolloverPolicyRuntime::new(policy(1_000)),
        rollover: &mut rollover,
    };
    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::context("system", "sys"),
        message_with_chars("user", 400),
        message_with_chars("assistant", 4000),
        AgentModelMessage::text("user", "latest"),
    ]
    .into();
    let original = messages.clone();
    let mut turn_events = Vec::new();

    let outcome = compression.maybe_compress_requested(
        &mut messages,
        Some(1_000),
        TurnId(7),
        &mut turn_events,
    );

    assert_eq!(outcome, CompressionOutcome::Compressed);
    assert!(!original.shares_storage_with(&messages));
    assert_eq!(messages[0].role, "system");
    assert!(
        messages.iter().any(|message| {
            message.content.contains("当前窗口交接事实：")
                && message.content.contains("compact summary")
        }),
        "压缩成功后新窗口必须包含交接 checkpoint"
    );
    assert_eq!(turn_events.len(), 2);
    assert_eq!(turn_events[0].status.as_deref(), Some("running"));
    assert_eq!(turn_events[1].status.as_deref(), Some("completed"));
    assert_eq!(turn_events[0].phase.as_deref(), Some("history"));
    // 换窗前后读数已下线：起始与完成摘要都不得出现任何 token 数字。
    assert!(
        !turn_events[0].summary.contains("tokens"),
        "起始摘要不得带换窗前读数：{}",
        turn_events[0].summary
    );
    assert!(
        !turn_events[1].summary.contains("tokens"),
        "完成摘要不得带换窗读数：{}",
        turn_events[1].summary
    );
    let metadata = turn_events[1].metadata.as_ref().expect("压缩指标 metadata");
    // 换窗读数已整体下线：metadata 只留消息数与 checkpoint 体量。
    assert!(metadata.get("beforeTokens").is_none());
    assert!(metadata.get("afterTokens").is_none());
    assert_eq!(metadata["beforeMessages"], 4);
    assert_eq!(metadata["afterMessages"], messages.len());
    assert!(
        metadata["checkpointBytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );
}

#[test]
fn test_maybe_compress_retries_after_failure() {
    let attempts = std::cell::Cell::new(0_u8);
    let mut rollover = |messages: &[AgentModelMessage]| {
        let attempt = attempts.get();
        attempts.set(attempt + 1);
        if attempt == 0 {
            Err(axon_domain::AxonError::runtime_failure(
                "provider 暂时不可用",
            ))
        } else {
            CanonicalCheckpoint::parse(&checkpoint_summary("recovered summary", "- none"), messages)
                .map_err(axon_domain::AxonError::runtime_failure)
        }
    };
    let mut compression = WindowRolloverRuntime {
        runtime: WindowRolloverPolicyRuntime::new(policy(4_000)),
        rollover: &mut rollover,
    };
    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::context("system", "sys"),
        message_with_chars("user", 400),
        message_with_chars("assistant", 10000),
        AgentModelMessage::text("user", "继续任务"),
    ]
    .into();
    messages[2].retention_class = crate::RetentionClass::P3Derivable;
    let mut turn_events = Vec::new();

    assert_eq!(
        compression.maybe_compress_requested(
            &mut messages,
            Some(4_000),
            TurnId(2),
            &mut turn_events,
        ),
        CompressionOutcome::Prompted
    );
    assert_eq!(
        compression.maybe_compress_requested(
            &mut messages,
            Some(4_000),
            TurnId(3),
            &mut turn_events,
        ),
        CompressionOutcome::Compressed
    );
    assert_eq!(attempts.get(), 2);

    messages.push(AgentModelMessage::text(
        "assistant",
        "provider 产生了新历史",
    ));
    assert_eq!(
        compression.maybe_compress_requested(
            &mut messages,
            Some(4_000),
            TurnId(4),
            &mut turn_events,
        ),
        CompressionOutcome::Compressed
    );

    assert_eq!(attempts.get(), 3);
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("recovered summary"))
    );
}

#[test]
fn failed_compression_retries_when_provider_visible_capabilities_change() {
    let attempts = std::cell::Cell::new(0_u8);
    let mut rollover = |_messages: &[AgentModelMessage]| {
        attempts.set(attempts.get().saturating_add(1));
        Err(axon_domain::AxonError::runtime_failure(
            "provider 暂时不可用",
        ))
    };
    let mut compression = WindowRolloverRuntime {
        runtime: WindowRolloverPolicyRuntime::new(policy(4_000)),
        rollover: &mut rollover,
    };
    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::context("system", "sys"),
        message_with_chars("user", 400),
        message_with_chars("assistant", 10_000),
    ]
    .into();
    let mut turn_events = Vec::new();

    assert_eq!(
        compression.maybe_compress_requested(
            &mut messages,
            Some(4_000),
            TurnId(2),
            &mut turn_events,
        ),
        CompressionOutcome::Prompted
    );
    compression.runtime.configure_provider_wire(
        PromptCacheProviderWireFormat::AnthropicMessages,
        &[],
        false,
    );
    assert_eq!(
        compression.maybe_compress_requested(
            &mut messages,
            Some(4_000),
            TurnId(3),
            &mut turn_events,
        ),
        CompressionOutcome::Prompted
    );
    assert_eq!(attempts.get(), 2);
}

#[test]
fn test_maybe_compress_rejects_checkpoint_over_window_output_budget() {
    // 需越过 token 上限：18,000 字节约为 4500 tokens，超过 4096 且仍低于 32 KiB 字节阀。
    let oversized_task = "摘要".repeat(3_000);
    let mut rollover = |messages: &[AgentModelMessage]| {
        CanonicalCheckpoint::parse(&checkpoint_summary(&oversized_task, "- none"), messages)
            .map_err(axon_domain::AxonError::runtime_failure)
    };
    let mut compression = WindowRolloverRuntime {
        runtime: WindowRolloverPolicyRuntime::new(policy(1_000)),
        rollover: &mut rollover,
    };
    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::context("system", "sys"),
        AgentModelMessage::text("user", "task"),
        AgentModelMessage::text("assistant", "answer"),
    ]
    .into();
    let original = messages.clone();
    let mut turn_events = Vec::new();

    let outcome = compression.maybe_compress_requested(
        &mut messages,
        Some(1_000),
        TurnId(9),
        &mut turn_events,
    );

    assert_eq!(outcome, CompressionOutcome::Prompted);
    assert!(messages.len() > original.len());
    assert!(&messages.as_slice()[..original.len()] == original.as_slice());
    assert_eq!(
        turn_events[1]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("reason"))
            .and_then(serde_json::Value::as_str),
        Some("checkpoint_over_budget")
    );
}

#[test]
fn requested_rollover_defers_until_state_saved() {
    struct UnsavedMemoryHandoff;

    impl WindowRolloverTransaction for UnsavedMemoryHandoff {
        fn validate_memory_handoff(
            &mut self,
            _validation_mode: WindowRolloverValidationMode,
        ) -> axon_domain::AxonResult<()> {
            Err(axon_domain::AxonError::runtime_failure(
                "当前窗口尚未成功保存",
            ))
        }

        fn prepare(
            &mut self,
            _messages: &[AgentModelMessage],
            _turn_id: TurnId,
        ) -> axon_domain::AxonResult<CanonicalCheckpoint> {
            panic!("未通过记忆交接门禁时不得生成 checkpoint")
        }

        fn commit(
            &mut self,
            _messages: &[AgentModelMessage],
            _checkpoint: &CanonicalCheckpoint,
            _replacement: &CompactedHistory,
        ) -> axon_domain::AxonResult<()> {
            panic!("未通过记忆交接门禁时不得提交")
        }
    }

    let mut rollover = UnsavedMemoryHandoff;
    let mut compression = WindowRolloverRuntime::new(
        WindowRolloverPolicyRuntime::new(policy(1_000)),
        &mut rollover,
    );
    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::context("system", "sys"),
        message_with_chars("user", 400),
        message_with_chars("assistant", 4_000),
    ]
    .into();
    let original = messages.clone();
    let mut turn_events = Vec::new();

    let outcome = compression.maybe_compress_requested(
        &mut messages,
        Some(1_000),
        TurnId(10),
        &mut turn_events,
    );

    assert_eq!(outcome, CompressionOutcome::Prompted);
    assert_ne!(messages, original);
    assert_eq!(&messages[..original.len()], &original[..]);
    assert!(messages.iter().any(|message| {
        message.content.contains("当前任务窗口已耗尽")
            && message.content.contains("builtin.notes_write_file")
    }));
    assert!(
        turn_events.is_empty(),
        "便签门禁拒绝是可自愈的推迟，不发布用户可见的失败事实"
    );
}

#[test]
fn commit_failure_keeps_the_current_history_for_the_next_turn() {
    struct FailingCommit;

    impl WindowRolloverTransaction for FailingCommit {
        fn prepare(
            &mut self,
            messages: &[AgentModelMessage],
            _turn_id: TurnId,
        ) -> axon_domain::AxonResult<CanonicalCheckpoint> {
            Ok(canonical_checkpoint(messages, "继续"))
        }
        fn commit(
            &mut self,
            _messages: &[AgentModelMessage],
            _checkpoint: &CanonicalCheckpoint,
            _replacement: &CompactedHistory,
        ) -> axon_domain::AxonResult<()> {
            Err(axon_domain::AxonError::runtime_failure(
                "单条 JSONL 记录超过 shard 大小上限",
            ))
        }
    }

    let mut rollover = FailingCommit;
    let mut compression = WindowRolloverRuntime::new(
        WindowRolloverPolicyRuntime::new(policy(1_000)),
        &mut rollover,
    );
    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::context("system", "sys"),
        message_with_chars("user", 400),
        message_with_chars("assistant", 4_000),
    ]
    .into();
    let original = messages.clone();
    let mut turn_events = Vec::new();

    let outcome = compression.maybe_compress_requested(
        &mut messages,
        Some(1_000),
        TurnId(11),
        &mut turn_events,
    );

    assert_eq!(outcome, CompressionOutcome::Prompted);
    // 归档失败时窗口身份没有推进，历史必须原样留着：只在末尾多一条修复提示。
    // 若这里换成新窗口，界面读数会与持久事实分叉，下一轮又把整段旧历史装回来。
    assert_eq!(&messages[..original.len()], &original[..]);
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("当前任务窗口已耗尽"))
    );
    let failed = turn_events
        .iter()
        .find(|event| event.summary.contains("窗口归档提交失败"))
        .expect("必须留下归档失败的轮次事件");
    assert_eq!(
        failed
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("reason"))
            .and_then(serde_json::Value::as_str),
        Some("commit_failed")
    );
}

mod insert_then_compress_projection_tests;

/// 换窗时稳定前缀按内容去重：每开一个新窗口都会重新注入一组，不去重就会逐窗累积。
///
/// 反向判据：摘掉 `dedup_stable_messages`，这条用例立刻失败（真机窗口 10 实测 84 条里
/// 72 条是 9 组重复，单这一项约 114K tokens）。
#[test]
fn fresh_window_collapses_the_repeated_stable_prefix() {
    let checkpoint = crate::CanonicalCheckpoint::parse(&checkpoint_summary("任务", "- none"), &[])
        .expect("摘要应通过完整性门禁");
    let stable = AgentModelMessage::context(
        "developer",
        "<user-agents-context>全局规则</user-agents-context>",
    );
    let messages = vec![stable.clone(), stable.clone(), stable];

    let compacted =
        crate::insert_then_compress::rebuild_with_digest_for_test(&messages, &checkpoint, None);

    let kept = compacted
        .messages
        .iter()
        .filter(|message| message.content.contains("<user-agents-context>"))
        .count();
    assert_eq!(kept, 1, "同一份稳定前缀只能留一份，否则窗口越换越大");
}

#[test]
fn fresh_window_injects_the_rollover_digest_note() {
    let checkpoint = crate::CanonicalCheckpoint::parse(&checkpoint_summary("任务", "- none"), &[])
        .expect("摘要应通过完整性门禁");
    let digest = "- builtin.edit（completed）：改 crates/a.rs\n- builtin.shell_command（completed）：cargo test -p a";

    let compacted =
        crate::insert_then_compress::rebuild_with_digest_for_test(&[], &checkpoint, Some(digest));

    let note = compacted
        .messages
        .iter()
        .find(|message| {
            message.kind == AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::RolloverDigest)
        })
        .expect("新窗口必须带一条换窗回执 note");
    assert!(
        note.content.contains("builtin.edit"),
        "回执要带具体动作：{}",
        note.content
    );
    assert!(
        note.content.contains("history_list_windows"),
        "回执要指出回读旧窗口的工具：{}",
        note.content
    );
}

/// 没有动作可写时，路线说明照常下发。
///
/// 回归背景：最后几步恰好都是便签写入与换窗请求时动作段为空，旧实现整条 note 都不发，
/// 新窗口于是既没有动作、也没有回读路线，醒来只能靠猜。
#[test]
fn fresh_window_keeps_the_route_note_when_no_actions_were_recorded() {
    let checkpoint = crate::CanonicalCheckpoint::parse(&checkpoint_summary("任务", "- none"), &[])
        .expect("摘要应通过完整性门禁");

    let compacted =
        crate::insert_then_compress::rebuild_with_digest_for_test(&[], &checkpoint, None);
    let note = compacted
        .messages
        .iter()
        .find(|message| {
            message.kind == AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::RolloverDigest)
        })
        .expect("没有动作也要给一条路线说明");
    assert!(
        note.content.contains("builtin.history_list_windows"),
        "{}",
        note.content
    );
    assert!(
        note.content.contains("builtin.notes_read_file"),
        "{}",
        note.content
    );
    assert!(
        note.content.contains("不必再整份重读"),
        "摘要已是便签原文时要说明可以跳过重读：{}",
        note.content
    );
    assert!(
        !note.content.contains("换窗前最近的动作"),
        "没有动作时不留空段：{}",
        note.content
    );
    // 分隔必须是真换行：曾经把这两处写成字面量 \n，新窗口里读到的是一串反斜杠。
    assert!(!note.content.contains("\\n"), "{}", note.content);
}
