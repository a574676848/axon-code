//! JARVIS 主 turn 的 Interaction 消费应用服务。

use axon_domain::{InteractionRecord, InteractionStatus};
use axon_session::{InteractionProcessingLease, InteractionStore, InteractionStoreError};
use axon_session_runtime::{JarvisRuntimeProfile, JarvisRuntimeSelector, RuntimeClock};
use std::{
    marker::PhantomData,
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use thiserror::Error;

use super::EventStreamBridge;
use super::daemon_host::primary_session_id_for_interaction;
use super::primary_turn_lanes::{
    LaneReportWait, await_lane_report, group_lanes, merge_lane_report,
};
use super::primary_turn_worker_pool::{PrimaryTurnWorkerPool, PrimaryTurnWorkerPoolSlot};

const LANE_PANIC_ERROR: &str = "Primary turn lane worker 异常退出";
const LEASE_RENEWAL_THREAD_NAME: &str = "primary-turn-lease-renewal";
/// lease 剩余一半时续租，为 supervisor 留出完整故障判定窗口。
const LEASE_RENEWAL_INTERVAL_DIVISOR: u64 = 2;
const LEASE_RENEWAL_RETRY_LIMIT: u32 = 3;
const LEASE_RENEWAL_RETRY_DELAY: Duration = Duration::from_millis(5);
/// 批内补位轮询间隔。
///
/// 协调线程在同一批里只等待「任意一个 lane」结束，并在有空闲槽位时重新领取，因此
/// 空闲 lane 的新消息不必等待本批最慢 lane。没有空闲槽位时不做轮询。
const LANE_REPLENISH_INTERVAL: Duration = Duration::from_millis(250);

/// 交给 Agent runtime 的主 turn 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryTurnRequest {
    /// 已持久化的 Interaction。
    pub interaction: InteractionRecord,
    /// 本次 claim 的 fencing token。
    pub lease_token: String,
    /// 应用运行时选择的统一人格 profile。
    pub runtime_profile: JarvisRuntimeProfile,
}

/// 主 turn runner 错误所属的应用层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryTurnErrorLayer {
    /// Provider 请求或协议失败。
    Provider,
    /// Channel renderer 或输出映射失败。
    Renderer,
    /// 其他 Agent/runtime 编排失败。
    Runtime,
}

/// 主 turn runner 的结构化错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimaryTurnRunnerError {
    /// Provider 请求或协议失败。
    Provider(String),
    /// 用户主动中断：终态是 interrupted，不是 rejected。
    Interrupted(String),
    /// Channel renderer 或输出映射失败。
    Renderer(String),
    /// 其他 Agent/runtime 编排失败。
    Runtime(String),
}

impl PrimaryTurnRunnerError {
    fn layer(&self) -> PrimaryTurnErrorLayer {
        match self {
            // 中断发生在 Provider 执行期间，归到同一层，失败回告口径不变。
            Self::Provider(_) | Self::Interrupted(_) => PrimaryTurnErrorLayer::Provider,
            Self::Renderer(_) => PrimaryTurnErrorLayer::Renderer,
            Self::Runtime(_) => PrimaryTurnErrorLayer::Runtime,
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Provider(message)
            | Self::Interrupted(message)
            | Self::Renderer(message)
            | Self::Runtime(message) => message,
        }
    }

    /// 本次终态是否由用户主动中断引起。
    #[must_use]
    pub const fn is_interrupted(&self) -> bool {
        matches!(self, Self::Interrupted(_))
    }
}

/// 可回告给上层渠道的主 turn 失败回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryTurnFailure {
    /// 失败 Interaction。
    pub interaction_id: axon_domain::InteractionId,
    /// 失败所在应用层。
    pub layer: PrimaryTurnErrorLayer,
    /// 稳定、可展示的错误描述。
    pub message: String,
    /// 来源渠道；组合根据此判断是否需要向渠道补发失败回执。
    pub channel: String,
    /// 渠道投递目标（Feishu 为 chat_id）。
    pub destination: String,
    /// 可回帖的入站消息幂等键（Feishu 形如 `chat_id:message_id`）。
    pub source_message_id: String,
}

/// 主 turn runner 的 transport-neutral 端口。
///
/// 组合根应在此端口内装配 Provider 请求并调用 Agent runtime；Daemon
/// 只负责 durable claim、结果状态提交和失败隔离。
pub trait PrimaryTurnRunner: Send + Sync {
    /// 运行一个已 claim 的主 turn。
    fn run(&self, request: PrimaryTurnRequest) -> Result<(), PrimaryTurnRunnerError>;
}

/// 一次主 turn tick 的统计。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrimaryTurnTickReport {
    /// 成功领取的 Interaction 数量。
    pub claimed: u32,
    /// 成功完成理解的数量。
    pub understood: u32,
    /// 被拒绝或运行失败的数量。
    pub rejected: u32,
    /// 被用户主动中断的数量。
    pub interrupted: u32,
    /// 每个失败 Interaction 的分层回执。
    pub failures: Vec<PrimaryTurnFailure>,
}

/// 主 turn 消费失败。
#[derive(Debug, Error)]
pub enum PrimaryTurnServiceError {
    /// Interaction Store 失败。
    #[error(transparent)]
    Store(#[from] InteractionStoreError),
    /// 主 turn runner 失败。
    #[error("Primary turn 执行失败：{0}")]
    Runner(String),
}

/// 领取 Interaction 并逐个运行主 turn。
#[derive(Debug, Clone)]
pub struct PrimaryTurnService<S, R, C> {
    store: S,
    runner: R,
    clock: C,
    lease_duration_ms: u64,
    worker_pool: PrimaryTurnWorkerPoolSlot,
    stream_bridge: Arc<EventStreamBridge>,
}

struct InteractionLeaseRenewalGuard<S, C> {
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
    errors: mpsc::Receiver<PrimaryTurnServiceError>,
    _types: PhantomData<fn() -> (S, C)>,
}

impl<S, C> InteractionLeaseRenewalGuard<S, C> {
    fn take_error(&mut self) -> Option<PrimaryTurnServiceError> {
        self.errors.try_recv().ok()
    }
}

impl<S, C> Drop for InteractionLeaseRenewalGuard<S, C> {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl<S, R, C> PrimaryTurnService<S, R, C>
where
    S: InteractionStore + Clone + Send + Sync + 'static,
    R: PrimaryTurnRunner,
    C: RuntimeClock + Clone + Send + Sync + 'static,
{
    /// 创建主 turn 服务。
    pub fn new(
        store: S,
        runner: R,
        clock: C,
        lease_duration_ms: u64,
    ) -> Result<Self, PrimaryTurnServiceError> {
        Self::new_with_worker_pool(
            store,
            runner,
            clock,
            lease_duration_ms,
            PrimaryTurnWorkerPoolSlot::default(),
            Arc::new(EventStreamBridge::new()),
        )
    }

    pub(super) fn new_with_worker_pool(
        store: S,
        runner: R,
        clock: C,
        lease_duration_ms: u64,
        worker_pool: PrimaryTurnWorkerPoolSlot,
        stream_bridge: Arc<EventStreamBridge>,
    ) -> Result<Self, PrimaryTurnServiceError> {
        if lease_duration_ms == 0 {
            return Err(PrimaryTurnServiceError::Store(
                InteractionStoreError::InvalidState("主 turn lease 必须大于 0".to_owned()),
            ));
        }
        Ok(Self {
            store,
            runner,
            clock,
            lease_duration_ms,
            worker_pool,
            stream_bridge,
        })
    }

    /// 批量领取并运行主 turn。
    pub fn tick(&self, limit: u32) -> Result<PrimaryTurnTickReport, PrimaryTurnServiceError> {
        validate_batch_limit(limit)?;
        let now_ms = self.clock.now_ms();
        let leases = self
            .store
            .claim_processing(limit, now_ms, self.lease_duration_ms)?;
        let mut report = PrimaryTurnTickReport {
            claimed: leases.len() as u32,
            ..PrimaryTurnTickReport::default()
        };
        for lease in leases {
            self.run_claimed(lease, now_ms, &mut report)?;
        }
        Ok(report)
    }

    /// 按复合 lane 并发处理主 turn。
    ///
    /// 同一复合 lane（以及 Global lane）始终保持领取顺序；不同 lane 最多同时运行
    /// `max_concurrency` 个。领取由 Store 按 lane 排他推进，同 lane 的多条排队记录
    /// 作为一条 lane 顺序执行，不会并发进入 Provider。
    ///
    /// 协调线程只等待「任意一个 lane」结束：某个 lane 归还槽位后立即补位领取并投递，
    /// 所以最慢 lane 不再阻塞其他空闲 lane 的新消息；只有槽位全满时才等待 lane 结束。
    /// 本批结束前不会返回，保证本轮报告覆盖全部已投递 lane。
    pub fn tick_concurrent(
        &self,
        limit: u32,
        max_concurrency: usize,
    ) -> Result<PrimaryTurnTickReport, PrimaryTurnServiceError>
    where
        S: Clone + Send + Sync + 'static,
        R: Clone + Send + Sync + 'static,
        C: Clone + Send + Sync + 'static,
    {
        validate_batch_limit(limit)?;
        validate_max_concurrency(max_concurrency)?;
        let pool = self
            .worker_pool
            .get_or_create(max_concurrency)
            .map_err(PrimaryTurnServiceError::Runner)?;
        let (sender, receiver) = mpsc::channel();
        let mut report = PrimaryTurnTickReport::default();
        let mut in_flight = 0_usize;
        loop {
            // 只要有空闲槽位就补位；领取为空说明当前没有可领取的工作。
            while in_flight < max_concurrency {
                let capacity = claim_capacity(max_concurrency - in_flight, limit);
                let leases = self.store.claim_processing(
                    capacity,
                    self.clock.now_ms(),
                    self.lease_duration_ms,
                )?;
                if leases.is_empty() {
                    break;
                }
                for lane in group_lanes(leases).into_values() {
                    self.dispatch_lane(&pool, lane, sender.clone())?;
                    in_flight = in_flight.saturating_add(1);
                }
            }
            if in_flight == 0 {
                return Ok(report);
            }
            // 槽位未满时用有界窗口等待：窗口到期后重新领取空闲 lane 的新消息。
            let idle_window = (in_flight < max_concurrency).then_some(LANE_REPLENISH_INTERVAL);
            match await_lane_report(&receiver, idle_window)? {
                LaneReportWait::Report(lane_report) => {
                    in_flight = in_flight.saturating_sub(1);
                    merge_lane_report(&mut report, lane_report?);
                }
                LaneReportWait::Idle => {}
            }
        }
    }

    /// 把一条 lane 投递到有界 worker pool。
    ///
    /// 投递量受空闲槽位约束，池内排队深度不会超过 worker 数，因此这里不会阻塞协调线程。
    fn dispatch_lane(
        &self,
        pool: &PrimaryTurnWorkerPool,
        lane: Vec<InteractionProcessingLease>,
        sender: mpsc::Sender<Result<PrimaryTurnTickReport, PrimaryTurnServiceError>>,
    ) -> Result<(), PrimaryTurnServiceError>
    where
        R: Clone + Send + Sync + 'static,
    {
        let service = self.clone();
        pool.execute(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| service.run_lane(lane)))
                    .map_err(|_| PrimaryTurnServiceError::Runner(LANE_PANIC_ERROR.to_owned()))
                    .and_then(|result| result);
            let _ = sender.send(result);
        })
        .map_err(PrimaryTurnServiceError::Runner)
    }

    fn run_lane(
        &self,
        leases: Vec<InteractionProcessingLease>,
    ) -> Result<PrimaryTurnTickReport, PrimaryTurnServiceError> {
        let mut report = PrimaryTurnTickReport {
            claimed: leases.len() as u32,
            ..PrimaryTurnTickReport::default()
        };
        for lease in leases {
            self.run_claimed(lease, self.clock.now_ms(), &mut report)?;
        }
        Ok(report)
    }

    fn run_claimed(
        &self,
        lease: InteractionProcessingLease,
        now_ms: u64,
        report: &mut PrimaryTurnTickReport,
    ) -> Result<(), PrimaryTurnServiceError> {
        let interaction_id = lease.interaction.interaction_id.clone();
        // 失败回执必须携带来源渠道路由证据，否则组合根无法把拒绝原因回告给用户。
        let channel = lease.interaction.source.channel().to_owned();
        let destination = lease.interaction.reply_route.destination().to_owned();
        let source_message_id = lease.interaction.source.source_message_id().to_owned();
        let session_id = primary_session_id_for_interaction(&lease.interaction)
            .map_err(PrimaryTurnServiceError::Runner)?;
        let request = PrimaryTurnRequest {
            interaction: lease.interaction,
            lease_token: lease.lease_token.clone(),
            runtime_profile: JarvisRuntimeSelector::for_primary_turn(),
        };
        let mut renewal = self.start_lease_renewal(&interaction_id, &lease.lease_token)?;
        let runner_result = self.runner.run(request);
        let renewal_error = renewal.take_error();
        drop(renewal);
        if let Some(error) = renewal_error {
            return Err(error);
        }
        match runner_result {
            Ok(()) => {
                self.store.mark_understood(
                    &interaction_id,
                    &lease.lease_token,
                    self.clock.now_ms().max(now_ms),
                )?;
                self.stream_bridge.publish_interaction_status(
                    &session_id,
                    &interaction_id,
                    InteractionStatus::Understood,
                );
                report.understood = report.understood.saturating_add(1);
            }
            Err(error) => {
                // 用户主动中断与执行失败是两种终态，不能都记成 rejected。
                let interrupted = error.is_interrupted();
                if interrupted {
                    self.store.mark_interrupted(
                        &interaction_id,
                        &lease.lease_token,
                        self.clock.now_ms().max(now_ms),
                    )?;
                    self.stream_bridge.publish_interaction_status(
                        &session_id,
                        &interaction_id,
                        InteractionStatus::Interrupted,
                    );
                    report.interrupted = report.interrupted.saturating_add(1);
                } else {
                    self.store.mark_rejected(
                        &interaction_id,
                        &lease.lease_token,
                        self.clock.now_ms().max(now_ms),
                    )?;
                    self.stream_bridge.publish_interaction_status(
                        &session_id,
                        &interaction_id,
                        InteractionStatus::Rejected,
                    );
                    report.rejected = report.rejected.saturating_add(1);
                }
                report.failures.push(PrimaryTurnFailure {
                    interaction_id,
                    layer: error.layer(),
                    message: non_empty_error_message(error.message()),
                    channel,
                    destination,
                    source_message_id,
                });
            }
        }
        Ok(())
    }

    fn start_lease_renewal(
        &self,
        interaction_id: &axon_domain::InteractionId,
        lease_token: &str,
    ) -> Result<InteractionLeaseRenewalGuard<S, C>, PrimaryTurnServiceError> {
        let renewal_interval =
            Duration::from_millis(self.lease_duration_ms / LEASE_RENEWAL_INTERVAL_DIVISOR);
        let (stop, stop_receiver) = mpsc::channel();
        let (error_sender, error_receiver) = mpsc::channel();
        let store = self.store.clone();
        let clock = self.clock.clone();
        let interaction_id = interaction_id.clone();
        let lease_token = lease_token.to_owned();
        let lease_duration_ms = self.lease_duration_ms;
        let worker = thread::Builder::new()
            .name(LEASE_RENEWAL_THREAD_NAME.to_owned())
            .spawn(move || {
                loop {
                    match stop_receiver.recv_timeout(renewal_interval) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    let mut renewal_error = None;
                    for retry in 0..=LEASE_RENEWAL_RETRY_LIMIT {
                        match store.renew_processing(
                            &interaction_id,
                            &lease_token,
                            clock.now_ms(),
                            lease_duration_ms,
                        ) {
                            Ok(_) => break,
                            Err(InteractionStoreError::Storage(_error))
                                if retry < LEASE_RENEWAL_RETRY_LIMIT =>
                            {
                                thread::sleep(LEASE_RENEWAL_RETRY_DELAY);
                            }
                            Err(error) => {
                                renewal_error = Some(error);
                                break;
                            }
                        }
                    }
                    if let Some(error) = renewal_error {
                        let _ = error_sender.send(PrimaryTurnServiceError::Store(error));
                        return;
                    }
                }
            })
            .map_err(|error| PrimaryTurnServiceError::Runner(error.to_string()))?;
        Ok(InteractionLeaseRenewalGuard {
            stop,
            worker: Some(worker),
            errors: error_receiver,
            _types: PhantomData,
        })
    }
}

fn validate_batch_limit(limit: u32) -> Result<(), PrimaryTurnServiceError> {
    if limit == 0 {
        return Err(PrimaryTurnServiceError::Store(
            InteractionStoreError::InvalidState("主 turn batch limit 必须大于 0".to_owned()),
        ));
    }
    Ok(())
}

fn validate_max_concurrency(max_concurrency: usize) -> Result<(), PrimaryTurnServiceError> {
    if max_concurrency == 0 {
        return Err(PrimaryTurnServiceError::Store(
            InteractionStoreError::InvalidState("Primary turn 最大并发 lane 必须大于 0".to_owned()),
        ));
    }
    Ok(())
}

/// 单次领取上限：空闲 lane 槽位与调用方批上限的较小值。
///
/// 领取量不超过空闲槽位，lane 数就不会超过槽位，因此投递无需归还 lease。
fn claim_capacity(free_slots: usize, limit: u32) -> u32 {
    u32::try_from(free_slots).map_or(limit, |free| free.min(limit))
}

fn non_empty_error_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        "主 turn runner 返回空错误".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
#[path = "primary_turn_service_tests.rs"]
mod tests;
