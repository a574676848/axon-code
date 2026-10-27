//! 失败回执里告警的解析。

use super::failure_warnings;

#[test]
fn reads_warnings_from_a_structured_failure_payload() {
    let message = "运行失败：{\"event\":\"run_failed\",\"warnings\":[\"人格稳定段接近上限（5000 B）\"],\"failure\":{}}";

    assert_eq!(
        failure_warnings(message),
        vec!["人格稳定段接近上限（5000 B）".to_owned()]
    );
}

#[test]
fn missing_or_broken_payload_yields_no_warnings() {
    assert!(failure_warnings("provider 直接失败").is_empty());
    assert!(failure_warnings("失败：{\"event\":\"run_failed\"}").is_empty());
    assert!(failure_warnings("失败：{ 不是 JSON").is_empty());
}

#[test]
fn non_string_entries_are_skipped() {
    let message = "{\"warnings\":[\"ok\",1,null]}";

    assert_eq!(failure_warnings(message), vec!["ok".to_owned()]);
}
