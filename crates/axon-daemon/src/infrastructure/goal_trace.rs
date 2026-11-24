//! 目标看板只读面的 trace 级计时。
//!
//! 字段固定为 `goal_list_us` 与 `goal_session_check_us`：只记录时长与结果枚举，
//! 不记录目标正文、会话标识或路径。未启用 trace 时 [`GoalTrace::start`] 返回 `None`，
//! 调用方因此零计时、零分配，普通运行路径上不留任何额外开销。

use std::sync::LazyLock;
use std::time::Instant;

/// trace 开关的环境变量名。
pub const GOAL_TRACE_ENV: &str = "AXON_TRACE_GOAL";

/// 目标列表服务端本地耗时字段。
pub const GOAL_LIST_FIELD: &str = "goal_list_us";
/// 目标执行会话订阅校验耗时字段。
pub const GOAL_SESSION_FIELD: &str = "goal_session_check_us";

/// trace 开关：进程内只解析一次环境变量。
static ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var(GOAL_TRACE_ENV)
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "on" | "ON"))
        .unwrap_or(false)
});

/// trace 是否启用。
#[must_use]
pub fn enabled() -> bool {
    *ENABLED
}

/// 一次计时句柄；未启用时不创建，因此不存在计时开销。
#[derive(Debug)]
pub struct GoalTrace {
    field: &'static str,
    started: Instant,
}

impl GoalTrace {
    /// 启用时开始计时。
    #[must_use]
    pub fn start(field: &'static str) -> Option<Self> {
        enabled().then(|| Self {
            field,
            started: Instant::now(),
        })
    }

    /// 收口并写一条 trace 日志；`result` 只接受结果枚举。
    pub fn finish(self, result: &str) {
        let elapsed_us = self.started.elapsed().as_micros();
        axon_logger::debug(&format!("{}={elapsed_us} result={result}", self.field));
    }
}

#[cfg(test)]
mod tests {
    use super::{GOAL_LIST_FIELD, GOAL_SESSION_FIELD, GoalTrace, enabled};

    #[test]
    fn field_names_are_the_contract_fields() {
        // 两个字段名是性能合同的测量入口，改名等于测量口径失效。
        assert_eq!(GOAL_LIST_FIELD, "goal_list_us");
        assert_eq!(GOAL_SESSION_FIELD, "goal_session_check_us");
    }

    #[test]
    fn trace_is_off_by_default() {
        // 测试进程不设 AXON_TRACE_GOAL，因此计时句柄必须为空。
        assert!(!enabled());
        assert!(GoalTrace::start(GOAL_LIST_FIELD).is_none());
    }
}
