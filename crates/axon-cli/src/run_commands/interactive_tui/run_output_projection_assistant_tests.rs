use axon_tui_runtime::{AgentTuiMessagePartRole, AgentTuiRunLine, AgentTuiRunOutput};

use super::{CliRunResult, run_output_renderer::format_tui_run_output};

#[test]
fn format_tui_run_output_removes_machine_summary_lines() {
    let cases = [
        (
            "真实回复\n任务完成：status=success tools=0/0 failed=0 verify=none tokens=5113\n用量 Token total=5113 input=5009 cache_read=0 cache_write=0 output=104 reasoning=0",
            "真实回复",
        ),
        (
            "真实回复\n下一步 检查 provider/network/config 后重试\n建议下一步: 基于已恢复上下文继续执行\n已完成 12\n已修改文件 src/lib.rs\napply 摘要 patch applied\n上次失败点 cargo check",
            "真实回复",
        ),
        (
            "真实回复\n任务完成：status=success tools=1/1 failed=0\n用量 Token total=5113 input=5009 output=104",
            "真实回复",
        ),
        (
            "真实回复\n工具摘要: completed=1 failed=0 latest=shell\n工具统计 tools=1/1 failed=0\n后续建议: 继续修复\n失败点 cargo check\n恢复上下文 run-1",
            "真实回复",
        ),
        (
            "Real reply\nSummary status=success tools=1/1 failed=0 tokens=5113\nUsage Token total=5113 input=5009 cache_read=0 cache_write=0 output=104 reasoning=0\nTool summary: completed=1 failed=0 latest=shell\nNext step: retry provider\nLast failure: cargo check\nResume context: run-1\nverify stopped: status=stopped tools=0/0 failed=0 verify=none tokens=5113",
            "Real reply",
        ),
        (
            "真实回复\n✓ Summary 任务完成：status=success tools=0/0 failed=0 verify=none tokens=5113\n▸ ◌ Usage Token 使用：total=5113 input=5009 cache_read=0 cache_write=0 output=104 reasoning=0",
            "真实回复",
        ),
        (
            "真实回复\nSummary 任务完成：status=success tools=0/0 failed=0 verify=none tokens=5113\nTool Summary 工具摘要：completed=1 failed=0 latest=shell\nUsage Token 使用：total=5113 input=5009 cache_read=0 cache_write=0 output=104 reasoning=0",
            "真实回复",
        ),
    ];

    for (content, expected) in cases {
        let result = CliRunResult {
            stdout: assistant_wire_stdout(content),
            stderr: String::new(),
            exit_code: 0,
        };

        let output = format_tui_run_output(&result);

        assert_eq!(assistant_texts(&output), vec![expected]);
    }
}

#[test]
fn format_tui_run_output_keeps_natural_verify_sentence() {
    let result = CliRunResult {
        stdout: assistant_wire_stdout("验证 输入区域的交互是否符合预期\n真实回复"),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert_eq!(
        assistant_texts(&output),
        vec!["验证 输入区域的交互是否符合预期\n真实回复"]
    );
}

#[test]
fn format_tui_run_output_keeps_natural_next_step_sentence() {
    let result = CliRunResult {
        stdout: assistant_wire_stdout("下一步我们直接收敛渲染区噪声\n真实回复"),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert_eq!(
        assistant_texts(&output),
        vec!["下一步我们直接收敛渲染区噪声\n真实回复"]
    );
}

#[test]
fn format_tui_run_output_keeps_natural_english_summary_sentence() {
    let result = CliRunResult {
        stdout: assistant_wire_stdout(
            "Next step we should compare Codex rendering first\nModel behavior matters when tools produce visible output\nThe summary is part of the answer",
        ),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert_eq!(
        assistant_texts(&output),
        vec![
            "Next step we should compare Codex rendering first\nModel behavior matters when tools produce visible output\nThe summary is part of the answer"
        ]
    );
}

fn assistant_wire_stdout(content: &str) -> String {
    serde_json::json!({
        "wireEvents": [{
            "schemaVersion": "axon.wire.v3",
            "eventId": "assistant-test-event",
            "correlationId": "assistant-test",
            "session": {
                "sessionId": "assistant-test-session"
            },
            "sequence": 1,
            "recordedAtMs": 1,
            "kind": {
                "type": "assistantMessage",
                "payload": {
                    "partId": "assistant-part:test",
                    "messageId": "provider-message:test",
                    "content": content,
                    "delta": false
                }
            }
        }]
    })
    .to_string()
}

fn assistant_texts(output: &AgentTuiRunOutput) -> Vec<&str> {
    output
        .lines()
        .iter()
        .filter_map(|line| match line {
            AgentTuiRunLine::Message(message)
                if message.role == AgentTuiMessagePartRole::Assistant =>
            {
                Some(message.text.as_str())
            }
            _ => None,
        })
        .collect()
}
