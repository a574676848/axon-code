//! 后台执行的会话投影：把子代理逐字稿写进**执行会话本身**。
//!
//! 目标与 cron 的执行会话是 `goal-session-<goalId>`：执行绑定、订阅放行与看板入口
//! 都指向这一条会话。运行侧原本按「派生子会话」写逐字稿（`<parent>--subagent--<path>`），
//! 本模块把落点重定向到执行会话，界面因此不需要知道派生子会话的标识，也不需要
//! 为后台执行另造一套视图。
//!
//! 写入口径与子代理会话完全一致：宿主代际围栏下 `append_events`，落库之后再发布，
//! 订阅者看到的每一帧都已经是 durable 事实。
//!
//! 收口事件同时置位「终态已写」标记：Worker 据此跳过重复追加，避免同一轮被收口两次。

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axon_domain::DaemonSessionId;
use axon_session::{DaemonHostRecord, RuntimeStore};
use axon_session_runtime::{
    EventPublisher, ExecutionProjection, RuntimeClock, RuntimeEvent, SubagentSessionSinkFactory,
    TurnEventSink, model::TURN_COMPLETED_EVENT,
};

/// 装配一条执行会话的投影。
///
/// 返回的投影既是逐条事件的落点，也自带「终态已写」标记；运行侧经工厂拿到的
/// 派生 sink 与它共享同一个会话与标记，因此无论哪一侧写终态都能被 Worker 看见。
pub(super) fn execution_session_projection<S, C, P>(
    store: S,
    host: DaemonHostRecord,
    session_id: DaemonSessionId,
    clock: C,
    publisher: P,
) -> ExecutionProjection
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
    P: EventPublisher + Clone + Send + Sync + 'static,
{
    let terminal_written = Arc::new(AtomicBool::new(false));
    let sink = ExecutionSessionSink {
        store,
        host,
        session_id,
        clock,
        publisher,
        terminal_written: Arc::clone(&terminal_written),
    };
    ExecutionProjection::new(Arc::new(sink), terminal_written)
}

/// 单个后台执行会话的持久 sink。
struct ExecutionSessionSink<S, C, P> {
    store: S,
    host: DaemonHostRecord,
    session_id: DaemonSessionId,
    clock: C,
    publisher: P,
    terminal_written: Arc<AtomicBool>,
}

impl<S, C, P> TurnEventSink for ExecutionSessionSink<S, C, P>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
    P: EventPublisher + Clone + Send + Sync + 'static,
{
    fn publish(&self, event: &RuntimeEvent) -> Result<(), String> {
        let records = self
            .store
            .append_events(
                &self.session_id,
                &self.host,
                std::slice::from_ref(&(event.event_type.clone(), event.payload_json.clone())),
                self.clock.now_ms(),
            )
            .map_err(|error| format!("执行会话事件追加失败：{error}"))?;
        // 落库之后再发布：订阅者看到的每一帧都已经是 durable 事实。
        for record in &records {
            self.publisher
                .publish(record, &record.cursor())
                .map_err(|error| format!("执行会话事件出站失败：{error}"))?;
        }
        if event.event_type == TURN_COMPLETED_EVENT {
            self.terminal_written.store(true, Ordering::Release);
        }
        Ok(())
    }

    fn subagent_session_sinks(&self) -> Option<Arc<dyn SubagentSessionSinkFactory>> {
        Some(Arc::new(ExecutionSessionSinkFactory {
            store: self.store.clone(),
            host: self.host.clone(),
            session_id: self.session_id.clone(),
            clock: self.clock.clone(),
            publisher: self.publisher.clone(),
            terminal_written: Arc::clone(&self.terminal_written),
        }))
    }
}

/// 把运行侧派生的子会话 sink 重定向到执行会话。
struct ExecutionSessionSinkFactory<S, C, P> {
    store: S,
    host: DaemonHostRecord,
    session_id: DaemonSessionId,
    clock: C,
    publisher: P,
    terminal_written: Arc<AtomicBool>,
}

impl<S, C, P> SubagentSessionSinkFactory for ExecutionSessionSinkFactory<S, C, P>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
    P: EventPublisher + Clone + Send + Sync + 'static,
{
    fn sink_for(&self, _session_id: &str) -> Result<Arc<dyn TurnEventSink>, String> {
        // 请求的子会话标识被有意忽略：后台执行的逐字稿只能落在执行会话上，
        // 否则界面要额外知道派生子会话的标识，卡片入口与执行绑定也会分叉。
        Ok(Arc::new(ExecutionSessionSink {
            store: self.store.clone(),
            host: self.host.clone(),
            session_id: self.session_id.clone(),
            clock: self.clock.clone(),
            publisher: self.publisher.clone(),
            terminal_written: Arc::clone(&self.terminal_written),
        }))
    }

    fn failures_settled_by_scheduler(&self) -> bool {
        // 失败终态由 Worker 结算并写 `turn_failed` / `turn_retry_scheduled`：
        // 运行侧再写一条 `turn_completed` 会让同一轮出现两张失败卡。
        true
    }
}
