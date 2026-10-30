use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::Once;

/// Windows Script Host 控制台宿主：以 JScript 引擎执行 stdio 假服务器脚本。
#[cfg(windows)]
const JSCRIPT_HOST_PROGRAM: &str = "cscript";
#[cfg(windows)]
const JSCRIPT_HOST_NOLOGO_FLAG: &str = "//nologo";
#[cfg(windows)]
const JSCRIPT_HOST_ENGINE_FLAG: &str = "//E:JScript";

#[cfg(windows)]
pub(super) fn handshake_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    jscript_command(server_root, "mcp-test-server.js")
}

#[cfg(windows)]
pub(super) fn runtime_exec_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    script_command(server_root, "mcp-runtime-server.ps1")
}

#[cfg(windows)]
pub(super) fn malformed_resources_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    script_command(server_root, "mcp-malformed-resources-server.ps1")
}

#[cfg(windows)]
pub(super) fn collision_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    script_command(server_root, "mcp-collision-server.ps1")
}

#[cfg(windows)]
fn script_command(server_root: &Path, file_name: &str) -> (Vec<String>, PathBuf) {
    warm_powershell();
    let script_path = server_root.join(file_name);
    (
        vec![
            "powershell".to_owned(),
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-ExecutionPolicy".to_owned(),
            "Bypass".to_owned(),
            "-File".to_owned(),
            script_path.to_string_lossy().to_string(),
        ],
        script_path,
    )
}

/// 先把 Windows PowerShell 5.1 的一次性启动成本付掉。
///
/// 首次启动要建模块分析缓存，再叠加杀毒软件扫描，在 CI 机器上远超 MCP 握手超时（10s）：
/// 假服务器在被测代码的超时窗口内还一句话都没说，用例就会误判成「服务器无响应」。
/// 预热在超时窗口之外完成，之后每次 `-File` 启动都只剩进程冷启动的常规开销。
#[cfg(windows)]
fn warm_powershell() {
    static WARMED: Once = Once::new();
    WARMED.call_once(|| {
        let _ = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
            ])
            .args(["-Command", "$null"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// 以 Windows Script Host 的 JScript 引擎承载极简 stdio 假服务器。
///
/// 握手脚本只需要逐行读写 JSON，PowerShell 的冷启动（约 0.5s）会成为
/// 用例耗时主体；`cscript` 启动快一个数量级，且不改变 stdio 协议形态。
#[cfg(windows)]
fn jscript_command(server_root: &Path, file_name: &str) -> (Vec<String>, PathBuf) {
    let script_path = server_root.join(file_name);
    (
        vec![
            JSCRIPT_HOST_PROGRAM.to_owned(),
            JSCRIPT_HOST_NOLOGO_FLAG.to_owned(),
            JSCRIPT_HOST_ENGINE_FLAG.to_owned(),
            script_path.to_string_lossy().to_string(),
        ],
        script_path,
    )
}

#[cfg(not(windows))]
pub(super) fn handshake_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    script_command(server_root, "mcp-test-server.sh")
}

#[cfg(not(windows))]
pub(super) fn runtime_exec_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    script_command(server_root, "mcp-runtime-server.sh")
}

#[cfg(not(windows))]
pub(super) fn malformed_resources_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    script_command(server_root, "mcp-malformed-resources-server.sh")
}

#[cfg(not(windows))]
pub(super) fn collision_command(server_root: &Path) -> (Vec<String>, PathBuf) {
    script_command(server_root, "mcp-collision-server.sh")
}

#[cfg(not(windows))]
fn script_command(server_root: &Path, file_name: &str) -> (Vec<String>, PathBuf) {
    let script_path = server_root.join(file_name);
    (
        vec!["sh".to_owned(), script_path.to_string_lossy().to_string()],
        script_path,
    )
}

/// 握手假服务器脚本：每页 tools/list 响应都必须等到对应请求到达后再写。
///
/// 若抢在 `tools/list` 请求之前写出带 `nextCursor` 的第一页、随后立刻结束脚本，
/// 客户端的分页续页写入就会与子进程退出抢跑，偶发得到「MCP stdio 请求写入失败」。
#[cfg(windows)]
pub(super) fn handshake_script() -> &'static str {
    r#"
WScript.StdIn.ReadLine();
WScript.StdIn.ReadLine();
WScript.StdOut.WriteLine('{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25"}}');
WScript.StdIn.ReadLine();
WScript.StdOut.WriteLine('{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query-db","description":"query db","inputSchema":{"type":"object","required":["query"]}}],"nextCursor":"tools-page-2"}}');
WScript.StdIn.ReadLine();
WScript.StdOut.WriteLine('{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"explain-db","description":"explain db","inputSchema":{"type":"object","required":["query"]}}]}}');
"#
}

#[cfg(windows)]
pub(super) fn runtime_exec_script() -> &'static str {
    r#"
$line1 = [Console]::In.ReadLine()
if ($line1 -like '*"initialize"*') {
  Write-Output '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false},"resources":{"listChanged":false,"subscribe":false},"prompts":{"listChanged":false},"completions":{}},"serverInfo":{"name":"team-tools-runtime","title":"Team Tools Runtime","version":"1.0.0","websiteUrl":"https://example.test/team-tools"},"instructions":"Use stable Axon MCP tool names."}}'
  $line2 = [Console]::In.ReadLine()
  $line3 = [Console]::In.ReadLine()
  if ($line3 -like '*"tools/list"*' -and $line3 -like '*tools-page-2*') {
    Write-Output '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"explain-db","description":"explain db","inputSchema":{"type":"object","required":["query"]},"annotations":{"readOnlyHint":true}}]}}'
  } elseif ($line3 -like '*"tools/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query-db","description":"query db","inputSchema":{"type":"object","required":["query"]},"annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":true}}],"nextCursor":"tools-page-2"}}'
  } elseif ($line3 -like '*"resources/list"*' -and $line3 -like '*schema-page-2*') {
    Write-Output '{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"axon://team-tools/schema-extra","name":"Team Schema Extra","description":"schema extra","mimeType":"text/plain"}]}}'
  } elseif ($line3 -like '*"resources/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"axon://team-tools/schema","name":"Team Schema","description":"schema","mimeType":"text/plain"}],"nextCursor":"schema-page-2"}}'
  } elseif ($line3 -like '*"resources/read"*') {
    Write-Output '{"jsonrpc":"2.0","id":6,"result":{"contents":[{"uri":"axon://team-tools/schema","mimeType":"text/plain","text":"team schema v1"}]}}'
  } elseif ($line3 -like '*"resources/subscribe"*') {
    Write-Output '{"jsonrpc":"2.0","method":"notifications/resources/updated","params":{"uri":"axon://team-tools/schema","message":"schema updated"}}'
    Write-Output '{"jsonrpc":"2.0","id":10,"result":{}}'
  } elseif ($line3 -like '*"resources/unsubscribe"*') {
    Write-Output '{"jsonrpc":"2.0","id":11,"result":{}}'
  } elseif ($line3 -like '*"prompts/list"*' -and $line3 -like '*prompt-page-2*') {
    Write-Output '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"incident_summary","title":"Incident Summary","description":"incident prompt","arguments":[{"name":"incident","description":"incident id","required":true}]}]}}'
  } elseif ($line3 -like '*"prompts/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"code_review","title":"Code Review","description":"review prompt","arguments":[{"name":"target","description":"target path","required":true}]}],"nextCursor":"prompt-page-2"}}'
  } elseif ($line3 -like '*"prompts/get"*') {
    Write-Output '{"jsonrpc":"2.0","id":8,"result":{"description":"review prompt","messages":[{"role":"user","content":{"type":"text","text":"review src/lib.rs"}}]}}'
  } elseif ($line3 -like '*"completion/complete"*') {
    Write-Output '{"jsonrpc":"2.0","id":9,"result":{"completion":{"values":["src/lib.rs"],"total":1,"hasMore":false}}}'
  }
  if ($line3 -like '*"tools/list"*' -and $line3 -notlike '*tools-page-2*') {
    $line3b = [Console]::In.ReadLine()
    if ($line3b -like '*tools-page-2*') {
      Write-Output '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"explain-db","description":"explain db","inputSchema":{"type":"object","required":["query"]},"annotations":{"readOnlyHint":true}}]}}'
    }
  }
  if ($line3 -like '*"prompts/list"*' -and $line3 -notlike '*prompt-page-2*') {
    $line3b = [Console]::In.ReadLine()
    if ($line3b -like '*prompt-page-2*') {
      Write-Output '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"incident_summary","title":"Incident Summary","description":"incident prompt","arguments":[{"name":"incident","description":"incident id","required":true}]}]}}'
    }
  }
  $line4 = [Console]::In.ReadLine()
  if ($line4 -like '*tools/call*') {
    Write-Output '{"jsonrpc":"2.0","id":100,"result":{"content":[{"type":"text","text":"MCP_RESULT ORBIT-9"}],"structuredContent":{"files":[{"path":"src/lib.rs","operation":"update","diff":"--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n"}]},"isError":false}}'
  } elseif ($line4 -like '*"resources/templates/list"*' -and $line4 -like '*template-page-2*') {
    Write-Output '{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[{"uriTemplate":"axon://team-tools/extra/{name}","name":"Team Extra Template","description":"template extra","mimeType":"text/plain"}]}}'
  } elseif ($line4 -like '*"resources/templates/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[{"uriTemplate":"axon://team-tools/{name}","name":"Team Template","description":"template","mimeType":"text/plain"}],"nextCursor":"template-page-2"}}'
  } elseif ($line4 -like '*"resources/read"*') {
    Write-Output '{"jsonrpc":"2.0","id":6,"result":{"contents":[{"uri":"axon://team-tools/schema","mimeType":"text/plain","text":"team schema v1"}]}}'
  } elseif ($line4 -like '*"resources/subscribe"*') {
    Write-Output '{"jsonrpc":"2.0","method":"notifications/resources/updated","params":{"uri":"axon://team-tools/schema","message":"schema updated"}}'
    Write-Output '{"jsonrpc":"2.0","id":10,"result":{}}'
  } elseif ($line4 -like '*"resources/unsubscribe"*') {
    Write-Output '{"jsonrpc":"2.0","id":11,"result":{}}'
  } elseif ($line4 -like '*"prompts/list"*' -and $line4 -like '*prompt-page-2*') {
    Write-Output '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"incident_summary","title":"Incident Summary","description":"incident prompt","arguments":[{"name":"incident","description":"incident id","required":true}]}]}}'
  } elseif ($line4 -like '*"prompts/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"code_review","title":"Code Review","description":"review prompt","arguments":[{"name":"target","description":"target path","required":true}]}],"nextCursor":"prompt-page-2"}}'
  } elseif ($line4 -like '*"prompts/get"*') {
    Write-Output '{"jsonrpc":"2.0","id":8,"result":{"description":"review prompt","messages":[{"role":"user","content":{"type":"text","text":"review src/lib.rs"}}]}}'
  } elseif ($line4 -like '*"completion/complete"*') {
    Write-Output '{"jsonrpc":"2.0","id":9,"result":{"completion":{"values":["src/lib.rs"],"total":1,"hasMore":false}}}'
  }
}
"#
}

#[cfg(windows)]
pub(super) fn malformed_resources_script() -> &'static str {
    r#"
$line1 = [Console]::In.ReadLine()
if ($line1 -like '*"initialize"*') {
  Write-Output '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"resources":{"listChanged":false}}}}'
  $line2 = [Console]::In.ReadLine()
  $line3 = [Console]::In.ReadLine()
  if ($line3 -like '*"resources/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":4,"result":{}}'
  }
  $line4 = [Console]::In.ReadLine()
  if ($line4 -like '*"resources/templates/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[]}}'
  }
}
"#
}

#[cfg(windows)]
pub(super) fn collision_script() -> &'static str {
    r#"
$line1 = [Console]::In.ReadLine()
if ($line1 -like '*"initialize"*') {
  Write-Output '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25"}}'
  $line2 = [Console]::In.ReadLine()
  $line3 = [Console]::In.ReadLine()
  if ($line3 -like '*"tools/list"*') {
    Write-Output '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query-db","description":"query db","inputSchema":{"type":"object","required":["query"]}},{"name":"query_db","description":"query underscore","inputSchema":{"type":"object","required":["query"]}}]}}'
  }
}
"#
}

/// 握手假服务器脚本：与 Windows 分支保持同一分页时序，先收到请求再回应。
#[cfg(not(windows))]
pub(super) fn handshake_script() -> &'static str {
    r#"
read _
read _
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25"}}'
read _
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query-db","description":"query db","inputSchema":{"type":"object","required":["query"]}}],"nextCursor":"tools-page-2"}}'
read _
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"explain-db","description":"explain db","inputSchema":{"type":"object","required":["query"]}}]}}'
AXON_MCP_TEST_SECRET=hidden
"#
}

#[cfg(not(windows))]
pub(super) fn runtime_exec_script() -> &'static str {
    r#"
read line1
case "$line1" in
  *'"initialize"'*)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false},"resources":{"listChanged":false,"subscribe":false},"prompts":{"listChanged":false},"completions":{}},"serverInfo":{"name":"team-tools-runtime","title":"Team Tools Runtime","version":"1.0.0","websiteUrl":"https://example.test/team-tools"},"instructions":"Use stable Axon MCP tool names."}}'
    read line2
    read line3
    case "$line3" in
      *'"tools/list"'*tools-page-2*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"explain-db","description":"explain db","inputSchema":{"type":"object","required":["query"]},"annotations":{"readOnlyHint":true}}]}}' ;;
      *'"tools/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query-db","description":"query db","inputSchema":{"type":"object","required":["query"]},"annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":true}}],"nextCursor":"tools-page-2"}}' ;;
      *'"resources/list"'*schema-page-2*) printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"axon://team-tools/schema-extra","name":"Team Schema Extra","description":"schema extra","mimeType":"text/plain"}]}}' ;;
      *'"resources/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"axon://team-tools/schema","name":"Team Schema","description":"schema","mimeType":"text/plain"}],"nextCursor":"schema-page-2"}}' ;;
      *'"resources/read"'*) printf '%s\n' '{"jsonrpc":"2.0","id":6,"result":{"contents":[{"uri":"axon://team-tools/schema","mimeType":"text/plain","text":"team schema v1"}]}}' ;;
      *'"resources/subscribe"'*) printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/resources/updated","params":{"uri":"axon://team-tools/schema","message":"schema updated"}}' '{"jsonrpc":"2.0","id":10,"result":{}}' ;;
      *'"resources/unsubscribe"'*) printf '%s\n' '{"jsonrpc":"2.0","id":11,"result":{}}' ;;
      *'"prompts/list"'*prompt-page-2*) printf '%s\n' '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"incident_summary","title":"Incident Summary","description":"incident prompt","arguments":[{"name":"incident","description":"incident id","required":true}]}]}}' ;;
      *'"prompts/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"code_review","title":"Code Review","description":"review prompt","arguments":[{"name":"target","description":"target path","required":true}]}],"nextCursor":"prompt-page-2"}}' ;;
      *'"prompts/get"'*) printf '%s\n' '{"jsonrpc":"2.0","id":8,"result":{"description":"review prompt","messages":[{"role":"user","content":{"type":"text","text":"review src/lib.rs"}}]}}' ;;
      *'"completion/complete"'*) printf '%s\n' '{"jsonrpc":"2.0","id":9,"result":{"completion":{"values":["src/lib.rs"],"total":1,"hasMore":false}}}' ;;
    esac
    case "$line3" in
      *'"tools/list"'*tools-page-2*) ;;
      *'"tools/list"'*)
        read line3b
        case "$line3b" in
          *tools-page-2*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"explain-db","description":"explain db","inputSchema":{"type":"object","required":["query"]},"annotations":{"readOnlyHint":true}}]}}' ;;
        esac
        ;;
    esac
    case "$line3" in
      *'"prompts/list"'*prompt-page-2*) ;;
      *'"prompts/list"'*)
        read line3c
        case "$line3c" in
          *prompt-page-2*) printf '%s\n' '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"incident_summary","title":"Incident Summary","description":"incident prompt","arguments":[{"name":"incident","description":"incident id","required":true}]}]}}' ;;
        esac
        ;;
    esac
    read line4
    case "$line4" in
      *tools/call*) printf '%s\n' '{"jsonrpc":"2.0","id":100,"result":{"content":[{"type":"text","text":"MCP_RESULT ORBIT-9"}],"structuredContent":{"files":[{"path":"src/lib.rs","operation":"update","diff":"--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n"}]},"isError":false}}' ;;
      *'"resources/templates/list"'*template-page-2*) printf '%s\n' '{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[{"uriTemplate":"axon://team-tools/extra/{name}","name":"Team Extra Template","description":"template extra","mimeType":"text/plain"}]}}' ;;
      *'"resources/templates/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[{"uriTemplate":"axon://team-tools/{name}","name":"Team Template","description":"template","mimeType":"text/plain"}],"nextCursor":"template-page-2"}}' ;;
      *'"resources/read"'*) printf '%s\n' '{"jsonrpc":"2.0","id":6,"result":{"contents":[{"uri":"axon://team-tools/schema","mimeType":"text/plain","text":"team schema v1"}]}}' ;;
      *'"resources/subscribe"'*) printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/resources/updated","params":{"uri":"axon://team-tools/schema","message":"schema updated"}}' '{"jsonrpc":"2.0","id":10,"result":{}}' ;;
      *'"resources/unsubscribe"'*) printf '%s\n' '{"jsonrpc":"2.0","id":11,"result":{}}' ;;
      *'"prompts/list"'*prompt-page-2*) printf '%s\n' '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"incident_summary","title":"Incident Summary","description":"incident prompt","arguments":[{"name":"incident","description":"incident id","required":true}]}]}}' ;;
      *'"prompts/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"code_review","title":"Code Review","description":"review prompt","arguments":[{"name":"target","description":"target path","required":true}]}],"nextCursor":"prompt-page-2"}}' ;;
      *'"prompts/get"'*) printf '%s\n' '{"jsonrpc":"2.0","id":8,"result":{"description":"review prompt","messages":[{"role":"user","content":{"type":"text","text":"review src/lib.rs"}}]}}' ;;
      *'"completion/complete"'*) printf '%s\n' '{"jsonrpc":"2.0","id":9,"result":{"completion":{"values":["src/lib.rs"],"total":1,"hasMore":false}}}' ;;
    esac
    ;;
esac
"#
}

#[cfg(not(windows))]
pub(super) fn malformed_resources_script() -> &'static str {
    r#"
read line1
case "$line1" in
  *'"initialize"'*)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"resources":{"listChanged":false}}}}'
    read line2
    read line3
    case "$line3" in
      *'"resources/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{}}' ;;
    esac
    read line4
    case "$line4" in
      *'"resources/templates/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[]}}' ;;
    esac
    ;;
esac
"#
}

#[cfg(not(windows))]
pub(super) fn collision_script() -> &'static str {
    r#"
read line1
case "$line1" in
  *'"initialize"'*)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25"}}'
    read line2
    read line3
    case "$line3" in
      *'"tools/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query-db","description":"query db","inputSchema":{"type":"object","required":["query"]}},{"name":"query_db","description":"query underscore","inputSchema":{"type":"object","required":["query"]}}]}}' ;;
    esac
    ;;
esac
"#
}
