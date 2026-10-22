//! `mcp_persistent` 集成测试：用脚本模拟的 MCP server 验证持久会话。

use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::{McpPersistentRuntime, McpPersistentSession};
use crate::{McpServerCommand, McpStartupProjection};
use axon_domain::{McpServerDefinition, McpServerStartupStatus};

const TEST_TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
const TEST_DELAYED_RESPONSE: std::time::Duration = std::time::Duration::from_millis(100);
const TEST_SHORT_TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(75);

fn mcp_report() -> McpServerDefinition {
    McpServerDefinition::new("persistent-tools").unwrap()
}

fn startup_projection() -> McpStartupProjection {
    McpStartupProjection {
        server_id: "persistent-tools".to_owned(),
        config_ref: "mcp://persistent-tools/.mcp.json".to_owned(),
        status: McpServerStartupStatus::Discovered,
        server_process_started: false,
        remote_trigger_allowed: false,
        audit_summary: "MCP startup 已发现".to_owned(),
        reasons: vec![],
    }
}

/// 跨平台模拟一个持久 MCP server：
/// 逐行读取 JSON-RPC 请求，对 initialize/tools-list/tools-call 分别回应，
/// 进程在 stdin 关闭前持续存活，从而验证「保活 + 多次往返」。
///
/// 脚本写入临时文件后以 `-File`/脚本路径方式调用，避免 argv 内嵌脚本的引号解析问题。
fn mock_server_command() -> McpServerCommand {
    use std::io::Write as _;

    let unique = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    #[cfg(windows)]
    {
        let script = r#"
while ($true) {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { break }
  if ($line.Trim() -eq '') { continue }
  $msg = $line | ConvertFrom-Json
  $id = $msg.id
  $method = $msg.method
  if ($method -eq 'initialize') {
    if ($line -match '"elicitation"|"roots"|"sampling"|"logging"') { exit 41 }
    Write-Output ('{"jsonrpc":"2.0","id":' + $id + ',"result":{"protocolVersion":"x","capabilities":{}}}')
  } elseif ($method -eq 'tools/list') {
    Write-Output ('{"jsonrpc":"2.0","id":' + $id + ',"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object","required":["text"]},"annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false}}]}}')
  } elseif ($method -eq 'tools/call') {
    $arg = $msg.params.arguments.text
    if ($arg -eq 'delayed') { Start-Sleep -Milliseconds __DELAY_MILLIS__ }
    Write-Output ('{"jsonrpc":"2.0","id":' + $id + ',"result":{"content":[{"type":"text","text":"echo ' + $arg + '"}],"structuredContent":{"echo":"' + $arg + '"},"isError":false}}')
  }
}
"#
        .replace(
            "__DELAY_MILLIS__",
            &TEST_DELAYED_RESPONSE.as_millis().to_string(),
        );
        let mut path = axon_test_support::test_temp_root();
        path.push(format!("axon_mock_mcp_{unique}.ps1"));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        McpServerCommand {
            program: "pwsh".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-File".to_owned(),
                path.to_string_lossy().into_owned(),
            ],
            working_dir: None,
            env: BTreeMap::new(),
            tool_timeout_ms: None,
        }
    }
    #[cfg(not(windows))]
    {
        let script = r#"
while IFS= read -r line; do
  [ -z "$line" ] && continue
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '%s' "$line" | grep -Eq '"elicitation"|"roots"|"sampling"|"logging"' && exit 41
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"x","capabilities":{}}}\n' "$id" ;;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object","required":["text"]},"annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false}}]}}\n' "$id" ;;
    *'"tools/call"'*)
      arg=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
      [ "$arg" = "delayed" ] && sleep __DELAY_SECONDS__
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo %s"}],"structuredContent":{"echo":"%s"},"isError":false}}\n' "$id" "$arg" "$arg" ;;
  esac
done
"#
        .replace(
            "__DELAY_SECONDS__",
            &TEST_DELAYED_RESPONSE.as_secs_f64().to_string(),
        );
        let mut path = axon_test_support::test_temp_root();
        path.push(format!("axon_mock_mcp_{unique}.sh"));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        McpServerCommand {
            program: "sh".to_owned(),
            args: vec![path.to_string_lossy().into_owned()],
            working_dir: None,
            env: BTreeMap::new(),
            tool_timeout_ms: None,
        }
    }
}

fn connect() -> McpPersistentSession {
    McpPersistentRuntime::connect(&mcp_report(), &startup_projection(), &mock_server_command())
        .expect("持久会话握手应成功")
}

#[test]
fn rejects_remote_triggered_startup() {
    let mut startup = startup_projection();
    startup.remote_trigger_allowed = true;
    let result = McpPersistentRuntime::connect(&mcp_report(), &startup, &mock_server_command());
    assert!(result.is_err());
}

#[test]
fn handshake_discovers_tools() {
    let session = connect();
    let handshake = session.handshake();
    assert_eq!(handshake.server_id, "persistent-tools");
    assert_eq!(session.tools().len(), 1);
    assert_eq!(session.tools()[0].name, "echo");
    assert_eq!(
        handshake.tools[0].input_schema["required"],
        serde_json::json!(["text"])
    );
    assert_eq!(
        session.tools()[0].input_schema,
        handshake.tools[0].input_schema
    );
    assert_eq!(
        session.tools()[0].annotations,
        crate::McpToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            open_world_hint: Some(false),
        },
        "持久会话路径必须保留 tools/list 的安全注解"
    );
    // 回归：注解若不穿透到 schema，工具会被一律降级为 GovernedWrite + Exclusive，
    // 从而把整批工具拖成串行并抢占整个工作区写锁。
    let schema = crate::McpToolSchema::from_discovered_tool(&handshake.tools[0]);
    assert_eq!(
        schema.annotations.concurrency_policy(),
        axon_domain::ToolConcurrencyPolicy::ParallelSafe
    );
    assert_eq!(
        schema.annotations.permission_policy(),
        axon_domain::ToolPermissionPolicy::ReadOnly
    );
}

#[test]
fn session_stays_alive_for_multiple_calls() {
    let mut session = connect();
    assert!(session.is_alive());

    let first = session
        .call_tool("echo", r#"{"text":"alpha"}"#, None, None)
        .unwrap();
    assert!(first.result_text.contains("alpha"));
    assert_eq!(first.request_id, 100);
    assert_eq!(first.result_projection.content_types, ["text"]);
    assert!(first.result_projection.structured_content_declared);
    assert_eq!(first.result_projection.structured_content_keys, ["echo"]);
    assert!(!first.result_projection.is_error);
    assert!(!first.result_projection.raw_payload_exposed);

    // 关键：同一进程仍存活，可继续第二次调用。
    let second = session
        .call_tool("echo", r#"{"text":"beta"}"#, Some(TEST_TOOL_TIMEOUT), None)
        .unwrap();
    assert!(second.result_text.contains("beta"));
    assert_eq!(second.request_id, 101);

    assert!(session.is_alive());
    session.shutdown();
}

#[test]
fn empty_arguments_default_to_object() {
    let mut session = connect();
    let result = session.call_tool("echo", "", None, None).unwrap();
    // server 对空 text 回显空串，调用本身应成功且复用持久进程。
    assert!(result.result_text.contains("echo"));
    session.shutdown();
}

#[test]
fn active_cancel_token_allows_response_after_poll_interval() {
    let mut session = connect();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_token: super::McpCancelToken = {
        let cancelled = Arc::clone(&cancelled);
        Arc::new(move || cancelled.load(Ordering::SeqCst))
    };

    let started_at = std::time::Instant::now();
    let result = session
        .call_tool(
            "echo",
            r#"{"text":"delayed"}"#,
            Some(TEST_TOOL_TIMEOUT),
            Some(&cancel_token),
        )
        .expect("未取消的 MCP 调用应跨越取消轮询窗口并等待响应");

    assert!(started_at.elapsed() >= TEST_DELAYED_RESPONSE);
    assert!(result.result_text.contains("delayed"));
    assert!(session.is_alive());
    session.shutdown();
}

#[test]
fn active_cancel_token_still_honors_total_timeout() {
    let mut session = connect();
    let cancel_token: super::McpCancelToken = Arc::new(|| false);

    let error = session
        .call_tool(
            "echo",
            r#"{"text":"delayed"}"#,
            Some(TEST_SHORT_TOOL_TIMEOUT),
            Some(&cancel_token),
        )
        .expect_err("未取消的 MCP 调用仍应受总超时约束");

    assert!(format!("{error:?}").contains("MCP persistent 响应超时"));
    assert!(session.is_alive());
    session.shutdown();
}

#[test]
fn cancelled_call_terminates_persistent_process() {
    let mut session = connect();
    let cancelled = Arc::new(AtomicBool::new(true));
    let cancel_token: super::McpCancelToken = {
        let cancelled = Arc::clone(&cancelled);
        Arc::new(move || cancelled.load(Ordering::SeqCst))
    };

    let error = session
        .call_tool("echo", r#"{"text":"cancel"}"#, None, Some(&cancel_token))
        .expect_err("已取消的 MCP 调用不应继续等待");

    assert!(format!("{error:?}").contains("MCP persistent 调用已取消"));
    assert!(!session.is_alive());
}

#[test]
fn diagnostic_stderr_tail_is_bounded_and_redacted() {
    let mut tail = "前缀".repeat(3_000);
    super::truncate_tail(&mut tail, super::MCP_STDERR_TAIL_LIMIT);
    assert!(tail.len() <= super::MCP_STDERR_TAIL_LIMIT);

    let sanitized = super::sanitize_stderr("normal\napi_key=secret-value\nlast");
    assert!(sanitized.contains("normal\\n"));
    assert!(sanitized.contains("[stderr 敏感行已脱敏]"));
    assert!(!sanitized.contains("secret-value"));
}
