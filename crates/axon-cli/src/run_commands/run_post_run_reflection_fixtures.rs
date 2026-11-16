//! 反思链路测试夹具：运行态隔离目录与后台会话校验。

use super::*;

pub(super) fn assert_no_running_subagent_sessions(request: &RunRequest) {
    let sessions = request
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        sessions
            .sessions()
            .iter()
            .all(|handle| handle.status != axon_agent_runtime::SubagentSessionStatus::Running),
        "post-run self-iteration 子代理必须完成会话闭环"
    );
}

pub(super) fn test_root(label: &str) -> std::path::PathBuf {
    static NEXT_TEST_ROOT_ID: AtomicU64 = AtomicU64::new(1);

    let root = axon_test_support::test_temp_root().join(format!(
        "axon-post-run-reflection-{label}-{}-{}",
        std::process::id(),
        NEXT_TEST_ROOT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}
