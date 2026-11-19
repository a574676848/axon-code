//! 后台执行的投影：逐字稿落执行自己的会话。
//!
//! 一次后台执行落的就是 work item 绑定的那条会话：目标执行落目标会话（`kind=goal`），
//! cron 落任务会话（`kind=cron`）。它不派生子会话，也不另建一条逐字稿载体，
//! 因此同一轮的所有事实只有一个落点，界面用与会话完全相同的投影即可渲染。
//!
//! 目标会话不展示在会话列表里，入口只有目标看板；这是 `kind` 决定的可见性，
//! 与投影口径无关。
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
            clock: self.clock.clone(),
            publisher: self.publisher.clone(),
        }))
    }
}

/// 把运行侧派生的子会话 sink 重定向到执行会话。
struct ExecutionSessionSinkFactory<S, C, P> {
    store: S,
    host: DaemonHostRecord,
    clock: C,
    publisher: P,
}

impl<S, C, P> SubagentSessionSinkFactory for ExecutionSessionSinkFactory<S, C, P>
where
    S: RuntimeStore + Clone + Send + Sync + 'static,
    C: RuntimeClock + Clone + Send + Sync + 'static,
    P: EventPublisher + Clone + Send + Sync + 'static,
{
    fn sink_for(
        &self,
        identity: &axon_session_runtime::SubagentSessionIdentity,
    ) -> Result<Arc<dyn TurnEventSink>, String> {
        // 逐字稿落 turn 所属子会话：执行会话只保留轮次状态与收口标记。
        // 子会话行必须在这里登记，否则事件没有地方推进序号水位。
        let session_id = DaemonSessionId::new(identity.session_id.as_str())
            .map_err(|error| format!("执行子会话标识非法：{error:?}"))?;
        super::interaction_turn_runner::child_session_event_sink::ensure_subagent_session(
            &self.store,
            self.clock.now_ms(),
            identity,
        )?;
        Ok(Arc::new(ExecutionSessionSink {
            store: self.store.clone(),
            host: self.host.clone(),
            session_id,
            clock: self.clock.clone(),
            publisher: self.publisher.clone(),
            // 子会话的收口标记与执行会话无关：共用会让运行侧一写子会话终态，
            // Worker 就误判「执行会话已收口」而跳过来源账本的终态卡。
            terminal_written: Arc::new(AtomicBool::new(false)),
        }))
    }

    fn failures_settled_by_scheduler(&self) -> bool {
        // 逐字稿与终态都落 turn 所属子会话：子会话的失败卡必须由运行侧写，否则它的历史
        // 会在失败轮悬空。执行会话（来源账本）的调度终态 `turn_failed` / `turn_retry_scheduled`
        // 仍由 Worker 写，两条会话各有一张卡，不再互相覆盖。
        false
    }
}

#[cfg(test)]
#[path = "execution_session_sink_tests.rs"]
mod tests;
