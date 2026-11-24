//! 通用 Session Host 与 attach snapshot 协调。

use super::egress_bridge::CommittedEventEgress;
use super::event_stream_bridge::{EventStreamBridge, EventStreamSubscription};
use super::event_stream_bridge_telemetry::EventStreamBridgeTelemetrySnapshot;
use super::interaction_ingress::{
    ActiveTurnSteerSink, InteractionDelivery, NoActiveTurnSteerSink, PrimaryTurnIngress,
};
use super::primary_turn_worker_pool::PrimaryTurnWorkerPoolSlot;
use super::scheduler_service::{SchedulerService, SchedulerTickReport};
use super::silent_turn_settlement::SilentTurnSettlementService;
use crate::model::{DaemonHostStatus, UpgradeAdmission};
use crate::ports::{DeliveryHealthStore, ProcessIdentity};
use axon_domain::{DaemonSessionId, InteractionId, InteractionStatus, RuntimeHostId};
use axon_protocol::{SubmitInteractionReceipt, SubmitInteractionRequest};
use axon_session::{
    DaemonHostRecord, InteractionStore, RuntimeStore, RuntimeStoreError, SessionRuntimeRecord,
};
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    thread,
};
use uuid::Uuid;

#[path = "daemon_host_error.rs"]
mod daemon_host_error;
pub use daemon_host_error::DaemonHostError;
#[path = "daemon_host_tick_report.rs"]
mod daemon_host_tick_report;
pub use daemon_host_tick_report::HostTickReport;
#[path = "daemon_host_lease_lost.rs"]
mod daemon_host_lease_lost;

#[cfg(test)]
#[path = "daemon_host_capacity_tests.rs"]
mod daemon_host_capacity_tests;
#[cfg(test)]
#[path = "daemon_host_fault_tests.rs"]
mod daemon_host_fault_tests;
#[cfg(test)]
#[path = "daemon_host_goal_interaction_tests.rs"]
mod daemon_host_goal_interaction_tests;
#[cfg(test)]
#[path = "daemon_host_health_tests.rs"]
mod daemon_host_health_tests;
#[cfg(test)]
#[path = "daemon_host_interaction_tests.rs"]
mod daemon_host_interaction_tests;
#[cfg(test)]
#[path = "daemon_host_schedule_tests.rs"]
mod daemon_host_schedule_tests;
#[cfg(test)]
#[path = "daemon_host_silent_turn_tests.rs"]
mod daemon_host_silent_turn_tests;
#[cfg(test)]
#[path = "daemon_host_soak_tests.rs"]
mod daemon_host_soak_tests;

#[path = "completion_delivery.rs"]
mod completion_delivery;
#[path = "daemon_host_interrupted_projection.rs"]
mod daemon_host_interrupted_projection;
#[path = "daemon_host_projection.rs"]
mod daemon_host_projection;
#[path = "daemon_host_status.rs"]
mod daemon_host_status;
pub use daemon_host_projection::{SessionAttachSnapshot, queued_action};
#[path = "daemon_health.rs"]
mod daemon_health;
#[path = "daemon_host_execution_runtime.rs"]
mod daemon_host_execution_runtime;
#[path = "daemon_host_goal_execution_interaction.rs"]
mod daemon_host_goal_execution_interaction;
#[path = "daemon_host_goal_inbox.rs"]
mod daemon_host_goal_inbox;
#[path = "daemon_host_goal_tools.rs"]
mod daemon_host_goal_tools;
#[path = "daemon_host_goal_tools_ledger.rs"]
mod daemon_host_goal_tools_ledger;
#[path = "daemon_host_request.rs"]
mod daemon_host_request;
#[path = "daemon_host_schedule.rs"]
mod daemon_host_schedule;
#[path = "goal_execution.rs"]
mod goal_execution;
#[path = "interaction_turn_runner.rs"]
mod interaction_turn_runner;
use interaction_turn_runner::InteractionTurnRunner;
pub use interaction_turn_runner::{PRIMARY_TURN_ACTION_KIND, primary_session_id_for_interaction};

const UPGRADE_GRACE_PERIOD_MS: u64 = 300_000;
/// 维护批量事务的收敛上限。
///
/// 中断收敛、`recovering` 结算与静默轮次收口各自在**一个写事务里**逐条结算，单次持锁
/// 时间随批量线性增长：64 条每条约 2 到 3 条语句，一次就能把运行态库的单写者锁握住
/// 数百毫秒，与逐帧写入共用同一个等待预算，突发时把对方挤成 `database is locked`。
/// 降到 16 后单次持锁有上界，积压仍在几个 tick 内排空。
const MAINTENANCE_BATCH_LIMIT: u32 = 16;
/// 会话停留在 `recovering` 的容忍时长。
///
/// supervisor 每轮都会重新接管失控会话，正常运行下 `recovering` 只存在一个 tick；
/// 两分钟仍未派发说明这一轮已经没有执行者，必须收口为中断而不是继续挂着。
const RECOVERING_SETTLEMENT_TTL_MS: u64 = 120_000;
/// 尚未领取 lease 时的租约到期哨兵值。
const NO_LEASE_MS: u64 = 0;
pub(super) const DEFAULT_QUEUE_LIMIT: u32 = 100;
const DEFAULT_SCHEDULE_LEASE_DURATION_MS: u64 = 30_000;
const PRIMARY_TURN_MAX_CONCURRENCY: usize = 4;
use daemon_host_status::{decode as host_status_from_code, encode as host_status_code};

/// 通用 Daemon Host。
#[derive(Clone)]
pub struct DaemonHost<S, E, P, C, I> {
    store: S,
    executor: E,
    publisher: P,
    clock: C,
    process: I,
    host_id: RuntimeHostId,
    host: Option<DaemonHostRecord>,
    status: Arc<AtomicU8>,
    host_lease_duration_ms: u64,
    action_lease_duration_ms: u64,
    primary_turn_worker_pool: PrimaryTurnWorkerPoolSlot,
    stream_bridge: Arc<EventStreamBridge>,
    active_turn_steer_sink: Arc<dyn ActiveTurnSteerSink>,
    delivery_health: Option<Arc<dyn DeliveryHealthStore>>,
    /// 组合根提供的远端通知目标；所有 Host 快照共享最新配置。
    /// 最近一次成功写入的 Host lease 到期时间；健康快照据此报告真实剩余租期。
    lease_expires_at_ms: Arc<AtomicU64>,
    /// 主 turn 租约丢失时请运行侧收口；组合根不接线时为空。
    lease_lost: Option<super::primary_turn_service::LeaseLostHook>,
}

/// Gateway infrastructure 使用的不可变 Host 依赖快照。
#[derive(Clone)]
pub struct GatewayHostDependencies<S, C> {
    /// 既有 RuntimeStore，同时实现 InteractionStore。
    pub store: S,
    /// 既有 active turn steer 端口。
    pub active_turn_steer_sink: Arc<dyn ActiveTurnSteerSink>,
    /// Daemon 时钟。
    pub clock: C,
    /// Interaction processing lease 时长。
    pub action_lease_duration_ms: u64,
    /// 既有实时事件桥。
    pub stream_bridge: Arc<EventStreamBridge>,
    /// 当前 Host generation；Host 尚未启动时为空。
    pub host_generation: Option<String>,
}

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 创建未启动 Host。
    pub fn new(
        store: S,
        executor: E,
        publisher: P,
        clock: C,
        process: I,
        host_id: RuntimeHostId,
        host_lease_duration_ms: u64,
        action_lease_duration_ms: u64,
    ) -> Self {
        Self::new_with_stream_bridge(
            store,
            executor,
            publisher,
            clock,
            process,
            host_id,
            host_lease_duration_ms,
            action_lease_duration_ms,
            Arc::new(EventStreamBridge::new()),
        )
    }

    /// 创建带实时事件桥接的 Host。
    pub fn new_with_stream_bridge(
        store: S,
        executor: E,
        publisher: P,
        clock: C,
        process: I,
        host_id: RuntimeHostId,
        host_lease_duration_ms: u64,
        action_lease_duration_ms: u64,
        stream_bridge: Arc<EventStreamBridge>,
    ) -> Self {
        Self {
            store,
            executor,
            publisher,
            clock,
            process,
            host_id,
            host: None,
            status: Arc::new(AtomicU8::new(host_status_code(DaemonHostStatus::Stopped))),
            host_lease_duration_ms,
            action_lease_duration_ms,
            primary_turn_worker_pool: PrimaryTurnWorkerPoolSlot::default(),
            stream_bridge,
            active_turn_steer_sink: Arc::new(NoActiveTurnSteerSink),
            delivery_health: None,
            lease_expires_at_ms: Arc::new(AtomicU64::new(NO_LEASE_MS)),
            lease_lost: None,
        }
    }

    /// 返回当前 EventStreamBridge 的 trace-only 聚合指标。
    pub fn stream_bridge_telemetry_snapshot(&self) -> EventStreamBridgeTelemetrySnapshot {
        self.stream_bridge.telemetry_snapshot()
    }
    /// 注入由组合根持有的活跃主 turn steer 端口。
    #[must_use]
    pub fn with_active_turn_steer_sink<K>(mut self, steer_sink: K) -> Self
    where
        K: ActiveTurnSteerSink + 'static,
    {
        self.active_turn_steer_sink = Arc::new(steer_sink);
        self
    }
    /// 注入由组合根持有的渠道投递健康端口。
    #[must_use]
    pub fn with_delivery_health_store<K>(mut self, delivery_health: K) -> Self
    where
        K: DeliveryHealthStore + 'static,
    {
        self.delivery_health = Some(Arc::new(delivery_health));
        self
    }
    /// 获取 Host 级实时事件桥接器，供 TUI、CLI 与桌面端订阅。
    #[must_use]
    pub fn stream_bridge(&self) -> Arc<EventStreamBridge> {
        Arc::clone(&self.stream_bridge)
    }
    /// 获取 Gateway bridge 所需的既有 Host 依赖，不创建新的运行时状态。
    #[must_use]
    pub fn gateway_dependencies(&self) -> GatewayHostDependencies<S, C> {
        GatewayHostDependencies {
            store: self.store.clone(),
            active_turn_steer_sink: Arc::clone(&self.active_turn_steer_sink),
            clock: self.clock.clone(),
            action_lease_duration_ms: self.action_lease_duration_ms,
            stream_bridge: Arc::clone(&self.stream_bridge),
            host_generation: self.host.as_ref().map(|host| host.generation.clone()),
        }
    }

    pub(super) fn store_clone(&self) -> S {
        self.store.clone()
    }
    /// 返回当前 Host generation，供订阅适配构造 ready replay cursor。
    pub(super) fn current_host_generation(&self) -> Result<String, DaemonHostError> {
        Ok(self.started_host()?.generation.clone())
    }

    /// 获取当前 Host generation 下 Session 的 durable 尾游标。
    pub(super) fn live_event_cursor(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<axon_domain::ConsumerCursor, DaemonHostError> {
        let generation = self.started_host()?.generation.clone();
        let sequence = match self.store.get_session(session_id) {
            Ok(runtime) => runtime.last_event_sequence,
            Err(RuntimeStoreError::NotFound(_)) => 0,
            Err(error) => return Err(error.into()),
        };
        Ok(axon_domain::ConsumerCursor::new(generation, sequence))
    }

    /// 订阅 Session 的实时事件；重连前应先通过 `attach` replay durable cursor。
    pub fn subscribe_events(
        &self,
        session_id: DaemonSessionId,
        interaction_id: InteractionId,
        after_cursor: Option<axon_domain::ConsumerCursor>,
    ) -> Result<EventStreamSubscription, DaemonHostError> {
        self.stream_bridge
            .subscribe(session_id, interaction_id, after_cursor)
            .map_err(|error| DaemonHostError::Worker(format!("{error:?}")))
    }

    /// 领取唯一 Host lease。
    pub fn start(&mut self) -> Result<(), DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let host = self.store.claim_host(
            &self.host_id,
            self.process.pid(),
            self.process.process_start_id().as_deref(),
            now_ms,
            self.host_lease_duration_ms,
        )?;
        self.lease_expires_at_ms
            .store(host.lease_expires_at_ms, Ordering::Release);
        self.host = Some(host);
        let interrupted = self.store.quarantine_interrupted_interactions(
            self.started_host()?,
            self.clock.now_ms(),
            MAINTENANCE_BATCH_LIMIT,
        )?;
        daemon_host_interrupted_projection::project_interrupted_interactions(
            &self.publisher,
            &self.stream_bridge,
            &interrupted,
        );
        self.status.store(
            host_status_code(DaemonHostStatus::Running),
            Ordering::Release,
        );
        Ok(())
    }

    /// 续租，失败时调用方必须停止所有 Worker。
    pub fn renew(&self) -> Result<(), DaemonHostError> {
        let now_ms = self.clock.now_ms();
        self.store
            .renew_host(self.started_host()?, now_ms, self.host_lease_duration_ms)?;
        self.lease_expires_at_ms.store(
            now_ms.saturating_add(self.host_lease_duration_ms),
            Ordering::Release,
        );
        Ok(())
    }

    /// 返回当前 host generation 的稳定投递 owner identity。
    pub fn owner_identity(&self) -> Result<String, DaemonHostError> {
        let host = self.started_host()?;
        Ok(format!("{}:{}:{}", host.host_id, host.generation, host.pid))
    }

    /// 当前 host 记录。
    ///
    /// 需要以本进程身份追加 durable 事件的调用方用它过代际围栏（`append_events`）；
    /// 事件落盘必须与宿主同一代，否则会被围栏拒绝。
    pub fn host_record(&self) -> Result<DaemonHostRecord, DaemonHostError> {
        Ok(self.started_host()?.clone())
    }

    /// 宿主自己的事实源存储。
    ///
    /// 只给宿主的维护动作使用：维护动作必须与业务写共用同一份连接池，
    /// 不能在进程内另建一份连接去竞争同一个写锁。
    pub fn store(&self) -> &S {
        &self.store
    }

    /// 创建可持久恢复的 Session。
    pub fn create_session(
        &self,
        session_id: DaemonSessionId,
        cwd: &str,
    ) -> Result<SessionRuntimeRecord, DaemonHostError> {
        let _ = self.started_host()?;
        self.store
            .create_session(&session_id, cwd, self.clock.now_ms())
            .map_err(Into::into)
    }

    /// 校验并持久化统一自然语言 Interaction。
    pub fn submit_interaction(
        &self,
        request: SubmitInteractionRequest,
    ) -> Result<SubmitInteractionReceipt, DaemonHostError>
    where
        S: InteractionStore,
    {
        if self.status() != DaemonHostStatus::Running {
            return Err(DaemonHostError::Draining);
        }
        let _ = self.started_host()?;
        // 必须「先持久化，再尝试 steer」：steer 命中时同样要落 durable Interaction，
        // 否则活跃 turn 尾部接纳的消息没有 durable 载体，进程崩溃即永久丢失。
        let receipt = PrimaryTurnIngress::new(
            self.store.clone(),
            Arc::clone(&self.active_turn_steer_sink),
            self.clock.now_ms(),
            self.action_lease_duration_ms,
        )
        .submit_new_interaction(&request.interaction)
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        if receipt.delivery == InteractionDelivery::Steered {
            let session_id = primary_session_id_for_interaction(&request.interaction)
                .map_err(DaemonHostError::Worker)?;
            self.stream_bridge.publish_interaction_status(
                &session_id,
                &receipt.interaction_id,
                InteractionStatus::Understood,
            );
        }
        Ok(SubmitInteractionReceipt {
            interaction_id: receipt.interaction_id,
            accepted: true,
            deduplicated: !receipt.inserted,
        })
    }

    /// 在独立协调线程中领取并运行 Primary Turn。
    pub fn primary_turn_tick(
        &self,
        limit: u32,
    ) -> Result<super::primary_turn_service::PrimaryTurnTickReport, DaemonHostError> {
        if self.status() != DaemonHostStatus::Running {
            return Ok(Default::default());
        }
        let host = self.started_host()?.clone();
        let now_ms = self.clock.now_ms();
        let mut service = super::primary_turn_service::PrimaryTurnService::new_with_worker_pool(
            self.store.clone(),
            InteractionTurnRunner {
                store: self.store.clone(),
                executor: self.executor.clone(),
                publisher: self.publisher.clone(),
                stream_bridge: Arc::clone(&self.stream_bridge),
                host,
                clock: self.clock.clone(),
                now_ms,
            },
            self.clock.clone(),
            self.action_lease_duration_ms,
            self.primary_turn_worker_pool.clone(),
            Arc::clone(&self.stream_bridge),
        )
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        if let Some(lease_lost) = self.lease_lost.clone() {
            service = service.with_lease_lost(lease_lost);
        }
        service
            .tick_concurrent(limit, PRIMARY_TURN_MAX_CONCURRENCY)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))
    }

    /// 恢复租约失效的 Session，并直接按 `work_items` 驱动执行 Worker。
    pub fn supervisor_tick(&self, limit: u32) -> Result<HostTickReport, DaemonHostError> {
        let host = self.started_host()?.clone();
        let now_ms = self.clock.now_ms();
        let accepting_work = self.status() == DaemonHostStatus::Running;
        let recovered = if accepting_work {
            self.store.recover_expired_sessions(&host, now_ms, limit)?
        } else {
            Vec::new()
        };
        // 进程中断的执行必须在任何重跑之前收敛为 uncertain。
        let quarantined = self
            .store
            .quarantine_interrupted_executions(&host, now_ms, limit)?;
        for record in &quarantined {
            // durable 事件日志是唯一事实来源；实时订阅失败不影响本轮隔离结果。
            let cursor = record.cursor();
            let _ = CommittedEventEgress::new(&self.publisher, self.stream_bridge.as_ref())
                .publish(record, &cursor);
        }
        let interrupted_interactions = self.store.quarantine_interrupted_interactions(
            &host,
            now_ms,
            MAINTENANCE_BATCH_LIMIT,
        )?;
        daemon_host_interrupted_projection::project_interrupted_interactions(
            &self.publisher,
            &self.stream_bridge,
            &interrupted_interactions,
        );
        // `recovering` 必须有归宿：租约失效的会话被接管之后，没有任何环节会把它送回
        // `idle`，也没有人会重新派发。这里在中断收敛之后跑一次收敛，顺序不能颠倒：
        // 先让租约过期的执行收敛成终态，收敛才能看见「已经腾空」的会话并把它放回 `idle`。
        if accepting_work {
            match self.store.settle_recovering_sessions(
                &host,
                now_ms,
                RECOVERING_SETTLEMENT_TTL_MS,
                MAINTENANCE_BATCH_LIMIT,
            ) {
                Ok(settlement) => {
                    daemon_host_interrupted_projection::project_interrupted_interactions(
                        &self.publisher,
                        &self.stream_bridge,
                        &settlement.events,
                    )
                }
                // 收敛失败不影响本轮其它维护：下一轮重试，会话仍然停在 `recovering`
                // 由下一轮继续处理，不会静默丢失。
                Err(error) => eprintln!("recovering 会话收敛失败，将在下一轮重试: {error}"),
            }
        }
        // 静默轮次必须有归宿：租约仍有效、但首字期限内一个事件帧都没写过的轮次，没有任何
        // 执行者会再来写它的终态。这里排在两个收敛之后，顺序不能颠倒：
        // 1. 先让租约过期的轮次走隔离路径，那条路径会连同 `interaction_interrupted` 证据帧
        //    一起落盘；静默路径只写终态、不写证据帧，抢在前面会把证据一起吃掉。
        // 2. 再让 `recovering` 会话按「在途工作项」判完归宿，避免轮次先被收口、会话提前腾空。
        let silent_turns_settled = if accepting_work {
            match SilentTurnSettlementService::new(self.store.clone())
                .settle_pending(now_ms, MAINTENANCE_BATCH_LIMIT)
            {
                Ok(report) => {
                    // 排队轮次被判死意味着消息不会再跑了：这是运维需要看见的事实，
                    // 不能只体现在计数上。
                    if report.queued_turns_settled > 0 {
                        eprintln!(
                            "排队轮次超过派发期限仍无人领取，已收口 {} 条",
                            report.queued_turns_settled
                        );
                    }
                    report.turns_settled
                }
                // 失败不影响本轮其它维护：轮次仍在在途态，下一轮重试。
                Err(error) => {
                    eprintln!("静默轮次收口失败，将在下一轮重试: {error}");
                    0
                }
            }
        } else {
            0
        };
        // 目标轮与用户轮共用主 turn 管道：这里只负责把退避到期的执行重新投递成
        // 一条 Interaction，由主 turn 领取执行，不再有后台专用执行入口。
        let pumped = if accepting_work {
            self.redispatch_goal_executions(now_ms, limit)
        } else {
            0
        };
        // 崩溃补偿：commit 成功但通知未落库的 Execution 必须在这里被重新发现，
        // 否则对应 Result 会永久滞留未读。
        if let Err(error) = self.recover_missing_completions(limit) {
            eprintln!("完成通知补偿扫描失败，将在下一轮重试: {error}");
        }
        // 隔天未读自动失效：过期的完成消息静默停止投递，并释放 Result 的保留阻塞。
        if let Err(error) = self.expire_stale_completions(now_ms, limit) {
            eprintln!("完成通知失效扫描失败，将在下一轮重试: {error}");
        }
        Ok(HostTickReport {
            recovered: recovered.len() as u32,
            quarantined: quarantined.len() as u32,
            interrupted_interactions: interrupted_interactions.len() as u32,
            silent_turns_settled,
            pumped,
        })
    }

    /// 执行一轮持久 Scheduler，只向统一 Interaction inbox 提交任务。
    pub fn scheduler_tick(
        &self,
        schedule_lease_duration_ms: u64,
        limit: u32,
    ) -> Result<SchedulerTickReport, DaemonHostError> {
        if self.status() != DaemonHostStatus::Running {
            return Ok(Default::default());
        }
        let host = self.started_host()?;
        SchedulerService::new_with_steer_sink(
            self.store.clone(),
            Arc::clone(&self.active_turn_steer_sink),
            schedule_lease_duration_ms,
        )
        .tick(host, self.clock.now_ms(), limit)
        .map_err(|error| DaemonHostError::Worker(error.to_string()))
    }

    /// 持续运行 Host supervisor；连接生命周期与 Worker 生命周期彼此独立。
    pub fn run_forever<F>(
        &self,
        tick_interval_ms: u64,
        batch_limit: u32,
        mut should_stop: F,
    ) -> Result<(), DaemonHostError>
    where
        F: FnMut() -> bool,
    {
        let interval = std::time::Duration::from_millis(tick_interval_ms);
        while !should_stop() {
            self.renew()?;
            let _ = self.scheduler_tick(DEFAULT_SCHEDULE_LEASE_DURATION_MS, batch_limit)?;
            let _ = self.primary_turn_tick(batch_limit)?;
            let _ = self.supervisor_tick(batch_limit)?;
            thread::sleep(interval);
        }
        Ok(())
    }

    /// 根据 cursor 读取小型 attach 投影；cursor generation 变化时要求重新 snapshot。
    pub fn attach(
        &self,
        session_id: &DaemonSessionId,
        generation: Option<&str>,
        sequence: u64,
        limit: u32,
    ) -> Result<SessionAttachSnapshot, DaemonHostError> {
        let host = self.started_host()?;
        let runtime = self.store.get_session(session_id)?;
        let snapshot_required = generation.is_some_and(|value| value != host.generation);
        let events = self.store.replay_events(
            session_id,
            if snapshot_required { 0 } else { sequence },
            limit,
        )?;
        Ok(SessionAttachSnapshot {
            runtime,
            events,
            snapshot_required,
        })
    }

    /// 开始排空；新的 Action 被拒绝，已 claim Action 保持由 checkpoint/recovery 接管。
    pub fn begin_drain(&mut self) -> Result<(), DaemonHostError> {
        self.request_drain()
    }

    /// 请求进入排空；所有 Host clone 立即观察到该状态。
    pub fn request_drain(&self) -> Result<(), DaemonHostError> {
        let _ = self.started_host()?;
        self.set_status(DaemonHostStatus::AdmittingShutdown);
        Ok(())
    }

    /// 创建升级 admission；此后 Host 不再接收新 Action。
    pub fn request_upgrade(
        &mut self,
        target_version: &str,
        deadline_at_ms: u64,
    ) -> Result<UpgradeAdmission, DaemonHostError> {
        let host = self.started_host()?.clone();
        let handoff_token = Uuid::new_v4().to_string();
        let now_ms = self.clock.now_ms();
        self.store.request_upgrade(
            &host,
            &handoff_token,
            target_version,
            deadline_at_ms,
            now_ms,
        )?;
        self.set_status(DaemonHostStatus::AdmittingShutdown);
        Ok(UpgradeAdmission {
            host_generation: host.generation,
            handoff_token,
            deadline_at_ms,
        })
    }

    /// 在 Worker 已到达安全 checkpoint 后开放 replacement handoff。
    pub fn mark_handoff_ready(
        &mut self,
        admission: &UpgradeAdmission,
    ) -> Result<(), DaemonHostError> {
        let host = self.started_host()?.clone();
        if admission.host_generation != host.generation {
            return Err(DaemonHostError::Draining);
        }
        let now_ms = self.clock.now_ms();
        if now_ms >= admission.deadline_at_ms {
            let _ = self.store.interrupt_host_actions(&host, now_ms)?;
        }
        self.store
            .mark_handoff_ready(&host, &admission.handoff_token, now_ms)?;
        self.set_status(DaemonHostStatus::HandoffReady);
        Ok(())
    }

    /// 使用升级 token 启动 replacement Host；旧 generation 立刻被 fencing。
    pub fn start_replacement(&mut self, handoff_token: &str) -> Result<(), DaemonHostError> {
        let host = self.store.claim_upgrade_replacement(
            &self.host_id,
            self.process.pid(),
            self.process.process_start_id().as_deref(),
            handoff_token,
            self.clock.now_ms(),
            self.host_lease_duration_ms,
        )?;
        self.lease_expires_at_ms
            .store(host.lease_expires_at_ms, Ordering::Release);
        self.host = Some(host);
        self.set_status(DaemonHostStatus::Running);
        Ok(())
    }

    pub(super) fn started_host(&self) -> Result<&DaemonHostRecord, DaemonHostError> {
        self.host.as_ref().ok_or(DaemonHostError::NotStarted)
    }

    fn set_status(&self, status: DaemonHostStatus) {
        self.status
            .store(host_status_code(status), Ordering::Release);
    }
}

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I> {
    fn status(&self) -> DaemonHostStatus {
        host_status_from_code(self.status.load(Ordering::Acquire))
    }
}
