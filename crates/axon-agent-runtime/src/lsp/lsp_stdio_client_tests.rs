use super::*;

#[cfg(windows)]
const TEST_NOTIFICATION_COUNT: usize = 5;
#[cfg(windows)]
const TEST_NOTIFICATION_WAIT: Duration = Duration::from_secs(2);
#[cfg(windows)]
const TEST_NOTIFICATION_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// 写后诊断的有界重试窗口。
///
/// 断言的是「最终能看到 didSave 产生的诊断」，不是「一次查询就能拿到」：假 server 对
/// didChange 与 didSave 各推一条，两条的到达间隔不是协议保证。
#[cfg(windows)]
const TEST_SAVE_DIAGNOSTIC_WAIT: Duration = Duration::from_secs(10);

#[test]
fn read_content_length_parses_header() {
    let bytes = b"Content-Length: 17\r\n\r\n";
    let mut reader = BufReader::new(&bytes[..]);
    let length = read_content_length(&mut reader).unwrap();
    assert_eq!(length, 17);
}

#[test]
fn lsp_request_serializes_jsonrpc_request() {
    let request = LspClientRequest {
        id: 9,
        method: "workspace/symbol".to_owned(),
        params: json!({ "query": "demo" }),
    };
    let payload = serde_json::to_value(json!({
        "jsonrpc": LSP_JSONRPC_VERSION,
        "id": request.id,
        "method": request.method,
        "params": request.params,
    }))
    .unwrap();
    assert_eq!(payload["id"], 9);
    assert_eq!(payload["method"], "workspace/symbol");
    assert_eq!(payload["params"]["query"], "demo");
}

#[test]
fn persistent_reader_preserves_back_to_back_messages() {
    let first = r#"{"jsonrpc":"2.0","method":"window/logMessage"}"#;
    let second = r#"{"jsonrpc":"2.0","id":7,"result":[]}"#;
    let wire = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        first.len(),
        first,
        second.len(),
        second
    );
    let mut reader = BufReader::new(std::io::Cursor::new(wire.into_bytes()));

    let notification = read_message_from(&mut reader).unwrap();
    let response = read_message_from(&mut reader).unwrap();

    assert_eq!(notification["method"], "window/logMessage");
    assert_eq!(response["id"], 7);
}

#[cfg(windows)]
#[test]
fn diagnostics_document_key_normalizes_windows_uri_variants() {
    let workspace = axon_test_support::test_temp_root()
        .join(format!("axon-lsp-uri-key-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    let document = workspace.join("main.ts");
    std::fs::write(&document, "export const value = 1;\n").unwrap();
    let uri = crate::lsp_document::path_to_file_uri(&document).unwrap();
    let drive = uri.chars().nth("file:///".len()).unwrap();
    let prefix = format!("file:///{drive}:/");
    let server_uri = format!(
        "file:///{}%3A/{}",
        drive.to_ascii_lowercase(),
        uri.strip_prefix(&prefix).unwrap()
    );

    assert_eq!(
        canonical_file_path_from_uri(&uri).unwrap(),
        canonical_file_path_from_uri(&server_uri).unwrap()
    );

    let _ = std::fs::remove_dir_all(workspace);
}

#[cfg(windows)]
#[test]
fn document_sync_uses_open_then_change_and_falls_back_to_push_diagnostics() {
    let workspace = axon_test_support::test_temp_root()
        .join(format!("axon-lsp-document-sync-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    let document = workspace.join("main.ts");
    let script = workspace.join("fake-lsp.ps1");
    let log = workspace.join("notifications.jsonl");
    std::fs::write(&document, "export const value = 1;\n").unwrap();
    std::fs::write(
        &script,
        r#"
param([string]$LogPath)

function Send-LspMessage([object]$Message) {
    $response = $Message | ConvertTo-Json -Compress -Depth 20
    $length = [Text.Encoding]::UTF8.GetByteCount($response)
    $separator = [string][char]13 + [char]10 + [char]13 + [char]10
    [Console]::Out.Write("Content-Length: $length" + $separator + $response)
    [Console]::Out.Flush()
}

$reader = [Console]::In
while (($line = $reader.ReadLine()) -ne $null) {
    if ($line -notmatch 'Content-Length:\s*(\d+)') {
        continue
    }
    $length = [int]$Matches[1]
    $null = $reader.ReadLine()
    $buffer = New-Object char[] $length
    $read = $reader.ReadBlock($buffer, 0, $length)
    $request = (-join $buffer[0..($read - 1)]) | ConvertFrom-Json
    if ($request.method -eq 'initialize') {
        Send-LspMessage @{
            jsonrpc = '2.0'
            id = $request.id
            result = @{ capabilities = @{ diagnosticProvider = $false } }
        }
        continue
    }
    if ($request.method -eq 'textDocument/didOpen' -or
        $request.method -eq 'textDocument/didChange') {
        $version = $request.params.textDocument.version
        @{
            method = $request.method
            version = $version
            languageId = $request.params.textDocument.languageId
        } | ConvertTo-Json -Compress | Add-Content -LiteralPath $LogPath -Encoding utf8
        $message = if ($request.method -eq 'textDocument/didOpen') {
            'open-diagnostic'
        } else {
            'change-diagnostic'
        }
        Send-LspMessage @{
            jsonrpc = '2.0'
            method = 'textDocument/publishDiagnostics'
            params = @{
                uri = $request.params.textDocument.uri
                version = $version
                diagnostics = @(@{
                    message = $message
                    severity = 1
                    range = @{
                        start = @{ line = 0; character = 0 }
                        end = @{ line = 0; character = 1 }
                    }
                })
            }
        }
        continue
    }
    if ($request.method -eq 'textDocument/didSave') {
        @{
            method = $request.method
            uri = $request.params.textDocument.uri
        } | ConvertTo-Json -Compress | Add-Content -LiteralPath $LogPath -Encoding utf8
        Send-LspMessage @{
            jsonrpc = '2.0'
            method = 'textDocument/publishDiagnostics'
            params = @{
                uri = $request.params.textDocument.uri
                diagnostics = @(@{
                    message = 'save-diagnostic'
                    severity = 1
                    range = @{
                        start = @{ line = 0; character = 0 }
                        end = @{ line = 0; character = 1 }
                    }
                })
            }
        }
        continue
    }
    if ($request.method -eq 'textDocument/didClose') {
        @{
            method = $request.method
            uri = $request.params.textDocument.uri
        } | ConvertTo-Json -Compress | Add-Content -LiteralPath $LogPath -Encoding utf8
        continue
    }
    if ($request.method -eq 'workspace/didChangeWatchedFiles') {
        @{
            method = $request.method
            uri = $request.params.changes[0].uri
            type = $request.params.changes[0].type
        } | ConvertTo-Json -Compress | Add-Content -LiteralPath $LogPath -Encoding utf8
        continue
    }
    if ($request.method -eq 'textDocument/diagnostic') {
        @{
            method = $request.method
            version = 0
        } | ConvertTo-Json -Compress | Add-Content -LiteralPath $LogPath -Encoding utf8
        Send-LspMessage @{
            jsonrpc = '2.0'
            id = $request.id
            error = @{ code = -32601; message = 'unsupported' }
        }
    }
}
"#,
    )
    .unwrap();
    let plan = LspLaunchPlan {
        server_id: "fake-lsp".to_owned(),
        program: "pwsh".to_owned(),
        args: vec![
            "-NoLogo".to_owned(),
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-File".to_owned(),
            script.display().to_string(),
            log.display().to_string(),
        ],
        cwd: workspace.display().to_string(),
    };
    let uri = crate::lsp_document::path_to_file_uri(&document).unwrap();
    let root_uri = crate::lsp_document::path_to_file_uri(&workspace).unwrap();
    let mut client = LspStdioClient::start(&plan).unwrap();
    client
        .initialize(&LspClientInitializeInput {
            root_uri,
            process_id: std::process::id(),
        })
        .unwrap();

    assert!(client.touch_document(&uri).unwrap());
    let initial = client
        .document_diagnostic(&LspDiagnosticRequest {
            id: 9,
            text_document_uri: uri.clone(),
        })
        .unwrap();
    assert_eq!(initial["result"]["items"][0]["message"], "open-diagnostic");

    std::fs::write(&document, "export const value = 2;\n").unwrap();
    let document_changed = client.sync_and_notify_document_saved(&uri).unwrap();
    assert!(document_changed);
    assert!(!client.touch_document(&uri).unwrap());
    let after_save_request = LspDiagnosticRequest {
        id: 10,
        text_document_uri: uri.clone(),
    };
    // 写后诊断走 push 通道：假 server 对 didChange 与 didSave 各推一条，两条的到达间隔不是
    // 协议保证（满载下 didSave 可能晚于一次 settle 窗口）。单次查询拿到的可能仍是上一条，
    // 因此按「期望诊断出现」做有界重试，而不是把一次结果当结论。
    let settle_deadline = Instant::now() + TEST_SAVE_DIAGNOSTIC_WAIT;
    let mut response = client
        .document_diagnostic_after_save_with_timeout(
            &after_save_request,
            TEST_NOTIFICATION_WAIT,
            document_changed,
        )
        .unwrap();
    while response["result"]["items"][0]["message"] != "save-diagnostic"
        && Instant::now() < settle_deadline
    {
        thread::sleep(TEST_NOTIFICATION_POLL_INTERVAL);
        response = client
            .document_diagnostic_after_save_with_timeout(
                &after_save_request,
                TEST_NOTIFICATION_WAIT,
                document_changed,
            )
            .unwrap();
    }
    client
        .notify_workspace_file_changes(&[LspWorkspaceFileChange {
            uri: uri.clone(),
            kind: LspWorkspaceFileChangeKind::Deleted,
        }])
        .unwrap();

    let notification_deadline = Instant::now() + TEST_NOTIFICATION_WAIT;
    while std::fs::read_to_string(&log)
        .map(|content| content.lines().count())
        .unwrap_or_default()
        < TEST_NOTIFICATION_COUNT
        && Instant::now() < notification_deadline
    {
        thread::sleep(TEST_NOTIFICATION_POLL_INTERVAL);
    }

    assert_eq!(response["result"]["items"][0]["message"], "save-diagnostic");
    let notifications = std::fs::read_to_string(&log).unwrap();
    let records = notifications
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), TEST_NOTIFICATION_COUNT);
    assert_eq!(records[0]["method"], "textDocument/didOpen");
    assert_eq!(records[0]["version"], 1);
    assert_eq!(records[0]["languageId"], "typescript");
    assert_eq!(records[1]["method"], "textDocument/didChange");
    assert_eq!(records[1]["version"], 2);
    assert_eq!(records[2]["method"], "textDocument/didSave");
    assert_eq!(records[2]["uri"], uri);
    assert_eq!(records[3]["method"], "textDocument/didClose");
    assert_eq!(records[3]["uri"], uri);
    assert_eq!(records[4]["method"], "workspace/didChangeWatchedFiles");
    assert_eq!(records[4]["uri"], uri);
    assert_eq!(records[4]["type"], 3);

    client.shutdown();
    let _ = std::fs::remove_dir_all(workspace);
}
