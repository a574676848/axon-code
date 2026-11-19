//! 后台 Goal 的可选阶段续跑报告。

use serde::Deserialize;

/// 执行者可选声明下一阶段；当前 Execution 的成功由 Executor 返回状态决定。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GoalExecutionReport {
    #[serde(default)]
    pub continuation: Option<GoalStageContinuation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GoalStageContinuation {
    pub stage: String,
    pub prompt: String,
}

pub(super) fn parse(response: &str) -> Option<GoalExecutionReport> {
    serde_json::from_str(response.trim()).ok()
}
