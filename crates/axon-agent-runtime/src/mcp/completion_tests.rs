use std::collections::BTreeMap;

use crate::{
    McpCompletionReference, McpCompletionRequest, McpCompletionRuntime, McpServerCommand,
    McpServerConfig,
};

#[test]
fn completion_complete_projects_stdio_values_without_raw_payload() {
    if !pwsh_available() {
        return;
    }
    let script_path = write_completion_fixture_script();
    let projection = McpCompletionRuntime::complete(McpCompletionRequest {
        config: McpServerConfig::Stdio(fixture_command(&script_path)),
        reference: McpCompletionReference::Prompt {
            name: "code_review".to_owned(),
        },
        argument_name: "target".to_owned(),
        value: "src/".to_owned(),
        context_arguments: BTreeMap::from([("language".to_owned(), "rust".to_owned())]),
    })
    .expect("completion/complete 应返回安全投影");

    assert_eq!(projection.completion.values[0].value, "src/lib.rs");
    assert_eq!(projection.completion.total, Some(2));
    assert_eq!(projection.completion.has_more, Some(false));
    assert!(!projection.completion.raw_payload_exposed);
    let _ = std::fs::remove_file(script_path);
}

fn write_completion_fixture_script() -> std::path::PathBuf {
    let script_path = axon_test_support::test_temp_root().join(format!(
        "axon-mcp-completion-fixture-{}.ps1",
        std::process::id()
    ));
    std::fs::write(&script_path, completion_fixture_script())
        .expect("completion fixture 脚本应可写入");
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

fn completion_fixture_script() -> &'static str {
    r#"
$line1 = [Console]::In.ReadLine()
if ($line1 -like '*"initialize"*') {
  Write-Output '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"completions":{}},"serverInfo":{"name":"fixture","version":"1"}}}'
  $line2 = [Console]::In.ReadLine()
  $line3 = [Console]::In.ReadLine()
  if ($line3 -like '*"completion/complete"*' -and $line3 -like '*"type":"ref/prompt"*' -and $line3 -like '*"name":"target"*') {
    Write-Output '{"jsonrpc":"2.0","id":9,"result":{"completion":{"values":["src/lib.rs","src/main.rs"],"total":2,"hasMore":false}}}'
  }
}
"#
}
