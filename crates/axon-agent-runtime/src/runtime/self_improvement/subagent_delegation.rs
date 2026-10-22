//! Subagent 真实委派契约。
//!
//! 既有 `self_iteration` 只是记忆/skill 治理的 hook 开关，并非真正的子代理委派。
//! 本模块补齐真实委派：父任务可把一个有界子任务委派给隔离的 subagent，
//! 受递归深度与并发数守卫，子代理继承指定的模型/工具/历史快照，
//! 完成后回汇完整结果与成本，不泄漏 provider raw payload。
//!
//! 设计约束（对齐 Codex 委派语义，且不破坏本机审批边界）：
//! - 递归守卫：subagent 内不得再委派 subagent，杜绝无界递归。
//! - 并发守卫：单批委派数量有界。
//! - 隔离：子代理上下文与父任务隔离，结果由后续统一上下文预算治理。
//! - 远控关闭：远控不得直接驱动委派。

use axon_domain::{AxonError, AxonResult, SessionId};

/// 单批最大委派数量。
pub const SUBAGENT_DELEGATION_MAX_FANOUT: usize = 8;

/// 委派计划已生成审计原因。
pub const SUBAGENT_DELEGATION_REASON_PLANNED: &str = "subagent_delegation:planned";
/// 委派隔离上下文审计原因。
pub const SUBAGENT_DELEGATION_REASON_CONTEXT_ISOLATED: &str =
    "subagent_delegation:context_isolated";
/// 委派远控关闭审计原因。
pub const SUBAGENT_DELEGATION_REASON_REMOTE_TRIGGER_CLOSED: &str =
    "subagent_delegation:remote_trigger_closed";
/// 委派被递归守卫阻断审计原因。
pub const SUBAGENT_DELEGATION_REASON_RECURSION_BLOCKED: &str =
    "subagent_delegation:recursion_blocked";
/// 委派结果已回汇审计原因。
pub const SUBAGENT_DELEGATION_REASON_RESULTS_MERGED: &str = "subagent_delegation:results_merged";
/// 委派结果中断需传播审计原因。
pub const SUBAGENT_DELEGATION_REASON_INTERRUPT_PROPAGATED: &str =
    "subagent_delegation:interrupt_propagated";

/// 子代理上下文继承模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentInheritance {
    /// 继承父代理的模型、工具与历史快照。
    ModelToolsHistory,
    /// 仅继承模型与工具，使用全新隔离历史。
    ModelToolsOnly,
}

/// 单个委派子任务请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDelegationRequest {
    /// 子任务标签。
    pub label: String,
    /// 子任务消息。
    pub task_message: String,
    /// 上下文继承模式。
    pub inheritance: SubagentInheritance,
    /// 子代理最长运行时间。
    pub max_runtime_ms: Option<u64>,
}

/// 委派规划输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDelegationInput {
    /// 父会话。
    pub session_id: SessionId,
    /// 当前是否已在 subagent 内。
    pub is_subagent: bool,
    /// 是否远控触发。
    pub remote_requested: bool,
    /// 委派子任务请求，顺序即调度顺序。
    pub requests: Vec<SubagentDelegationRequest>,
}

/// 单个委派条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDelegationEntry {
    /// 调度顺序。
    pub dispatch_order: usize,
    /// 子任务标签。
    pub label: String,
    /// 子任务消息。
    pub task_message: String,
    /// 上下文继承模式。
    pub inheritance: SubagentInheritance,
    /// 子代理最长运行时间。
    pub max_runtime_ms: Option<u64>,
}

/// 委派计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDelegationPlan {
    /// 已规划的委派条目。
    pub entries: Vec<SubagentDelegationEntry>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 子代理运行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentRunStatus {
    /// 已完成且未产生持久写入。
    CompletedNoWrite,
    /// 已完成并产生持久写入。
    CompletedWithWrite,
    /// 非中断错误；不得遮蔽父任务结果。
    FailedNonFatal,
    /// 用户中断；必须向父任务传播。
    Interrupted,
}

/// 单个子代理执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRunResult {
    /// 调度顺序。
    pub dispatch_order: usize,
    /// 子任务标签。
    pub label: String,
    /// 运行状态。
    pub status: SubagentRunStatus,
    /// 子代理结果摘要；不得包含 provider raw payload。
    pub bounded_summary: String,
    /// 子代理成本，单位微美元。
    pub cost_micros: u64,
}

/// 委派回汇条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentMergedEntry {
    /// 调度顺序。
    pub dispatch_order: usize,
    /// 子任务标签。
    pub label: String,
    /// 运行状态。
    pub status: SubagentRunStatus,
    /// 子代理结果摘要。
    pub bounded_summary: String,
    /// 子代理成本，单位微美元。
    pub cost_micros: u64,
}

/// 委派回汇报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDelegationMergeReport {
    /// 已回汇条目。
    pub entries: Vec<SubagentMergedEntry>,
    /// 完成数量。
    pub completed_count: usize,
    /// 失败数量。
    pub failed_count: usize,
    /// 子代理成本合计。
    pub subagent_cost_micros: u64,
    /// 父原始成本。
    pub parent_total_cost_micros: u64,
    /// 合并后总成本。
    pub merged_total_cost_micros: u64,
    /// 是否必须向父任务传播中断。
    pub interrupt_should_propagate: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Subagent 委派 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SubagentDelegationRuntime;

impl SubagentDelegationRuntime {
    /// 规划真实子代理委派；受递归与并发守卫，远控触发一律拒绝。
    pub fn plan(&self, input: SubagentDelegationInput) -> AxonResult<SubagentDelegationPlan> {
        if input.is_subagent {
            return Err(AxonError::runtime_invalid_state(
                SUBAGENT_DELEGATION_REASON_RECURSION_BLOCKED,
            ));
        }
        if input.remote_requested {
            return Ok(SubagentDelegationPlan {
                entries: Vec::new(),
                remote_trigger_allowed: false,
                reasons: vec![SUBAGENT_DELEGATION_REASON_REMOTE_TRIGGER_CLOSED.to_owned()],
            });
        }
        validate_requests(&input.requests)?;

        let entries = input
            .requests
            .into_iter()
            .enumerate()
            .map(|(dispatch_order, request)| SubagentDelegationEntry {
                dispatch_order,
                task_message: request.task_message,
                label: request.label,
                inheritance: request.inheritance,
                max_runtime_ms: request.max_runtime_ms,
            })
            .collect();

        Ok(SubagentDelegationPlan {
            entries,
            remote_trigger_allowed: false,
            reasons: vec![
                SUBAGENT_DELEGATION_REASON_PLANNED.to_owned(),
                SUBAGENT_DELEGATION_REASON_CONTEXT_ISOLATED.to_owned(),
            ],
        })
    }

    /// 将子代理 bounded 结果回汇父任务；合并成本并按需传播中断。
    pub fn merge_results(
        &self,
        plan: &SubagentDelegationPlan,
        parent_total_cost_micros: u64,
        results: Vec<SubagentRunResult>,
    ) -> AxonResult<SubagentDelegationMergeReport> {
        validate_merge(plan, &results)?;
        let mut ordered = results;
        ordered.sort_by_key(|result| result.dispatch_order);

        let mut entries = Vec::with_capacity(ordered.len());
        let mut completed_count = 0;
        let mut failed_count = 0;
        let mut subagent_cost_micros: u64 = 0;
        let mut interrupt_should_propagate = false;

        for result in ordered {
            match result.status {
                SubagentRunStatus::CompletedNoWrite | SubagentRunStatus::CompletedWithWrite => {
                    completed_count += 1;
                }
                SubagentRunStatus::FailedNonFatal => failed_count += 1,
                SubagentRunStatus::Interrupted => {
                    failed_count += 1;
                    interrupt_should_propagate = true;
                }
            }
            subagent_cost_micros = subagent_cost_micros.saturating_add(result.cost_micros);
            entries.push(SubagentMergedEntry {
                dispatch_order: result.dispatch_order,
                label: result.label,
                status: result.status,
                bounded_summary: result.bounded_summary,
                cost_micros: result.cost_micros,
            });
        }

        let mut reasons = vec![SUBAGENT_DELEGATION_REASON_RESULTS_MERGED.to_owned()];
        if interrupt_should_propagate {
            reasons.push(SUBAGENT_DELEGATION_REASON_INTERRUPT_PROPAGATED.to_owned());
        }

        Ok(SubagentDelegationMergeReport {
            entries,
            completed_count,
            failed_count,
            subagent_cost_micros,
            parent_total_cost_micros,
            merged_total_cost_micros: parent_total_cost_micros.saturating_add(subagent_cost_micros),
            interrupt_should_propagate,
            remote_trigger_allowed: false,
            reasons,
        })
    }
}

fn validate_requests(requests: &[SubagentDelegationRequest]) -> AxonResult<()> {
    if requests.is_empty() || requests.len() > SUBAGENT_DELEGATION_MAX_FANOUT {
        return Err(AxonError::runtime_failure("subagent 委派数量无效"));
    }
    for request in requests {
        if request.label.trim().is_empty() {
            return Err(AxonError::runtime_failure("subagent 委派 label 不能为空"));
        }
        if request.task_message.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "subagent 委派子任务消息不能为空",
            ));
        }
    }
    Ok(())
}

fn validate_merge(plan: &SubagentDelegationPlan, results: &[SubagentRunResult]) -> AxonResult<()> {
    if plan.entries.is_empty() || results.len() != plan.entries.len() {
        return Err(AxonError::runtime_failure(
            "subagent 委派结果数量与计划不匹配",
        ));
    }
    let mut seen = vec![false; plan.entries.len()];
    for result in results {
        let Some(slot) = seen.get_mut(result.dispatch_order) else {
            return Err(AxonError::runtime_failure("subagent 委派结果调度顺序越界"));
        };
        if *slot {
            return Err(AxonError::runtime_failure("subagent 委派结果调度顺序重复"));
        }
        *slot = true;
        if result.bounded_summary.trim().is_empty() {
            return Err(AxonError::runtime_failure("subagent 委派结果摘要不能为空"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(label: &str) -> SubagentDelegationRequest {
        SubagentDelegationRequest {
            label: label.to_owned(),
            task_message: format!("处理子任务 {label}"),
            inheritance: SubagentInheritance::ModelToolsHistory,
            max_runtime_ms: Some(30_000),
        }
    }

    fn plan_two() -> SubagentDelegationPlan {
        SubagentDelegationRuntime
            .plan(SubagentDelegationInput {
                session_id: SessionId(1),
                is_subagent: false,
                remote_requested: false,
                requests: vec![request("a"), request("b")],
            })
            .unwrap()
    }

    #[test]
    fn delegation_plan_orders_entries_and_isolates_context() {
        let plan = plan_two();

        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].dispatch_order, 0);
        assert_eq!(plan.entries[1].dispatch_order, 1);
        assert!(!plan.remote_trigger_allowed);
        assert_eq!(
            plan.reasons,
            [
                SUBAGENT_DELEGATION_REASON_PLANNED,
                SUBAGENT_DELEGATION_REASON_CONTEXT_ISOLATED
            ]
        );
    }

    #[test]
    fn delegation_blocks_recursion_inside_subagent() {
        let result = SubagentDelegationRuntime.plan(SubagentDelegationInput {
            session_id: SessionId(1),
            is_subagent: true,
            remote_requested: false,
            requests: vec![request("a")],
        });

        assert!(matches!(
            result,
            Err(AxonError::Runtime(
                axon_domain::RuntimeError::InvalidState(reason),
            )) if reason == SUBAGENT_DELEGATION_REASON_RECURSION_BLOCKED
        ));
    }

    #[test]
    fn delegation_blocks_remote_trigger() {
        let plan = SubagentDelegationRuntime
            .plan(SubagentDelegationInput {
                session_id: SessionId(1),
                is_subagent: false,
                remote_requested: true,
                requests: vec![request("a")],
            })
            .unwrap();

        assert!(plan.entries.is_empty());
        assert_eq!(
            plan.reasons,
            [SUBAGENT_DELEGATION_REASON_REMOTE_TRIGGER_CLOSED]
        );
    }

    #[test]
    fn delegation_rejects_empty_or_overflowing_fanout() {
        let empty = SubagentDelegationRuntime.plan(SubagentDelegationInput {
            session_id: SessionId(1),
            is_subagent: false,
            remote_requested: false,
            requests: Vec::new(),
        });
        let overflowing = SubagentDelegationRuntime.plan(SubagentDelegationInput {
            session_id: SessionId(1),
            is_subagent: false,
            remote_requested: false,
            requests: (0..=SUBAGENT_DELEGATION_MAX_FANOUT)
                .map(|index| request(&format!("task-{index}")))
                .collect(),
        });

        assert!(empty.is_err());
        assert!(overflowing.is_err());
    }

    #[test]
    fn delegation_preserves_full_task_and_result_summaries() {
        let task_message = "任务正文".repeat(1_000);
        let plan = SubagentDelegationRuntime
            .plan(SubagentDelegationInput {
                session_id: SessionId(1),
                is_subagent: false,
                remote_requested: false,
                requests: vec![SubagentDelegationRequest {
                    label: "large".to_owned(),
                    task_message: task_message.clone(),
                    inheritance: SubagentInheritance::ModelToolsOnly,
                    max_runtime_ms: None,
                }],
            })
            .unwrap();
        assert_eq!(plan.entries[0].task_message, task_message);

        let summary = "结果摘要".repeat(500);
        let report = SubagentDelegationRuntime
            .merge_results(
                &plan,
                0,
                vec![SubagentRunResult {
                    dispatch_order: 0,
                    label: "large".to_owned(),
                    status: SubagentRunStatus::CompletedNoWrite,
                    bounded_summary: summary.clone(),
                    cost_micros: 0,
                }],
            )
            .unwrap();
        assert_eq!(report.entries[0].bounded_summary, summary);
    }

    #[test]
    fn merge_results_combines_cost_and_counts() {
        let plan = plan_two();
        let report = SubagentDelegationRuntime
            .merge_results(
                &plan,
                1_000,
                vec![
                    SubagentRunResult {
                        dispatch_order: 1,
                        label: "b".to_owned(),
                        status: SubagentRunStatus::CompletedWithWrite,
                        bounded_summary: "b 完成".to_owned(),
                        cost_micros: 200,
                    },
                    SubagentRunResult {
                        dispatch_order: 0,
                        label: "a".to_owned(),
                        status: SubagentRunStatus::FailedNonFatal,
                        bounded_summary: "a 非致命失败".to_owned(),
                        cost_micros: 300,
                    },
                ],
            )
            .unwrap();

        assert_eq!(report.entries[0].dispatch_order, 0);
        assert_eq!(report.entries[1].dispatch_order, 1);
        assert_eq!(report.completed_count, 1);
        assert_eq!(report.failed_count, 1);
        assert_eq!(report.subagent_cost_micros, 500);
        assert_eq!(report.merged_total_cost_micros, 1_500);
        assert!(!report.interrupt_should_propagate);
    }

    #[test]
    fn merge_results_propagates_interrupt() {
        let plan = plan_two();
        let report = SubagentDelegationRuntime
            .merge_results(
                &plan,
                0,
                vec![
                    SubagentRunResult {
                        dispatch_order: 0,
                        label: "a".to_owned(),
                        status: SubagentRunStatus::Interrupted,
                        bounded_summary: "用户中断".to_owned(),
                        cost_micros: 10,
                    },
                    SubagentRunResult {
                        dispatch_order: 1,
                        label: "b".to_owned(),
                        status: SubagentRunStatus::CompletedNoWrite,
                        bounded_summary: "b 完成".to_owned(),
                        cost_micros: 10,
                    },
                ],
            )
            .unwrap();

        assert!(report.interrupt_should_propagate);
        assert!(
            report
                .reasons
                .contains(&SUBAGENT_DELEGATION_REASON_INTERRUPT_PROPAGATED.to_owned())
        );
    }

    #[test]
    fn merge_results_rejects_mismatched_or_duplicate_results() {
        let plan = plan_two();
        let mismatched = SubagentDelegationRuntime.merge_results(
            &plan,
            0,
            vec![SubagentRunResult {
                dispatch_order: 0,
                label: "a".to_owned(),
                status: SubagentRunStatus::CompletedNoWrite,
                bounded_summary: "ok".to_owned(),
                cost_micros: 1,
            }],
        );
        let duplicate = SubagentDelegationRuntime.merge_results(
            &plan,
            0,
            vec![
                SubagentRunResult {
                    dispatch_order: 0,
                    label: "a".to_owned(),
                    status: SubagentRunStatus::CompletedNoWrite,
                    bounded_summary: "ok".to_owned(),
                    cost_micros: 1,
                },
                SubagentRunResult {
                    dispatch_order: 0,
                    label: "a".to_owned(),
                    status: SubagentRunStatus::CompletedNoWrite,
                    bounded_summary: "ok".to_owned(),
                    cost_micros: 1,
                },
            ],
        );

        assert!(mismatched.is_err());
        assert!(duplicate.is_err());
    }
}
