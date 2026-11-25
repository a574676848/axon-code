//! 子代理名册状态的归一测试：两侧取值必须在这一层对齐。

use super::*;

#[test]
fn errored_is_reported_as_failed_to_the_workbench() {
    // 运行侧把失败写成 errored，界面只认 failed：两侧必须在这一层对齐。
    assert_eq!(normalize_last_turn_status(Some("errored")), Some("failed"));
}

#[test]
fn terminal_states_pass_through_unchanged() {
    for status in ["active", "completed", "interrupted", "failed"] {
        assert_eq!(normalize_last_turn_status(Some(status)), Some(status));
    }
}

#[test]
fn missing_status_stays_unknown() {
    // 旧库的会话行没有状态列：读作未知，不在这里猜成已完成。
    assert_eq!(normalize_last_turn_status(None), None);
}
