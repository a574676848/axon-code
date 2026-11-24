//! 渠道侧源码守卫：前端与 CLI 不得自行领取或执行 Provider 回合。

const CLI_ENTRY_SOURCE: &str = include_str!("../lib.rs");
const TUI_ADAPTER_SOURCE: &str = include_str!("interactive_tui/runtime_adapter.rs");
const DAEMON_SUBMISSION_SOURCE: &str = include_str!("daemon_interaction_submission.rs");
const DAEMON_SUBSCRIPTION_SOURCE: &str = include_str!("daemon_event_subscription_client.rs");
/// 订阅连接恢复策略（握手、重试退避、durable 进度看门狗）所属模块。
const DAEMON_SUBSCRIPTION_RECOVERY_SOURCE: &str = include_str!("daemon_subscription_recovery.rs");
const DAEMON_CURSOR_SOURCE: &str = include_str!("daemon_event_cursor.rs");
const DAEMON_PROTOCOL_SOURCE: &str = include_str!("../../../axon-protocol/src/daemon.rs");

/// 渠道侧不得出现只属于 Daemon 的领取与 Provider 执行入口。
const DAEMON_ONLY_SYMBOLS: [&str; 2] = ["claim_processing", "run_run_with_progress"];

#[test]
fn cli_and_tui_frontends_do_not_claim_or_execute_provider_turns() {
    for source in [CLI_ENTRY_SOURCE, TUI_ADAPTER_SOURCE] {
        assert_only_outside_channel_source(source);
    }
}

#[test]
fn daemon_client_uses_submit_subscription_and_cursor_replay_protocol() {
    assert!(DAEMON_SUBMISSION_SOURCE.contains("DaemonCommand::SubmitInteraction"));
    assert!(DAEMON_SUBSCRIPTION_SOURCE.contains("subscription_ready"));
    assert!(DAEMON_SUBSCRIPTION_RECOVERY_SOURCE.contains("DaemonCommand::SubscribeEvents"));
    assert!(DAEMON_SUBSCRIPTION_RECOVERY_SOURCE.contains("DaemonCommand::GetInteraction"));
    assert!(DAEMON_CURSOR_SOURCE.contains("event.cursor()"));
    for source in [DAEMON_SUBMISSION_SOURCE, DAEMON_CURSOR_SOURCE] {
        assert!(!source.contains("DaemonCommand::GetInteraction"));
    }
    for source in [
        DAEMON_SUBMISSION_SOURCE,
        DAEMON_SUBSCRIPTION_SOURCE,
        DAEMON_SUBSCRIPTION_RECOVERY_SOURCE,
        DAEMON_CURSOR_SOURCE,
    ] {
        assert!(!source.contains("DaemonCommand::Attach"));
    }
}

#[test]
fn daemon_protocol_does_not_expose_fixed_polling_wait_command() {
    assert!(!DAEMON_PROTOCOL_SOURCE.contains("WaitIdle"));
}

fn assert_only_outside_channel_source(source: &str) {
    for symbol in DAEMON_ONLY_SYMBOLS {
        assert!(
            !source.contains(symbol),
            "渠道侧源码不得出现 Daemon 专有符号 {symbol}"
        );
    }
}
