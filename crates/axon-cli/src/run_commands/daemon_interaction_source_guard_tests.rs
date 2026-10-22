use std::path::{Path, PathBuf};

const CLI_ENTRY_SOURCE: &str = include_str!("../lib.rs");
const TUI_ADAPTER_SOURCE: &str = include_str!("interactive_tui/runtime_adapter.rs");
const DAEMON_SUBMISSION_SOURCE: &str = include_str!("daemon_interaction_submission.rs");
const DAEMON_SUBSCRIPTION_SOURCE: &str = include_str!("daemon_event_subscription_client.rs");
/// 订阅连接恢复策略（握手、重试退避、durable 进度看门狗）所属模块。
const DAEMON_SUBSCRIPTION_RECOVERY_SOURCE: &str = include_str!("daemon_subscription_recovery.rs");
const DAEMON_CURSOR_SOURCE: &str = include_str!("daemon_event_cursor.rs");
const DAEMON_PROTOCOL_SOURCE: &str = include_str!("../../../axon-protocol/src/daemon.rs");
const FEISHU_CONTROL_SOURCE: &str = include_str!("../remote_commands/remote_feishu_control.rs");
const FEISHU_GATEWAY_SOURCE: &str =
    include_str!("../remote_commands/remote_feishu_gateway_event_execution.rs");
/// gateway 拆分出的 Agent 执行投影与入站映射模块：同为渠道侧源码，必须一并受守卫。
const FEISHU_GATEWAY_ACTION_SOURCE: &str =
    include_str!("../remote_commands/remote_feishu_gateway_action.rs");
const FEISHU_GATEWAY_MAPPING_SOURCE: &str =
    include_str!("../remote_commands/remote_feishu_gateway_mapping.rs");
const FEISHU_TRANSPORT_SOURCE_ROOT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../axon-remote-feishu/src");

/// 渠道侧不得出现只属于 Daemon 的领取与 Provider 执行入口。
const DAEMON_ONLY_SYMBOLS: [&str; 2] = ["claim_processing", "run_run_with_progress"];

#[test]
fn cli_and_tui_frontends_do_not_claim_or_execute_provider_turns() {
    for source in [
        CLI_ENTRY_SOURCE,
        TUI_ADAPTER_SOURCE,
        FEISHU_CONTROL_SOURCE,
        FEISHU_GATEWAY_SOURCE,
        FEISHU_GATEWAY_ACTION_SOURCE,
        FEISHU_GATEWAY_MAPPING_SOURCE,
    ] {
        assert_only_outside_channel_source(source);
    }
}

#[test]
fn feishu_transport_crate_does_not_claim_or_execute_provider_turns() {
    let mut sources = Vec::new();
    collect_rust_sources(Path::new(FEISHU_TRANSPORT_SOURCE_ROOT), &mut sources);
    assert!(!sources.is_empty(), "未扫描到 Feishu 传输层源码");

    for path in sources {
        let source = std::fs::read_to_string(&path).expect("读取 Feishu 传输层源码");
        assert_only_outside_channel_source(&source);
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

fn collect_rust_sources(directory: &Path, collected: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("读取源码目录 {} 失败：{error}", directory.display()));
    for entry in entries {
        let path = entry.expect("读取源码条目").path();
        if path.is_dir() {
            collect_rust_sources(&path, collected);
        } else if path.extension().is_some_and(|value| value == "rs") {
            collected.push(path);
        }
    }
}
