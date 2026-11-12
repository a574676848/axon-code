//! 子代理模型选择：继承、显式指定与快速模型槽位。
//!
//! 与主 turn 的模型选择同源：显式指定走 `run_model_routing` 的渠道均衡入口，
//! 继承则原样沿用父代理的 provider、模型、渠道链与档位。选择结果同时决定
//! 子代理请求的 `reasoning_effort`，两者不能分头解析。

use super::super::{
    run_model_routing::{
        FAST_MODEL_SLOT_NAME, select_channel_balanced_model_by_name,
        select_channel_balanced_model_for_agent_slot,
    },
    run_request::hash_session_selector,
    run_subagent_executor_support::ChildModelSelection,
};
use super::{ProviderRunSubagentExecutor, RunSubagentExecutionInput, RunSubagentModelIntent};

impl ProviderRunSubagentExecutor {
    pub(super) fn child_model_selection(
        &self,
        input: &RunSubagentExecutionInput,
        session_id: &str,
    ) -> Result<ChildModelSelection, String> {
        match input.model_intent {
            RunSubagentModelIntent::Inherit => {
                self.inherit_child_model_selection(input, session_id)
            }
            // 快速委托由槽位决定模型：调用方传入的模型名与档位在这里不生效。
            RunSubagentModelIntent::FastDelegate => self.agent_slot_child_model_selection(
                &self.environment.config.agent.fast_model,
                FAST_MODEL_SLOT_NAME,
                session_id,
            ),
        }
    }

    /// 继承路径：显式模型名与档位覆盖优先，缺省原样沿用父代理。
    ///
    /// 只给模型名不给档位时沿用父代理档位：档位是同一请求意图，跨协议不支持的
    /// 档位由 provider 协商层按协议上限收敛，不在选择层拒绝。
    fn inherit_child_model_selection(
        &self,
        input: &RunSubagentExecutionInput,
        session_id: &str,
    ) -> Result<ChildModelSelection, String> {
        let inherited_effort = input
            .reasoning_effort_override
            .clone()
            .or_else(|| self.environment.reasoning_effort.clone());
        let Some(requested) = input.model_override.as_deref() else {
            return Ok(ChildModelSelection {
                provider: self.environment.provider.clone(),
                model: self.environment.model.clone(),
                channel_chain: self.environment.channel_chain.clone(),
                routing_key: self.environment.routing_key.clone(),
                reasoning_effort: inherited_effort,
            });
        };
        let balanced = select_channel_balanced_model_by_name(
            &self.environment.config,
            requested,
            session_id,
            hash_session_selector(session_id),
            crate::tool_orchestrator::now_ms(),
        )?;
        Ok(ChildModelSelection {
            provider: balanced.provider,
            model: balanced.model,
            channel_chain: balanced.chain,
            routing_key: balanced.routing_key,
            reasoning_effort: inherited_effort,
        })
    }

    fn agent_slot_child_model_selection(
        &self,
        slot: &str,
        slot_name: &str,
        session_id: &str,
    ) -> Result<ChildModelSelection, String> {
        let balanced = select_channel_balanced_model_for_agent_slot(
            &self.environment.config,
            slot,
            slot_name,
            session_id,
            hash_session_selector(session_id),
            crate::tool_orchestrator::now_ms(),
        )?;
        Ok(ChildModelSelection {
            provider: balanced.provider,
            model: balanced.model,
            channel_chain: balanced.chain,
            routing_key: balanced.routing_key,
            reasoning_effort: None,
        })
    }
}
