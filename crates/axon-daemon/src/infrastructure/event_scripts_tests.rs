//! provider 脚本桥契约测试。

use super::*;
use std::fs;

const PROVIDER: &str = "jira";
const VALID_SCRIPT: &str = r#"import json, sys
payload = json.load(sys.stdin)["payload"]
issue = payload["issue"]["key"]
summary = payload["issue"]["fields"]["summary"]
print(json.dumps({
    "event_id": "jira:" + issue,
    "correlation_key": "jira:" + issue + ":assigned",
    "title": "工单已指派",
    "content": "工单 " + issue + "：" + summary,
}, ensure_ascii=False))
"#;

fn write_provider(root: &Path, provider: &str, script: &str) {
    let directory = root.join(provider);
    fs::create_dir_all(&directory).expect("创建 provider 目录");
    fs::write(directory.join(EVENT_PROVIDER_PARSER_FILE_NAME), script).expect("写入解析脚本");
}

fn python_available() -> bool {
    non_interactive_command(EVENT_SCRIPT_INTERPRETER)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn payload() -> serde_json::Value {
    serde_json::json!({
        "issue": {"key": "AXON-1", "fields": {"summary": "订单导出接口偶发 500"}},
        "webhookEvent": "jira:issue_assigned",
    })
}

#[test]
fn provider_name_validation_rejects_unsafe_names() {
    assert!(EventProviderRegistry::validate_provider_name("jira").is_ok());
    assert!(EventProviderRegistry::validate_provider_name("sls-alert_v2").is_ok());
    for invalid in [
        "",
        "..",
        "../evil",
        "a/b",
        "a\\b",
        "C:",
        "jira events",
        "Jira",
    ] {
        assert!(
            EventProviderRegistry::validate_provider_name(invalid).is_err(),
            "{invalid} 必须被拒绝"
        );
    }
    let long_name = "x".repeat(EVENT_PROVIDER_NAME_MAX_LENGTH + 1);
    assert!(EventProviderRegistry::validate_provider_name(&long_name).is_err());
}

#[test]
fn registered_provider_exposes_sop_path_only_when_present() {
    let temp = axon_test_support::test_temp_dir();
    let registry = EventProviderRegistry::new(temp.path());
    assert!(registry.sop_path(PROVIDER).is_none());
    write_provider(temp.path(), PROVIDER, VALID_SCRIPT);
    assert!(registry.sop_path(PROVIDER).is_none());
    fs::write(
        temp.path()
            .join(PROVIDER)
            .join(EVENT_PROVIDER_SOP_FILE_NAME),
        "# Jira SOP",
    )
    .expect("写入 SOP 文档");
    let sop = registry.sop_path(PROVIDER).expect("SOP 路径");
    assert!(sop.ends_with(EVENT_PROVIDER_SOP_FILE_NAME));
    assert!(registry.sop_path("../evil").is_none());
}

#[test]
fn missing_provider_and_missing_parser_fail_closed() {
    let temp = axon_test_support::test_temp_dir();
    let registry = EventProviderRegistry::new(temp.path());
    assert!(matches!(
        registry.run_parser(PROVIDER, &payload(), 0),
        Err(EventScriptError::UnknownProvider(name)) if name == PROVIDER
    ));
    fs::create_dir_all(temp.path().join(PROVIDER)).expect("创建 provider 目录");
    assert!(matches!(
        registry.run_parser(PROVIDER, &payload(), 0),
        Err(EventScriptError::MissingParser(name)) if name == PROVIDER
    ));
}

#[test]
fn output_validation_requires_namespaced_event_id_and_fields() {
    let namespaced = EventScriptEvent {
        event_id: "jira:AXON-1".to_owned(),
        correlation_key: None,
        title: "标题".to_owned(),
        content: "正文".to_owned(),
    };
    assert_eq!(
        EventScriptEvent::validate(PROVIDER, namespaced.clone())
            .expect("合法输出")
            .debounce_key(),
        "jira:AXON-1"
    );
    let mut unnamespaced = namespaced.clone();
    unnamespaced.event_id = "AXON-1".to_owned();
    assert!(matches!(
        EventScriptEvent::validate(PROVIDER, unnamespaced),
        Err(EventScriptError::InvalidOutput(_))
    ));
    let mut empty_title = namespaced.clone();
    empty_title.title = "  ".to_owned();
    assert!(matches!(
        EventScriptEvent::validate(PROVIDER, empty_title),
        Err(EventScriptError::InvalidOutput(_))
    ));
    let mut empty_content = namespaced.clone();
    empty_content.content = String::new();
    assert!(matches!(
        EventScriptEvent::validate(PROVIDER, empty_content),
        Err(EventScriptError::InvalidOutput(_))
    ));
    let mut long_key = namespaced;
    long_key.correlation_key = Some("k".repeat(MAX_EXTERNAL_EVENT_CORRELATION_KEY_LENGTH + 1));
    assert!(matches!(
        EventScriptEvent::validate(PROVIDER, long_key),
        Err(EventScriptError::InvalidOutput(_))
    ));
}

#[test]
fn schema_declares_exactly_the_runtime_contract() {
    let schema: serde_json::Value =
        serde_json::from_str(EVENT_SCRIPT_SCHEMA_JSON).expect("契约文件必须是合法 JSON");
    let mut declared = schema["properties"]
        .as_object()
        .expect("properties 必须是对象")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    declared.sort();
    let mut expected = EVENT_SCRIPT_OUTPUT_FIELDS
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(declared, expected);
    assert_eq!(schema["additionalProperties"], serde_json::json!(false));
    let required = schema["required"]
        .as_array()
        .expect("required 必须是数组")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(required.contains(&"event_id"));
    assert!(required.contains(&"title"));
    assert!(required.contains(&"content"));
    assert!(!required.contains(&"correlation_key"));
}

#[test]
fn parser_script_round_trip_and_failures_are_fail_closed() {
    if !python_available() {
        eprintln!("跳过：本机未安装 {EVENT_SCRIPT_INTERPRETER}");
        return;
    }
    let temp = axon_test_support::test_temp_dir();
    let registry = EventProviderRegistry::new(temp.path());
    write_provider(temp.path(), PROVIDER, VALID_SCRIPT);
    let event = registry
        .run_parser(PROVIDER, &payload(), 1_760_000_000_000)
        .expect("脚本输出应通过契约");
    assert_eq!(event.event_id, "jira:AXON-1");
    assert_eq!(event.debounce_key(), "jira:AXON-1:assigned");
    // 中文字段必须双向原样往返：Windows 默认代码页会破坏未固定 UTF-8 的脚本 stdio。
    assert_eq!(event.title, "工单已指派");
    assert_eq!(event.content, "工单 AXON-1：订单导出接口偶发 500");

    write_provider(
        temp.path(),
        PROVIDER,
        "import sys\nsys.stderr.write('boom')\nsys.exit(3)\n",
    );
    match registry
        .run_parser(PROVIDER, &payload(), 0)
        .expect_err("非零退出必须失败")
    {
        EventScriptError::Failed { code, stderr } => {
            assert_eq!(code, Some(3));
            assert!(stderr.contains("boom"));
        }
        other => panic!("期望 Failed，实际 {other:?}"),
    }

    write_provider(temp.path(), PROVIDER, "print('not json')\n");
    assert!(matches!(
        registry.run_parser(PROVIDER, &payload(), 0),
        Err(EventScriptError::InvalidJson(_))
    ));

    write_provider(
        temp.path(),
        PROVIDER,
        "import json\nprint(json.dumps({'event_id': 'AXON-1', 'title': 't', 'content': 'c'}))\n",
    );
    assert!(matches!(
        registry.run_parser(PROVIDER, &payload(), 0),
        Err(EventScriptError::InvalidOutput(_))
    ));
}

#[test]
fn parser_script_content_is_not_capped() {
    if !python_available() {
        eprintln!("跳过：本机未安装 {EVENT_SCRIPT_INTERPRETER}");
        return;
    }
    let temp = axon_test_support::test_temp_dir();
    let registry = EventProviderRegistry::new(temp.path());
    let script = format!(
        "import json\npayload = json.load(__import__('sys').stdin)\nprint(json.dumps({{'event_id': 'jira:AXON-9', 'title': '大正文', 'content': '中' * {}}}))\n",
        PARSER_CONTENT_CHARS
    );
    write_provider(temp.path(), PROVIDER, &script);
    let event = registry
        .run_parser(PROVIDER, &payload(), 0)
        .expect("超长正文由归档层处理，脚本输出不设上限");
    assert_eq!(event.content.chars().count(), PARSER_CONTENT_CHARS);
}

/// 脚本输出正文不设上限的用例长度。
const PARSER_CONTENT_CHARS: usize = 50_000;

#[test]
fn parser_script_timeout_reclaims_the_process_tree() {
    if !python_available() {
        eprintln!("跳过：本机未安装 {EVENT_SCRIPT_INTERPRETER}");
        return;
    }
    let temp = axon_test_support::test_temp_dir();
    let registry = EventProviderRegistry::new(temp.path());
    write_provider(temp.path(), PROVIDER, "import time\ntime.sleep(60)\n");
    let started = Instant::now();
    let error = registry
        .run_parser(PROVIDER, &payload(), 0)
        .expect_err("超过硬超时的事件必须被丢弃");
    assert_eq!(error, EventScriptError::Timeout);
    assert!(
        started.elapsed() < EVENT_SCRIPT_TIMEOUT + Duration::from_secs(2),
        "超时必须按硬截止时间收敛，实际耗时 {:?}",
        started.elapsed()
    );
}
