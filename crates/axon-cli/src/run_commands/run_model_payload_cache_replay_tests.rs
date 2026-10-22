//! 跨轮缓存契约：本轮注入上下文必须按原位置回放，Provider 可见消息只追加。
//!
//! 该契约是长会话缓存命中率的地基：一旦下一轮把上一轮的注入块重新推导或挪位，
//! 整段会话历史都会失去前缀缓存。

use super::*;
use axon_config::{AxonConfig, ConfigLoader};
use axon_domain::DisclosurePolicy;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn replayed_injected_context_keeps_provider_messages_append_only() {
    let root = fixture_root("cache-replay");
    let config_path = root.join("runtime").join("config.toml");
    let cwd = root.join("workspace");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    fs::write(cwd.join("AGENTS.md"), "项目规则").unwrap();
    ConfigLoader::save_to_path(&config_path, &AxonConfig::default()).unwrap();
    let startup = RunStartupSnapshot::build(&config_path, &cwd).unwrap();
    let soul = AgentModelMessage::context(
        "system",
        "<assistant-profile-context>\n主代理 Soul\n</assistant-profile-context>",
    );

    let first = build_payload(&startup, &config_path, &cwd, &soul, None, "第一轮问题");
    assert!(
        !first.injected_context_messages.is_empty(),
        "每轮都必须记录注入上下文"
    );

    // 模拟一轮结束后的落库与回放：注入块 + 用户输入 + 助手回答。
    let mut replayed = first.injected_context_messages.clone();
    replayed.push(
        first
            .messages
            .last()
            .cloned()
            .expect("payload 末尾是用户输入"),
    );
    replayed.push(AgentModelMessage::text("assistant", "第一轮回答"));
    let history_json = serde_json::to_string(&replayed).unwrap();
    let second = build_payload(
        &startup,
        &config_path,
        &cwd,
        &soul,
        Some(&history_json),
        "第二轮问题",
    );

    assert!(
        second.messages.len() > first.messages.len(),
        "第二轮必须在本轮消息之后继续追加"
    );
    assert_eq!(
        second.messages[..first.messages.len()],
        first.messages[..],
        "上一轮 Provider 可见消息必须逐字节成为本轮前缀"
    );
    // 本轮注入块只能落在上一轮消息之后，绝不能插进已发出的前缀里。
    let injected_start = second.messages.len() - 1 - second.injected_context_messages.len();
    assert!(injected_start >= first.messages.len());
    assert_eq!(
        second.messages[injected_start..second.messages.len() - 1],
        second.injected_context_messages[..],
        "本轮注入块必须原序出现在用户输入之前"
    );

    let _ = fs::remove_dir_all(root);
}

fn build_payload(
    startup: &RunStartupSnapshot,
    config_path: &Path,
    cwd: &Path,
    soul: &AgentModelMessage,
    history_json: Option<&str>,
    prompt: &str,
) -> AgentModelPayload {
    let skill_context = prepare_cli_run_skill_context(startup, prompt, 128_000);
    let registry = startup.execution_tools_for_session(config_path, "session-cache-replay");
    let registry = registry
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (payload, _) = build_cli_run_model_payload(
        CliRunModelPayloadInput {
            memory_hits: &[],
            soul_context_message: Some(soul),
            include_personal_memory: true,
            memory_disclosure_policy: DisclosurePolicy::PrivateOnly,
            cwd: &cwd.to_string_lossy(),
            workspace_snapshot: None,
            startup,
            stable_prefix: &startup.prompt_context.stable_prefix,
            context_window: 128_000,
            max_subagent_threads: 16,
            max_subagent_active_turns: 16,
            model_history_json: history_json,
            ledger_context: None,
        },
        skill_context,
        &registry,
    )
    .unwrap();
    payload
}

fn fixture_root(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root()
        .join(format!("axon-cli-{label}-{}-{nonce}", std::process::id()))
}
