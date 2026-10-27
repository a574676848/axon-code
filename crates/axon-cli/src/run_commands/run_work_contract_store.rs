//! 会话级开工合同状态机注册表。
//!
//! `work_contract` 必须跨批次保持同一份合同状态，因此按运行会话持有；
//! 会话结束、最后一个引用释放后状态机随之销毁。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use axon_agent_runtime::WorkContractRuntime;

/// 取（或创建）某个运行会话的合同状态机。
pub(super) fn session_work_contract(session: &str) -> Arc<Mutex<WorkContractRuntime>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Mutex<WorkContractRuntime>>>>> =
        OnceLock::new();
    let mut registry = REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // 会话结束后回收，避免状态机常驻。
    registry.retain(|_, runtime| Arc::strong_count(runtime) > 1);
    Arc::clone(
        registry
            .entry(session.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(WorkContractRuntime::default()))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_session_shares_state_machine() {
        let first = session_work_contract("work-contract-test-session");
        let second = session_work_contract("work-contract-test-session");
        assert!(Arc::ptr_eq(&first, &second));
        let other = session_work_contract("work-contract-test-session-2");
        assert!(!Arc::ptr_eq(&first, &other));
    }
}
