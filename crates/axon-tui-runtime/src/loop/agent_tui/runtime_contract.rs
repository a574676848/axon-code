//! Agent TUI 与宿主 runtime 的协作契约。

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use axon_agent_runtime::RunCancelToken;

use crate::surface::runtime_adapter::{RuntimeAdapterContract, RuntimeError, RuntimeEventEmitter};

use super::{
    catalog_loader::AgentTuiCatalogLoader,
    completion_inbox::AgentTuiCompletionInbox,
    launch::AgentTuiLaunchRequest,
    mcp_login_loader::{AgentTuiMcpAuthLoader, AgentTuiMcpAuthRequest},
    run_output::{AgentTuiRunOutput, AgentTuiRuntimeEvent},
    state::AgentTuiSubmit,
};

/// Agent TUI 当前提交的协作式取消信号。
#[derive(Debug, Clone, Default)]
pub struct AgentTuiCancelToken {
    run_cancel_token: RunCancelToken,
    lifecycle: Arc<AtomicU8>,
}

impl AgentTuiCancelToken {
    const CREATED: u8 = 0;
    const REQUESTED: u8 = 1;
    const OBSERVED: u8 = 2;
    const SETTLED: u8 = 3;

    /// 请求取消当前运行中的提交。
    pub fn cancel(&self) {
        self.run_cancel_token.cancel();
        self.lifecycle
            .compare_exchange(
                Self::CREATED,
                Self::REQUESTED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .ok();
    }

    /// 当前提交是否已被请求取消。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        let cancelled = self.run_cancel_token.is_cancelled();
        if cancelled {
            self.mark_cancel_observed();
        }
        cancelled
    }

    /// 标记取消请求已被 runtime 或 driver 观察到。
    pub fn mark_cancel_observed(&self) {
        self.lifecycle
            .compare_exchange(
                Self::REQUESTED,
                Self::OBSERVED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .ok();
    }

    /// 标记当前提交已结束，取消生命周期已收敛。
    pub fn settle(&self) {
        self.lifecycle.store(Self::SETTLED, Ordering::SeqCst);
    }

    /// 当前取消生命周期稳定编码。
    #[must_use]
    pub fn lifecycle(&self) -> &'static str {
        match self.lifecycle.load(Ordering::SeqCst) {
            Self::CREATED => "created",
            Self::REQUESTED => "requested",
            Self::OBSERVED => "observed",
            Self::SETTLED => "settled",
            _ => "unknown",
        }
    }

    /// 返回与当前 TUI 提交绑定的运行取消令牌。
    #[must_use]
    pub fn run_cancel_token(&self) -> RunCancelToken {
        self.run_cancel_token.clone()
    }
}

/// TUI 启动时发现的 Daemon 活动回合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiActiveTurn {
    /// 原 Interaction 的用户输入文本。
    pub prompt: String,
    /// 原 Interaction 所属的 TUI 车道 destination。
    pub reply_destination: Option<String>,
}

impl AgentTuiActiveTurn {
    /// 创建活动回合描述。
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            reply_destination: None,
        }
    }

    /// 创建绑定原始 TUI 车道的活动回合描述。
    #[must_use]
    pub fn with_reply_destination(
        prompt: impl Into<String>,
        reply_destination: impl Into<String>,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            reply_destination: Some(reply_destination.into()),
        }
    }
}

/// 运行中 steer 注入结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AgentTuiSteerResult {
    /// 当前 turn 接受了补充输入。
    Accepted,
    /// 当前宿主或当前 turn 不支持 steer，调用方应保留用户输入。
    #[default]
    Unsupported,
    /// adapter 支持 steer，但当前 turn 拒绝本次注入。
    Rejected,
}

/// 提交失败：结构化错误必须与宿主已获得的 durable 终态投影成对返回。
///
/// 实时邮箱只允许淘汰可重建的展示增量（见 `submission_worker_mailbox`），
/// 被淘汰的增量只能由终态按 durable 顺序完整重放补齐。失败若只携带错误文本，
/// 已积累的重放就永久缺失，正文会留下无法察觉的缺口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiSubmitFailure {
    error: RuntimeError,
    output: AgentTuiRunOutput,
}

impl AgentTuiSubmitFailure {
    /// 宿主没有任何 durable 累积（worker panic、提交前校验失败）时只携带错误。
    #[must_use]
    pub fn without_output(error: RuntimeError) -> Self {
        Self {
            error,
            output: AgentTuiRunOutput::empty(),
        }
    }

    /// 携带宿主已获得的 durable 终态投影。
    #[must_use]
    pub fn with_output(error: RuntimeError, output: AgentTuiRunOutput) -> Self {
        Self { error, output }
    }

    /// 结构化失败原因。
    #[must_use]
    pub fn error(&self) -> &RuntimeError {
        &self.error
    }

    /// 取回宿主交出的 durable 终态投影；宿主没有累积时为空输出。
    #[must_use]
    pub fn into_output(self) -> AgentTuiRunOutput {
        self.output
    }
}

impl From<RuntimeError> for AgentTuiSubmitFailure {
    fn from(error: RuntimeError) -> Self {
        Self::without_output(error)
    }
}

/// Agent TUI 宿主运行时适配器。
pub trait AgentTuiRuntimeAdapter: Send + Clone + 'static {
    /// 返回 adapter 契约声明。
    fn contract(&self) -> RuntimeAdapterContract {
        RuntimeAdapterContract::current()
    }

    /// 启动一个新的用户 turn。
    ///
    /// 失败必须连同宿主已获得的 durable 终态投影一起返回：实时邮箱淘汰的展示增量
    /// 只能由终态重放补齐，只上报错误会让正文留下缺口。
    fn submit(
        &self,
        submission: &AgentTuiSubmit,
        cancel_token: &AgentTuiCancelToken,
        emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiRunOutput, AgentTuiSubmitFailure>;

    /// 尝试把输入注入当前运行中的 turn。
    fn steer(
        &self,
        _submission: &AgentTuiSubmit,
        _emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiSteerResult, RuntimeError> {
        Ok(AgentTuiSteerResult::Unsupported)
    }

    /// 在首个 TUI 帧完成后启动会话预热。
    fn start_session_prewarm(&self, _request: &AgentTuiLaunchRequest) {}

    /// 加载进入 TUI 时应直接显示的最近已完成回合。
    ///
    /// 宿主无法读取历史时返回 None，不阻断 TUI 启动。
    fn load_initial_history(&self, _request: &AgentTuiLaunchRequest) -> Option<AgentTuiRunOutput> {
        None
    }

    /// 查询当前工作区是否存在可恢复的 Daemon 活动回合。
    fn load_active_turn(&self, _request: &AgentTuiLaunchRequest) -> Option<AgentTuiActiveTurn> {
        None
    }

    /// 恢复一个已提交且仍在执行的 Daemon 回合。
    fn resume_active_turn(
        &self,
        _active_turn: &AgentTuiActiveTurn,
        _request: &AgentTuiLaunchRequest,
        _cancel_token: &AgentTuiCancelToken,
        _emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<Option<AgentTuiRunOutput>, AgentTuiSubmitFailure> {
        Ok(None)
    }

    /// 启动 TUI 首屏后台 catalog 加载。
    fn start_catalog_loader(
        &self,
        _request: &AgentTuiLaunchRequest,
        _force_mcp_refresh: bool,
    ) -> Option<AgentTuiCatalogLoader> {
        None
    }

    /// 启动完成通知收件箱：声明客户端 presence 并轮询 Daemon 持久收件箱。
    ///
    /// 默认实现表示宿主没有 Daemon 收件箱，此时完成通知只会走回落渠道。
    fn start_completion_inbox(
        &self,
        _request: &AgentTuiLaunchRequest,
    ) -> Option<AgentTuiCompletionInbox> {
        None
    }

    /// 在后台执行 MCP OAuth 授权动作。
    ///
    /// 默认实现表示宿主不支持 TUI 内授权；调用方会退化为提示用户在终端执行 `axon mcp`。
    fn start_mcp_auth(
        &self,
        _request: &AgentTuiLaunchRequest,
        _auth: &AgentTuiMcpAuthRequest,
    ) -> Option<AgentTuiMcpAuthLoader> {
        None
    }
}

impl<F> AgentTuiRuntimeAdapter for F
where
    F: Fn(
            &AgentTuiSubmit,
            &AgentTuiCancelToken,
            &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
        ) -> Result<AgentTuiRunOutput, AgentTuiSubmitFailure>
        + Clone
        + Send
        + 'static,
{
    fn submit(
        &self,
        submission: &AgentTuiSubmit,
        cancel_token: &AgentTuiCancelToken,
        emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiRunOutput, AgentTuiSubmitFailure> {
        self(submission, cancel_token, emit)
    }
}
