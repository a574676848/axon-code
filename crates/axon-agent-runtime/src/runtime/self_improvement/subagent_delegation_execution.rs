//! Subagent 委派执行桥接 runtime。
//!
//! 本模块不直接启动真实子线程，而是把 delegation plan 与调用方提供的执行器桥接起来，
//! 统一收口“逐条执行 -> bounded 结果 -> 成本合并 -> 中断传播”的运行时骨架。

use crate::{
    SubagentDelegationMergeReport, SubagentDelegationPlan, SubagentDelegationRuntime,
    SubagentRunResult,
};
use axon_domain::AxonResult;

/// Subagent 委派执行 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SubagentDelegationExecutionRuntime;

impl SubagentDelegationExecutionRuntime {
    /// 顺序执行 delegation plan，并回收统一 merge report。
    pub fn execute<TExecutor>(
        &self,
        plan: &SubagentDelegationPlan,
        parent_total_cost_micros: u64,
        mut executor: TExecutor,
    ) -> AxonResult<SubagentDelegationMergeReport>
    where
        TExecutor: FnMut(&crate::SubagentDelegationEntry, usize) -> AxonResult<SubagentRunResult>,
    {
        let mut results = Vec::with_capacity(plan.entries.len());
        for (index, entry) in plan.entries.iter().enumerate() {
            results.push(executor(entry, index)?);
        }
        SubagentDelegationRuntime.merge_results(plan, parent_total_cost_micros, results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SubagentDelegationInput, SubagentDelegationRequest, SubagentInheritance, SubagentRunStatus,
    };
    use axon_domain::SessionId;

    #[test]
    fn execution_runtime_merges_results_from_executor() {
        let plan = crate::SubagentDelegationRuntime
            .plan(SubagentDelegationInput {
                session_id: SessionId(1),
                is_subagent: false,
                remote_requested: false,
                requests: vec![
                    SubagentDelegationRequest {
                        label: "memory_update".to_owned(),
                        task_message: "persist memory".to_owned(),
                        inheritance: SubagentInheritance::ModelToolsHistory,
                        max_runtime_ms: None,
                    },
                    SubagentDelegationRequest {
                        label: "skill_reflection".to_owned(),
                        task_message: "reflect skill".to_owned(),
                        inheritance: SubagentInheritance::ModelToolsHistory,
                        max_runtime_ms: Some(30_000),
                    },
                ],
            })
            .unwrap();

        let report = SubagentDelegationExecutionRuntime
            .execute(&plan, 100, |entry, dispatch_order| {
                Ok(SubagentRunResult {
                    dispatch_order,
                    label: entry.label.clone(),
                    status: if dispatch_order == 0 {
                        SubagentRunStatus::CompletedWithWrite
                    } else {
                        SubagentRunStatus::CompletedNoWrite
                    },
                    bounded_summary: format!("{} done", entry.label),
                    cost_micros: 25,
                })
            })
            .unwrap();

        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.completed_count, 2);
        assert_eq!(report.subagent_cost_micros, 50);
        assert_eq!(report.merged_total_cost_micros, 150);
    }
}
