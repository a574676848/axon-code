use super::*;

#[test]
fn cancelled_compaction_is_rejected_at_commit_boundary() {
    assert!(ensure_compaction_commit_allowed(None).is_ok());

    let active = RunCancelToken::default();
    assert!(ensure_compaction_commit_allowed(Some(&active)).is_ok());

    active.cancel();
    assert_eq!(
        ensure_compaction_commit_allowed(Some(&active)).unwrap_err(),
        "压缩已取消"
    );
}
