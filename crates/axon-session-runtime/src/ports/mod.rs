//! Session Runtime 的外部端口。

pub mod cancellation;
pub mod observation;
pub mod turn_performance;
pub use cancellation::CancellationSignal;
pub use observation::{ObservationPublisher, ObservationSubscriber};
pub use turn_performance::{
    AtomicTurnPerformanceRecorder, TurnPerformanceRecorder, TurnPerformanceSnapshot,
    TurnPerformanceStage, TurnPerformanceStageTiming, configure_first_token_telemetry,
    first_token_telemetry_enabled,
};

use crate::model::{
    ActionExecutionRequest, RuntimeEvent, TurnExecutionError, TurnExecutionOutcome,
};
use axon_domain::{ConsumerCursor, DaemonSessionId};
use axon_session::SessionEventRecord;

/// 执行 Agent turn 的端口。
pub trait TurnExecutor: Send + Sync {
    /// 执行已 claim 的 Action；失败必须携带结构化类别，禁止文本嗅探。
    fn execute(
        &self,
        request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError>;

    /// 带实时事件通道地执行已 claim 的 Action。
    ///
    /// 支持实时投影的执行器必须在运行过程中把每条运行事件即时送入 `sink`；
    /// 默认实现忽略通道，退化为收口时批量投影，保持既有执行器零改动可用。
    fn execute_streaming(
        &self,
        request: ActionExecutionRequest,
        _sink: Option<&dyn TurnEventSink>,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        self.execute(request)
    }
}

/// Provider turn 运行期间的逐条实时投影通道。
///
/// 通道承担两类事实，语义不同：
/// - 运行过程中的进度可见性：`publish`/`publish_batch` 的失败可由调用方降级为告警，
///   不得因此拒绝整轮 turn；
/// - 收口的 canonical 终止事件（最终 assistant 与 RunLifecycle completed）：
///   由 `AgentTurnExecutor` 单独提交，失败必须终止本轮，禁止在 durable 缺口上继续收口。
pub trait TurnEventSink: Send + Sync {
    /// 即时投影一条已经规范化为 Runtime 事件的事实。
    fn publish(&self, event: &RuntimeEvent) -> Result<(), String>;

    /// 原子提交一组有序 Runtime 事件；默认逐条兼容既有 sink。
    fn publish_batch(&self, events: &[RuntimeEvent]) -> Result<(), String> {
        for event in events {
            self.publish(event)?;
        }
        Ok(())
    }

    /// 返回本 turn 的可选性能 recorder；默认关闭且不产生额外计时。
    fn performance_recorder(&self) -> Option<std::sync::Arc<dyn TurnPerformanceRecorder>> {
        None
    }

    /// 即时发布非持久观测帧；默认忽略，保持向后兼容。
    fn publish_observation(
        &self,
        _frame: &axon_domain::RuntimeObservationFrame,
    ) -> Result<(), String> {
        Ok(())
    }

    /// 返回本轮的子代理会话 sink 工厂；默认不支持子会话持久化。
    ///
    /// 子代理是独立会话，它的逐字稿必须落在**它自己**的会话事件流里，
    /// 这样界面才能用与普通会话完全相同的一条管线去投影与渲染。
    /// 持久化的归属方仍是持锁的宿主：工厂由宿主侧实现，运行侧只负责把
    /// 子代理的原始事件交给它，绝不自行开第二条写入路径。
    fn subagent_session_sinks(&self) -> Option<std::sync::Arc<dyn SubagentSessionSinkFactory>> {
        None
    }
}

/// 为子代理会话提供 durable sink 的工厂。
///
/// 由宿主（持 host 代际与租约的一方）实现：登记子会话运行态行并按其会话标识
/// 追加事件。运行侧拿到 sink 后只做交付，不关心存储细节。
pub trait SubagentSessionSinkFactory: Send + Sync {
    /// 取（必要时先登记）某个子代理会话的 durable sink。
    fn sink_for(&self, session_id: &str) -> Result<std::sync::Arc<dyn TurnEventSink>, String>;
}

/// 发布事件给外部订阅者的端口。
pub trait EventPublisher: Send + Sync {
    /// 发布已经提交到事件日志的事件。
    fn publish(&self, event: &SessionEventRecord, cursor: &ConsumerCursor) -> Result<(), String>;

    /// 发布非持久观测帧（不落盘、不改变游标、允许丢弃）；默认空实现。
    fn publish_observation(
        &self,
        _frame: &axon_domain::RuntimeObservationFrame,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// 可注入时间源，避免持久状态依赖系统时钟细节。
pub trait RuntimeClock: Send + Sync {
    /// 当前 Unix 时间毫秒。
    fn now_ms(&self) -> u64;
}

/// 禁止未使用的导出被错误优化掉。
pub type SessionIdRef = DaemonSessionId;
