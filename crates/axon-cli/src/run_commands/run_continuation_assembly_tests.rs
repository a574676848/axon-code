//! continuation 恢复装配单元测试（合同 §5.3.3）。

use axon_session::{AgentContinuationRecord, AgentContinuationStore, SqliteRuntimeStore};

use super::{
    ASSEMBLY_CONSUMER_PREFIX, ASSEMBLY_TRUNCATION_MARKER, ContinuationAssemblyReport,
    bounded_assembly_text, render_assembly, token_summary,
};

fn sample_continuation(id: &str, path: &str, summary: &str) -> AgentContinuationRecord {
    AgentContinuationRecord::pending(
        id,
        format!("key-{id}"),
        "session-1",
        "session-1",
        "child-1",
        path,
        "completed",
        "子代理正常结束",
        summary,
        1_000,
        100_000,
    )
}

#[test]
fn render_assembly_formats_tokens_and_artifacts() {
    let mut record = sample_continuation("c1", "/root/review", "审查通过");
    record = record
        .with_artifact_path("/tmp/artifact.log")
        .with_tokens(100, 20, 5);

    let rendered = render_assembly(&[record]);
    assert!(rendered.contains("路径：/root/review"));
    assert!(rendered.contains("终态：completed"));
    assert!(rendered.contains("完整结果：/tmp/artifact.log"));
    assert!(rendered.contains("Token 用量：输入 100，输出 20，推理 5"));
    assert!(rendered.contains("摘要：审查通过"));
}

#[test]
fn render_assembly_omits_none_fields() {
    let record = sample_continuation("c1", "/root/worker", "无附件和用量");
    let rendered = render_assembly(&[record]);
    assert!(rendered.contains("路径：/root/worker"));
    assert!(!rendered.contains("完整结果："));
    assert!(!rendered.contains("Token 用量："));
    assert!(rendered.contains("摘要：无附件和用量"));
}

#[test]
fn token_summary_returns_none_when_input_or_output_missing() {
    let mut record = sample_continuation("c1", "/root/worker", "摘要");
    assert_eq!(token_summary(&record), None);

    record.token_input = Some(10);
    assert_eq!(token_summary(&record), None);

    record.token_output = Some(20);
    assert_eq!(
        token_summary(&record),
        Some("输入 10，输出 20，推理 0".to_owned())
    );

    record.token_reasoning = Some(7);
    assert_eq!(
        token_summary(&record),
        Some("输入 10，输出 20，推理 7".to_owned())
    );
}

#[test]
fn bounded_assembly_text_truncates_long_content() {
    let long_text = "测".repeat(5_000);
    let bounded = bounded_assembly_text(&long_text);
    assert!(bounded.chars().count() <= axon_agent_runtime::CONTINUATION_ASSEMBLY_MAX_CHARS);
    assert!(bounded.ends_with(ASSEMBLY_TRUNCATION_MARKER));
}

#[test]
fn continuation_assembly_report_helpers() {
    let empty = ContinuationAssemblyReport::default();
    assert_eq!(empty.candidate_count, 0);
    assert_eq!(empty.assembled_count, 0);
    assert!(!empty.assembled_any());

    let some = ContinuationAssemblyReport {
        candidate_count: 2,
        assembled_count: 1,
    };
    assert!(some.assembled_any());
}

#[test]
fn consume_in_assembly_uses_consistent_prefix() {
    let dir = axon_test_support::test_temp_dir();
    let db_path = dir.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(&db_path);

    let record = sample_continuation("c1", "/root/review", "结论");
    store.append_continuation(&record).unwrap();

    let consumer = format!("{ASSEMBLY_CONSUMER_PREFIX}session-1");
    let consumed = store.consume_continuation("c1", &consumer, 2_000).unwrap();
    assert!(consumed);

    let loaded = store.continuation("c1").unwrap().unwrap();
    assert_eq!(loaded.consumed_by.as_deref(), Some("run:session-1"));
    assert_eq!(loaded.consumed_at_ms, Some(2_000));
}
