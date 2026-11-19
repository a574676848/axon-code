//! 一轮对话的执行段：请求就绪之后、终态记账之前。
//!
//! 这里不区分 turn 来源，两个入口跑的是同一段代码。收尾清理是唯一会波及其他 turn 的动作
//! （取消子代理、终止 shell 会话、释放终态作业），因此作用域由装载端显式交进来，
//! 执行段只按给定作用域执行，绝不自行推断「我应该清理谁」。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use axon_agent_runtime::{AgentPath, RunCancelToken};

use super::{
    RunRequest,
    run_execution::{ProviderStreamingRunner, RunExecutionError, RunResponse},
    run_progress::RunProgressSink,
};

/// 收尾清理的作用域。
///
/// 主 turn 拥有整轮的子代理与作业；执行子会话只是别人的一段工作，清理归它的父轮。
/// 装载端说清自己拥有什么，执行段照做。
pub(crate) struct TurnTeardownScope {
    subagent_cancel_tokens: Option<Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>>,
    shell_owner_session_id: Option<String>,
    terminal_job_owner_session_id: Option<String>,
}

impl TurnTeardownScope {
    /// 主 turn：整轮结束，取消名下所有子代理、终止它的 shell 会话、释放它的终态作业。
    ///
    /// 终态作业的查询消费者随父 turn 一起消失（结果只经完成主动通知交付），
    /// 立即释放能避免后台保留窗拖住本轮交付链路。
    pub(crate) fn primary(request: &RunRequest) -> Self {
        Self {
            subagent_cancel_tokens: Some(Arc::clone(&request.subagent_cancel_tokens)),
            shell_owner_session_id: Some(request.session_id.clone()),
            terminal_job_owner_session_id: Some(request.session_id.clone()),
        }
    }

    /// 执行子会话：它只是别人的一段工作，收尾归父轮，这里什么都不清理。
    ///
    /// 这也是「不误杀」的落点：子会话若按自己的令牌表清理，会连带取消兄弟子代理。
    pub(crate) fn none() -> Self {
        Self {
            subagent_cancel_tokens: None,
            shell_owner_session_id: None,
            terminal_job_owner_session_id: None,
        }
    }

    fn apply(&self) {
        if let Some(cancel_tokens) = &self.subagent_cancel_tokens {
            cancel_all_running_subagents(cancel_tokens);
        }
        if let Some(owner) = &self.shell_owner_session_id {
            axon_agent_runtime::ShellCommandToolRuntime::terminate_sessions_for_owner(owner);
        }
        if let Some(owner) = &self.terminal_job_owner_session_id {
            axon_agent_runtime::JobRuntime::global()
                .release_terminal_for_owner(owner, Instant::now());
        }
    }
}

/// 一次执行的原始结果：provider 的成败与起跑时刻。
///
/// 起跑时刻随结果一起交回，耗时由调用端在它自己的记账点上计算，
/// 执行段不替调用端决定「这一轮算了多久」。
pub(crate) struct TurnExecution {
    pub(crate) provider: Result<RunResponse, Box<RunExecutionError>>,
    pub(crate) started_at_ms: u64,
}

/// provider 循环的入口签名。
///
/// 是一个函数指针而不是泛型参数：执行段只允许一个实现，测试需要替换的也只是「谁去调
/// provider」，而不是整段编排。
pub(crate) type TurnProviderRunner = for<'a> fn(
    &RunRequest,
    Option<&RunProgressSink<'a>>,
) -> Result<RunResponse, Box<RunExecutionError>>;

/// 跑完一轮：provider 循环加收尾清理。
pub(crate) fn execute_turn(
    request: &RunRequest,
    progress: Option<&RunProgressSink<'_>>,
    teardown: &TurnTeardownScope,
) -> TurnExecution {
    execute_turn_with(ProviderStreamingRunner::run, request, progress, teardown)
}

/// 与 [`execute_turn`] 相同，但由调用方指定 provider 入口。
pub(crate) fn execute_turn_with(
    runner: TurnProviderRunner,
    request: &RunRequest,
    progress: Option<&RunProgressSink<'_>>,
    teardown: &TurnTeardownScope,
) -> TurnExecution {
    let started_at_ms = super::run_execution::now_ms();
    let provider = runner(request, progress);
    axon_logger::debug(&format!(
        "ProviderStreamingRunner 返回: session={}, elapsed_ms={}",
        request.session_id,
        super::run_execution::now_ms().saturating_sub(started_at_ms)
    ));
    teardown.apply();
    TurnExecution {
        provider,
        started_at_ms,
    }
}

fn cancel_all_running_subagents(cancel_tokens: &Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>) {
    let tokens = {
        let mut cancel_tokens = cancel_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *cancel_tokens)
    };
    for token in tokens.into_values() {
        token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_terminal_cancels_all_running_subagents() {
        let first_path = AgentPath::parse("/root/first").unwrap();
        let second_path = AgentPath::parse("/root/second").unwrap();
        let first_token = RunCancelToken::default();
        let second_token = RunCancelToken::default();
        let cancel_tokens = Arc::new(Mutex::new(HashMap::from([
            (first_path, first_token.clone()),
            (second_path, second_token.clone()),
        ])));

        cancel_all_running_subagents(&cancel_tokens);

        assert!(first_token.is_cancelled());
        assert!(second_token.is_cancelled());
        assert!(cancel_tokens.lock().unwrap().is_empty());
    }

    /// 执行子会话不得按父轮的令牌表收尾：那会连带取消兄弟子代理。
    #[test]
    fn child_scope_leaves_sibling_subagents_alone() {
        let sibling_path = AgentPath::parse("/root/sibling").unwrap();
        let sibling_token = RunCancelToken::default();
        let cancel_tokens = Arc::new(Mutex::new(HashMap::from([(
            sibling_path,
            sibling_token.clone(),
        )])));

        TurnTeardownScope::none().apply();

        assert!(!sibling_token.is_cancelled());
        assert_eq!(cancel_tokens.lock().unwrap().len(), 1);
    }
}
