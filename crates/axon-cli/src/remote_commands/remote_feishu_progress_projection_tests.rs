//! 飞书折叠态卡片的渲染契约测试。

use super::*;
use crate::remote_commands::remote_feishu_progress_model::ProjectionEvent;

fn projection_with_body(text: &str) -> ProgressProjection {
    let mut projection = ProgressProjection::default();
    projection.apply(ProjectionEvent::AssistantDelta {
        part_id: "part-1".to_owned(),
        text: text.to_owned(),
    });
    projection
}

#[test]
fn progress_body_hides_media_directive_lines() {
    assert_eq!(
        render_progress_body("说明\nMEDIA:C:\\tmp\\a.png\n结论").text,
        "说明\n结论"
    );
}

#[test]
fn progress_body_holds_back_possible_media_prefix() {
    for fragment in ["MED", "MEDIA", "MEDIA:"] {
        assert_eq!(
            render_progress_body(&format!("说明\n{fragment}")).text,
            "说明",
            "片段 {fragment} 应被扣留"
        );
    }
    assert_eq!(
        render_progress_body("说明\n媒体已生成").text,
        "说明\n媒体已生成"
    );
}

#[test]
fn progress_body_reports_truncation() {
    let rendered = render_progress_body(&"x".repeat(PROGRESS_CARD_MAX_CHARS + 10));
    assert!(rendered.truncated, "超出字符上限必须显式标记截断");
    assert_eq!(rendered.text.chars().count(), PROGRESS_CARD_MAX_CHARS + 1);
    assert!(!render_progress_body("短正文").truncated);
}

#[test]
fn tool_rows_render_one_self_contained_line() {
    let row = ToolRow {
        call_id: "call-1".to_owned(),
        label: "执行".to_owned(),
        target: Some("cargo test".to_owned()),
        status: RowStatus::Failed,
        meta: Some("exit 1".to_owned()),
    };
    assert_eq!(
        tool_row_line(&row),
        "✕ **执行** · cargo test · exit 1",
        "失败、类别、目标与读数必须在一行内自足"
    );

    let silent = ToolRow {
        call_id: "call-2".to_owned(),
        label: "上下文".to_owned(),
        target: None,
        status: RowStatus::Running,
        meta: None,
    };
    assert_eq!(tool_row_line(&silent), "◌ **上下文**");
}

#[test]
fn tool_row_target_is_bounded_to_one_line() {
    let row = ToolRow {
        call_id: "call-1".to_owned(),
        label: "执行".to_owned(),
        target: Some("x".repeat(TOOL_TARGET_MAX_CHARS + 20)),
        status: RowStatus::Completed,
        meta: None,
    };
    let line = tool_row_line(&row);
    assert_eq!(
        line.chars().count(),
        "✓ **执行** · ".chars().count() + TOOL_TARGET_MAX_CHARS + 1
    );
    assert!(line.ends_with('…'));
}

#[test]
fn card_puts_process_rows_first_then_body_then_terminal_footer() {
    let mut projection = projection_with_body("结论正文");
    projection.apply(ProjectionEvent::ToolStarted {
        call_id: "call-1".to_owned(),
        label: "读取".to_owned(),
        target: Some("a.md".to_owned()),
    });
    projection.apply(ProjectionEvent::Route {
        route: "cpa/gemini-3.8-flash-high".to_owned(),
    });

    let running = render_card(&projection, CardStatus::Running, 0);
    let content: serde_json::Value =
        serde_json::from_str(&running.content_json).expect("卡片 JSON");
    assert_eq!(content["schema"], "2.0");
    assert_eq!(content["header"]["title"]["content"], "Axon · 执行中");
    let markdown = content["body"]["elements"][0]["content"]
        .as_str()
        .expect("markdown");
    assert_eq!(
        markdown, "◌ **读取** · a.md\n\n结论正文",
        "结论必须在过程行下方：卡片向下生长，读者不该为看结论向上回滚"
    );
    assert!(
        !markdown.contains("cpa/gemini"),
        "运行期不渲染路由，避免状态变化触发无效 PATCH"
    );

    let done = render_card(&projection, CardStatus::Completed, 12_400);
    let content: serde_json::Value = serde_json::from_str(&done.content_json).expect("卡片 JSON");
    let markdown = content["body"]["elements"][0]["content"]
        .as_str()
        .expect("markdown");
    assert_eq!(
        markdown,
        "◌ **读取** · a.md\n\n结论正文\n\n已完成 · 12.4s · cpa/gemini-3.8-flash-high"
    );
}

#[test]
fn card_window_keeps_only_the_latest_five_rows() {
    let mut projection = ProgressProjection::default();
    for index in 0..(PROCESS_WINDOW_ROWS + 5) {
        projection.apply(ProjectionEvent::ToolStarted {
            call_id: format!("call-{index}"),
            label: "读取".to_owned(),
            target: Some(format!("file-{index}.md")),
        });
    }
    let rendered = render_card(&projection, CardStatus::Running, 0);
    let content: serde_json::Value =
        serde_json::from_str(&rendered.content_json).expect("卡片 JSON");
    let markdown = content["body"]["elements"][0]["content"]
        .as_str()
        .expect("markdown");
    assert!(
        markdown.starts_with("… 5 个动作已省略\n"),
        "更早的 5 个动作必须压成一行计数：{markdown}"
    );
    assert!(markdown.contains("file-9.md"), "最近的行必须保留");
    assert!(!markdown.contains("file-4.md"), "窗口外的行必须让位");
}

#[test]
fn budget_shrink_drops_all_rows_before_touching_body() {
    let rows = vec!["✓ **执行** · a".to_owned(), "✓ **执行** · b".to_owned()];
    assert_eq!(
        assemble("结论", &rows, None, 0),
        "结论",
        "预算极紧时过程行先让位，正文必须保住"
    );
    assert_eq!(
        assemble("结论", &rows, None, rows.len()),
        "✓ **执行** · a\n✓ **执行** · b\n\n结论"
    );
}

#[test]
fn maximal_body_and_window_stay_within_budget() {
    let mut projection = projection_with_body(&"结".repeat(PROGRESS_CARD_MAX_CHARS));
    for index in 0..PROCESS_WINDOW_ROWS {
        projection.apply(ProjectionEvent::ToolStarted {
            call_id: format!("call-{index}"),
            label: "执行".to_owned(),
            target: Some("模".repeat(TOOL_TARGET_MAX_CHARS)),
        });
    }
    let rendered = render_card(&projection, CardStatus::Completed, 60_000);
    assert!(
        rendered.content_json.len() <= CARD_BUDGET_BYTES,
        "卡片序列化 {} 字节越过预算",
        rendered.content_json.len()
    );
    assert!(rendered.body_complete, "正文未超字符上限时不得被预算截断");
}

#[test]
fn oversized_body_is_marked_incomplete() {
    let rendered = render_card(
        &projection_with_body(&"结".repeat(PROGRESS_CARD_MAX_CHARS + 10)),
        CardStatus::Running,
        0,
    );
    assert!(
        !rendered.body_complete,
        "正文被字符上限截断时必须标记未完整投影，由 outbox 无损补投"
    );
    assert!(rendered.body_text.ends_with('…'));
}

#[test]
fn uncompressed_body_within_budget_is_complete() {
    let rendered = render_card(&projection_with_body("短结论"), CardStatus::Running, 0);
    assert!(rendered.body_complete);
    assert_eq!(rendered.body_text, "短结论");
}

#[test]
fn tool_label_follows_shared_display_rules() {
    assert_eq!(render_tool_label("shell_command"), "执行");
    assert_eq!(render_tool_label("mcp__team_tools__query_db"), LABEL_MCP);
    assert_eq!(render_tool_label(""), LABEL_TOOL_FALLBACK);
    assert_eq!(render_tool_label("brand_new_tool"), "brand_new_tool");
}

#[test]
fn tool_target_prefers_model_intent_over_command() {
    assert_eq!(
        render_tool_target(
            "shell_command",
            Some(r#"{"command":"cargo fmt --all --check"}"#),
            Some("校验代码格式")
        )
        .as_deref(),
        Some("校验代码格式")
    );
}

#[test]
fn tool_target_falls_back_to_primary_argument() {
    assert_eq!(
        render_tool_target(
            "shell_command",
            Some(r#"{"command":"cargo test","yield_time_ms":1000}"#),
            None
        )
        .as_deref(),
        Some("cargo test"),
        "没有概要时给命令原文，而不是整包 JSON"
    );
    assert_eq!(
        render_tool_target("read_file", Some(r#"{"path":"crates/axon-cli"}"#), None).as_deref(),
        Some("crates/axon-cli")
    );
    assert_eq!(
        render_tool_target("read_file", Some("not json"), None).as_deref(),
        Some("not json")
    );
    assert_eq!(render_tool_target("shell_command", None, Some("   ")), None);
    assert_eq!(render_tool_target("thread_unknown", None, None), None);
}

#[test]
fn tool_target_uses_shared_rules_per_tool() {
    let cases = [
        (
            "memory_search",
            r#"{"limit":8,"query":"仓库文档重整"}"#,
            "仓库文档重整",
        ),
        (
            "invoke_skill",
            r#"{"skill_name":"open-source-docs"}"#,
            "open-source-docs",
        ),
        ("delegate_to_fast", r#"{"task":"审计 docs"}"#, "审计 docs"),
        (
            "send_message",
            r#"{"message":"尽快返回结论"}"#,
            "尽快返回结论",
        ),
        (
            "job_output",
            r#"{"job_id":"1","yield_time_ms":0}"#,
            "等待 1 结果",
        ),
        (
            "new_context",
            r#"{"reason":"清理已完成，准备换窗"}"#,
            "清理已完成，准备换窗",
        ),
        (
            "mcp__codebase-memory-mcp__get_architecture",
            r#"{"project":"E-zbg-axon-code"}"#,
            "codebase-memory-mcp.get_architecture",
        ),
    ];
    for (tool_name, arguments, expected) in cases {
        assert_eq!(
            render_tool_target(tool_name, Some(arguments), None).as_deref(),
            Some(expected),
            "{tool_name} 的折叠态目标"
        );
    }
    assert_eq!(
        render_tool_target(
            "get_context_remaining",
            Some(r#"{"__axon_tool_call_parse_error":"empty_arguments"}"#),
            None
        ),
        None,
        "参数没有信息量时不得拼接任何目标"
    );
}

#[test]
fn tool_target_collapses_lines_and_bounds_length() {
    let long = format!(r#"{{"task":"审计 {}"}}"#, "x".repeat(400));
    let target = render_tool_target("delegate_to_fast", Some(&long), None).expect("任务原文");
    assert_eq!(target.chars().count(), TOOL_TARGET_MAX_CHARS + 1);
    assert!(target.ends_with('…'));
    assert_eq!(
        render_tool_target(
            "send_message",
            Some(r#"{"message":"先   看\n  日志"}"#),
            None
        )
        .as_deref(),
        Some("先 看 日志")
    );
}
