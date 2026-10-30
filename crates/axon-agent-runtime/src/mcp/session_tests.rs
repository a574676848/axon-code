use std::collections::BTreeMap;

use serde_json::json;

use crate::{McpNotificationKind, McpServerCommand, McpStdioExchangeRuntime};

#[test]
fn stdio_exchange_projects_notifications_without_changing_response_contract() {
    if !pwsh_available() {
        return;
    }
    let script_path = write_fixture_script();
    let command = fixture_command(&script_path);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "tools/list"
    })
    .to_string();

    let report =
        McpStdioExchangeRuntime::exchange_with_notifications(&command, vec![request.clone()])
            .expect("stdio exchange 应捕获通知摘要");
    let responses = McpStdioExchangeRuntime::exchange(&command, vec![request])
        .expect("exchange 合同只返回业务响应");

    assert_eq!(report.responses.len(), 2);
    assert_eq!(responses.len(), 2);
    assert_eq!(report.notification_summary.progress_count, 1);
    assert_eq!(report.notification_summary.cancelled_count, 0);
    assert_eq!(report.notification_summary.resources_changed_count, 1);
    assert_eq!(report.notification_summary.prompts_changed_count, 1);
    assert_eq!(report.notification_summary.log_message_count, 1);
    assert_eq!(report.notification_summary.denied_request_count, 1);
    assert_eq!(
        report.notification_summary.latest_denied_method.as_deref(),
        Some("sampling/createMessage")
    );
    assert!(!report.notification_summary.raw_payload_exposed);
    assert_eq!(
        report
            .notification_summary
            .latest
            .as_ref()
            .map(|event| event.kind),
        Some(McpNotificationKind::ClientRequestDenied)
    );
    // 按内容定位而不是按下标：响应集合里同时有 initialize 结果与业务结果，二者顺序取决于
    // 服务端输出时机，断言不该依赖它。
    assert!(
        report
            .responses
            .iter()
            .any(|response| response["result"]["denialObserved"]
                .as_bool()
                .unwrap_or(false)),
        "必须有一条响应证明客户端拒绝已被服务端观察到：{:?}",
        report.responses
    );
    let _ = std::fs::remove_file(script_path);
}

fn write_fixture_script() -> std::path::PathBuf {
    let script_path = axon_test_support::test_temp_root().join(format!(
        "axon-mcp-notification-fixture-{}.ps1",
        std::process::id()
    ));
    std::fs::write(
        &script_path,
        r#"
$initialize = [Console]::In.ReadLine()
$initializeJson = $initialize | ConvertFrom-Json
if ($initializeJson.method -ne "initialize") {
    Write-Output '{"jsonrpc":"2.0","id":42,"error":{"code":-32000,"message":"initialize request missing"}}'
    exit 0
}
Write-Output '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}}'
$initialized = [Console]::In.ReadLine()
$initializedJson = $initialized | ConvertFrom-Json
if ($initializedJson.method -ne "notifications/initialized") {
    Write-Output '{"jsonrpc":"2.0","id":42,"error":{"code":-32000,"message":"initialized notification missing"}}'
    exit 0
}
Write-Output '{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"run-1","progress":25.0,"message":"working"}}'
Write-Output '{"jsonrpc":"2.0","method":"notifications/resources/list_changed","params":{"message":"resources changed"}}'
Write-Output '{"jsonrpc":"2.0","method":"notifications/prompts/list_changed","params":{"message":"prompts changed"}}'
Write-Output '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"debug","logger":"fixture"}}'
Write-Output '{"jsonrpc":"2.0","id":"sample-1","method":"sampling/createMessage","params":{"raw":"hidden"}}'
# 拒绝响应与前序业务请求的先后顺序不是协议保证：initialize 之后的通知只有有限的 drain 窗口，
# 满载下窗口会先到期，客户端随即把自己的业务请求写进 stdin。因此必须逐行读到 id 为 sample-1
# 的那一条，而不是假定紧接着的一行就是拒绝响应。
$denialJson = $null
for ($attempt = 0; $attempt -lt 5; $attempt++) {
    $line = [Console]::In.ReadLine()
    if ([string]::IsNullOrWhiteSpace($line)) { break }
    $candidate = $line | ConvertFrom-Json
    if ($candidate.id -eq "sample-1") {
        $denialJson = $candidate
        break
    }
}
if ($null -eq $denialJson -or $denialJson.error.code -ne -32601) {
    Write-Output '{"jsonrpc":"2.0","id":42,"error":{"code":-32000,"message":"denial response missing"}}'
    exit 0
}
Write-Output '{"jsonrpc":"2.0","id":42,"result":{"ok":true,"denialObserved":true}}'
"#,
    )
    .expect("fixture 脚本应可写入");
    script_path
}

fn fixture_command(script_path: &std::path::Path) -> McpServerCommand {
    McpServerCommand {
        program: "pwsh".to_owned(),
        args: vec![
            "-NoProfile".to_owned(),
            "-File".to_owned(),
            script_path.to_string_lossy().to_string(),
        ],
        working_dir: None,
        env: BTreeMap::new(),
        tool_timeout_ms: None,
    }
}

fn pwsh_available() -> bool {
    // 探测结果是本机静态环境属性，按进程缓存，避免每个用例重复启动 pwsh。
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("pwsh")
            .args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion.Major"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|stdout| stdout.trim().parse::<u32>().ok())
            .is_some_and(|major| major >= 7)
    })
}
