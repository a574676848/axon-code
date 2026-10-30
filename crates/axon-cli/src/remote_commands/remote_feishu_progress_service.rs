//! 飞书回合级实时进度投影服务。
//!
//! 投影消费 Daemon **已提交**的 Session Event 流，不另建重排或完成门禁：事件按共享
//! durable sequence 到达，本服务只按到达顺序归并、渲染与 PATCH。
//!
//! 一个回合只有**一张**进度卡：正文在前、过程行在后、终态页脚收尾，过程行不提供展开。
//!
//! 硬约束：
//! - `publish` 路径只做**无阻塞入队**；网络写入全部在单后台 worker 内完成，绝不阻塞
//!   provider / coding 主链；
//! - 邮箱有界，正文增量在满载时按最旧优先淘汰（进度是增强视图，可丢帧不可阻塞）；
//! - `Finish` 命令优先出队：回合收口不能被其他 chat 的在队工作量拖延；
//! - 正文与过程行共用**同一个**合并窗口与同一张卡片，是否真的发 PATCH 由渲染指纹决定；
//! - 回合结束时由 `finish` 同步收口：worker 强制 flush 后才返回投影结论，调用方据此
//!   决定是否由 durable outbox 补偿未投影内容（避免同一正文投递两次）；收口等待有
//!   硬上界，超时即让出结算权，改为完全由 durable outbox 投递。

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant},
};

use axon_agent_runtime::AgentReplyAttachmentRuntime;
use axon_remote_feishu::{RemoteFeishuMessageTarget, RemoteFeishuProgressClient};

use super::remote_feishu_progress_mailbox::{ProgressCommand, ProgressMailbox, ProgressSettlement};
use super::remote_feishu_progress_model::{ProgressProjection, ProjectionEvent};
use super::remote_feishu_progress_projection::{
    CardStatus, RenderedCard, render_card, render_progress_body,
};

/// 卡片在两次 PATCH 之间的最小合并窗口。
const PROGRESS_MERGE_WINDOW: Duration = Duration::from_millis(1_500);
/// 进度 PATCH 的单次受限网络超时。
const PROGRESS_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 收口硬上界在两次受限网络调用之上额外留出的结算余量。
const PROGRESS_FINISH_SETTLE_MARGIN: Duration = Duration::from_millis(500);
/// `finish` 等待 worker 收口的硬上界。
///
/// 收口延迟 = 在途网络调用（≤ `PROGRESS_REQUEST_TIMEOUT`，`Finish` 优先出队，结算前
/// 最多只有一个在途调用）+ 最终 PATCH（≤ `PROGRESS_REQUEST_TIMEOUT`）。上界必须覆盖
/// 这两个阶段：偏小会让调用方在卡片即将写成功时判定「未投影」，与 durable outbox 的
/// 补偿回复构成同一正文的重复投递。
const PROGRESS_FINISH_TIMEOUT: Duration = PROGRESS_REQUEST_TIMEOUT
    .saturating_mul(2)
    .saturating_add(PROGRESS_FINISH_SETTLE_MARGIN);
/// 在途 Feishu 主 turn 登记表容量；超出后淘汰最早的登记，避免异常回合永久占用。
const PROGRESS_ACTIVE_TURNS: usize = 64;
/// 已收口回合的短名单容量：同量级上界，用于丢弃收口后仍可能到达的迟到增量。
const PROGRESS_FINISHED_TURNS: usize = PROGRESS_ACTIVE_TURNS;

/// 单次回合的投影结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionOutcome {
    /// 全部可见正文都已由飞书 API 确认投影，且不含附件指令（无需二次投递）。
    FullyProjected,
    /// 存在未确认投影的内容，必须由 durable outbox 补偿。
    Unprojected,
}

/// 回合投影结论：正文是否已完整投影 + 是否仍需要附件投递。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnProjection {
    /// 投影结论。
    pub(crate) outcome: ProjectionOutcome,
    /// 回复是否携带 `MEDIA:` 附件指令（此时正文必须留给投递链路处理附件）。
    pub(crate) has_media_directive: bool,
}

impl TurnProjection {
    /// 未参与投影（功能未启用或不可用）时的结论。
    #[must_use]
    pub(crate) const fn unavailable() -> Self {
        Self {
            outcome: ProjectionOutcome::Unprojected,
            has_media_directive: false,
        }
    }

    /// 是否应抑制 durable outbox 的最终回复（避免同一正文投递两次）。
    #[must_use]
    pub(crate) const fn suppresses_durable_reply(self) -> bool {
        matches!(self.outcome, ProjectionOutcome::FullyProjected) && !self.has_media_directive
    }
}

/// 单次回合的投影身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgressTurn {
    /// 稳定 dispatch 身份；同回合的增量与收口共用。
    pub(crate) dispatch_id: String,
    /// 目标 chat。
    pub(crate) chat_id: String,
    /// 可选回帖目标。
    pub(crate) reply_to_message_id: Option<String>,
}

/// 单次回合的卡片状态。
struct TurnCard {
    card_message_id: Option<String>,
    projection: ProgressProjection,
    started_at: Instant,
    last_patch_at: Option<Instant>,
    /// 最近一次成功 PATCH 的卡片指纹；相同即无需再写。
    projected_fingerprint: Option<u64>,
    /// 已投影账本：卡片承载的正文。
    projected_body: String,
    /// 已投影账本：卡片是否完整承载了正文。
    projected_body_complete: bool,
}

impl TurnCard {
    fn new() -> Self {
        Self {
            card_message_id: None,
            projection: ProgressProjection::default(),
            started_at: Instant::now(),
            last_patch_at: None,
            projected_fingerprint: None,
            projected_body: String::new(),
            projected_body_complete: false,
        }
    }

    /// 归并一条投影事件。
    fn apply(&mut self, event: ProjectionEvent) {
        self.projection.apply(event);
    }

    /// 按当前投影渲染卡片。
    fn render(&self, status: CardStatus) -> RenderedCard {
        render_card(&self.projection, status, self.elapsed_ms())
    }

    /// 网络写入成功后提交账本与指纹。
    fn adopt_ledger(&mut self, rendered: &RenderedCard) {
        self.projected_fingerprint = Some(rendered.fingerprint);
        self.projected_body.clone_from(&rendered.body_text);
        self.projected_body_complete = rendered.body_complete;
    }

    /// 是否已越过合并窗口。
    fn merge_window_elapsed(&self) -> bool {
        self.last_patch_at
            .is_none_or(|at| at.elapsed() >= PROGRESS_MERGE_WINDOW)
    }

    /// 本回合从首次投影到现在的耗时。
    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

impl Default for TurnCard {
    fn default() -> Self {
        Self::new()
    }
}

/// 进程级投影服务。
pub(crate) struct FeishuProgressService {
    mailbox: Arc<ProgressMailbox>,
    config_path: std::path::PathBuf,
    axon_root: std::path::PathBuf,
    worker_started: Mutex<bool>,
    /// 在途回合登记表：session ID → 投影身份，带容量上界与 FIFO 淘汰。
    active_turns: Mutex<VecDeque<(String, ProgressTurn)>>,
}

static PROGRESS_SERVICE: OnceLock<FeishuProgressService> = OnceLock::new();

/// 登记一个 Feishu 主 turn，使 egress 侧能按 session 归属已提交的投影事件。
///
/// 已提交的普通流式事件不携带 `transportContext`，因此 turn 的目标必须在此登记。
pub(crate) fn register_turn(
    axon_root: std::path::PathBuf,
    config_path: std::path::PathBuf,
    session_id: &str,
    turn: ProgressTurn,
) {
    FeishuProgressService::global(axon_root, config_path).register_turn(session_id, turn);
}

/// 投影一条已提交的投影事件；未登记的 session（非 Feishu 回合）静默忽略。
pub(crate) fn project_committed_event(session_id: &str, event: ProjectionEvent) {
    let Some(service) = PROGRESS_SERVICE.get() else {
        return;
    };
    if let Some(turn) = service.registered_turn(session_id) {
        service.publish_event(turn, event);
    }
}

/// 收口该 session 的回合并返回投影结论；未登记时按不可用处理。
pub(crate) fn finish_registered_turn(session_id: &str, failed: bool) -> TurnProjection {
    let Some(service) = PROGRESS_SERVICE.get() else {
        return TurnProjection::unavailable();
    };
    match service.unregister_turn(session_id) {
        Some(turn) => service.finish_turn(turn, failed),
        None => TurnProjection::unavailable(),
    }
}

impl FeishuProgressService {
    /// 取得进程级服务实例。
    pub(crate) fn global(
        axon_root: std::path::PathBuf,
        config_path: std::path::PathBuf,
    ) -> &'static Self {
        PROGRESS_SERVICE.get_or_init(|| Self {
            mailbox: Arc::new(ProgressMailbox::new()),
            config_path,
            axon_root,
            worker_started: Mutex::new(false),
            active_turns: Mutex::new(VecDeque::new()),
        })
    }

    /// 登记一个在途回合；同一 session 重复登记只保留最新身份。
    fn register_turn(&self, session_id: &str, turn: ProgressTurn) {
        let mut turns = self
            .active_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        turns.retain(|(id, _)| id != session_id);
        if turns.len() >= PROGRESS_ACTIVE_TURNS {
            turns.pop_front();
        }
        turns.push_back((session_id.to_owned(), turn));
    }

    /// 按 session 取出已登记的回合身份。
    fn registered_turn(&self, session_id: &str) -> Option<ProgressTurn> {
        self.active_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(id, _)| id == session_id)
            .map(|(_, turn)| turn.clone())
    }

    /// 摘除并返回该 session 的回合身份；返回 `None` 表示该 session 未登记。
    fn unregister_turn(&self, session_id: &str) -> Option<ProgressTurn> {
        let mut turns = self
            .active_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        turns
            .iter()
            .position(|(id, _)| id == session_id)
            .and_then(|index| turns.remove(index))
            .map(|(_, turn)| turn)
    }

    /// 飞书进度卡片在途登记数；仅用于单元测试。
    #[cfg(test)]
    fn active_turn_count(&self) -> usize {
        self.active_turns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn ensure_worker(&self) {
        let mut started = self
            .worker_started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *started {
            return;
        }
        *started = true;
        let mailbox = Arc::clone(&self.mailbox);
        let axon_root = self.axon_root.clone();
        let config_path = self.config_path.clone();
        std::thread::Builder::new()
            .name("axon-feishu-progress".to_owned())
            .spawn(move || ProgressWorker::new(axon_root, config_path).run(&mailbox))
            .ok();
    }

    /// 提交一条已确认的投影事件；永不阻塞。
    pub(crate) fn publish_event(&self, turn: ProgressTurn, event: ProjectionEvent) {
        self.ensure_worker();
        self.mailbox
            .try_push(ProgressCommand::Event { turn, event });
    }

    /// 收口一个回合并返回投影结论。
    ///
    /// 同步等待 worker flush，是因为「已投影账本」必须与在途 PATCH 结算一致：否则
    /// 卡片收口与 durable outbox 补偿会重复投递同一段正文。等待有由网络超时派生的硬
    /// 上界；一旦超时就让出结算权，worker 随即放弃卡片收口，正文完全由 durable outbox
    /// 投递：超时结论与真实投递路径因此始终一致。
    pub(crate) fn finish_turn(&self, turn: ProgressTurn, failed: bool) -> TurnProjection {
        self.ensure_worker();
        let (ack, receiver) = mpsc::channel();
        let settlement = Arc::new(ProgressSettlement::new());
        let status = if failed {
            CardStatus::Failed
        } else {
            CardStatus::Completed
        };
        if !self.mailbox.try_push(ProgressCommand::Finish {
            turn,
            status,
            ack,
            settlement: Arc::clone(&settlement),
        }) {
            return TurnProjection::unavailable();
        }
        match receiver.recv_timeout(PROGRESS_FINISH_TIMEOUT) {
            Ok(projection) => projection,
            Err(_) => {
                // 让出结算权：worker 随后放弃写最终卡片，正文改由 durable outbox 独占投递。
                // 若卡片正文已经写成功，worker 早在超时前就取得结算权并回报结论，不会走到这里。
                settlement.abort();
                TurnProjection::unavailable()
            }
        }
    }
}

struct ProgressWorker {
    axon_root: std::path::PathBuf,
    config_path: std::path::PathBuf,
    client: Option<RemoteFeishuProgressClient>,
    turns: HashMap<String, TurnCard>,
    /// 已收口回合的短名单（FIFO 有界）。
    ///
    /// 收口后仍可能到达迟到增量：若照常建卡，就会在调用方已改由 durable outbox
    /// 投递之后又写出一张进度卡。这份名单让这些增量被直接丢弃。
    finished_turns: VecDeque<String>,
}

impl ProgressWorker {
    fn new(axon_root: std::path::PathBuf, config_path: std::path::PathBuf) -> Self {
        Self {
            axon_root,
            config_path,
            client: None,
            turns: HashMap::new(),
            finished_turns: VecDeque::new(),
        }
    }

    /// 记录一次已收口回合，超出容量时淘汰最早的记录。
    fn note_finished(&mut self, dispatch_id: &str) {
        self.finished_turns.retain(|id| id != dispatch_id);
        if self.finished_turns.len() >= PROGRESS_FINISHED_TURNS {
            self.finished_turns.pop_front();
        }
        self.finished_turns.push_back(dispatch_id.to_owned());
    }

    /// 该回合是否已经收口；收口后的迟到增量必须丢弃。
    fn is_finished(&self, dispatch_id: &str) -> bool {
        self.finished_turns.iter().any(|id| id == dispatch_id)
    }

    fn run(mut self, mailbox: &ProgressMailbox) {
        while let Some(command) = mailbox.pop() {
            match command {
                ProgressCommand::Event { turn, event } => {
                    self.apply_event(&turn, event);
                }
                ProgressCommand::Finish {
                    turn,
                    status,
                    ack,
                    settlement,
                } => {
                    // `Finish` 优先出队，因此这里最多只等待一个在途网络调用。
                    let projection = if settlement.begin() {
                        self.finish(&turn, status)
                    } else {
                        // 调用方已超时弃权：丢弃该回合状态，正文改由 durable outbox 投递，
                        // 卡片保持既有内容，避免同一正文出现两条消息。
                        let _ = self.turns.remove(&turn.dispatch_id);
                        TurnProjection::unavailable()
                    };
                    self.note_finished(&turn.dispatch_id);
                    let _ = ack.send(projection);
                }
            }
        }
    }

    /// 应用一条投影事件：归并到回合投影，再按合并窗口决定是否刷新卡片。
    ///
    /// 正文与过程行走同一个门：工具边界不再绕开合并窗口，避免一个多工具回合产生
    /// 数十次额外的全量写入。
    fn apply_event(&mut self, turn: &ProgressTurn, event: ProjectionEvent) {
        if self.is_finished(&turn.dispatch_id) {
            return;
        }
        let mut card = self.turns.remove(&turn.dispatch_id).unwrap_or_default();
        card.apply(event);
        if card.merge_window_elapsed() {
            self.flush_card(turn, &mut card, CardStatus::Running);
        }
        self.turns.insert(turn.dispatch_id.clone(), card);
    }

    /// 收口一个回合：写入最终卡片并结算「已投影账本」。
    fn finish(&mut self, turn: &ProgressTurn, status: CardStatus) -> TurnProjection {
        let Some(card) = self.turns.remove(&turn.dispatch_id) else {
            // 从未收到可渲染内容：可能是空回复，也可能是投影不可用。
            return TurnProjection::unavailable();
        };
        let mut card = card;
        self.flush_card(turn, &mut card, status);

        let raw_body = card.projection.body_text();
        let visible = render_progress_body(&raw_body);
        // 只有「卡片承载的正是当前可见正文、且未被任何预算截断」才算完整投影。
        let fully_projected = card.card_message_id.is_some()
            && !visible.text.is_empty()
            && card.projected_body == visible.text
            && card.projected_body_complete
            && !visible.truncated;
        TurnProjection {
            outcome: if fully_projected {
                ProjectionOutcome::FullyProjected
            } else {
                ProjectionOutcome::Unprojected
            },
            has_media_directive: AgentReplyAttachmentRuntime::has_media_directive(&raw_body),
        }
    }

    /// 把卡片写入飞书；成功才更新已投影账本与指纹。
    fn flush_card(&mut self, turn: &ProgressTurn, card: &mut TurnCard, status: CardStatus) {
        if card.projection.is_empty() {
            // 没有任何可渲染内容时不建空卡。
            return;
        }
        let Some(client) = self.client() else {
            return;
        };
        let rendered = card.render(status);
        if card.card_message_id.is_some()
            && card.projected_fingerprint == Some(rendered.fingerprint)
        {
            // 卡片内容与上次成功写入完全一致：不重复发网络请求。
            card.adopt_ledger(&rendered);
            return;
        }
        let dispatch_id = format!("feishu-progress-{}", turn.dispatch_id);
        let result = match card.card_message_id.clone() {
            Some(message_id) => client.edit_card(&message_id, &rendered.content_json, &dispatch_id),
            None => client
                .create_card(&progress_target(turn), &rendered.content_json, &dispatch_id)
                .map(|message_id| card.card_message_id = Some(message_id)),
        };
        card.last_patch_at = Some(Instant::now());
        match result {
            Ok(()) => card.adopt_ledger(&rendered),
            Err(error) => axon_logger::warning(&format!(
                "Feishu 进度投影失败，将由 durable outbox 兜底: dispatch={} error={error:?}",
                turn.dispatch_id
            )),
        }
    }

    fn client(&mut self) -> Option<&RemoteFeishuProgressClient> {
        if self.client.is_none() {
            self.client = connect_progress_client(&self.axon_root, &self.config_path);
        }
        self.client.as_ref()
    }
}

fn progress_target(turn: &ProgressTurn) -> RemoteFeishuMessageTarget {
    RemoteFeishuMessageTarget {
        receive_id: turn.chat_id.clone(),
        receive_id_type: if turn.chat_id.starts_with("ou_") {
            "open_id".to_owned()
        } else {
            "chat_id".to_owned()
        },
        reply_to_message_id: turn.reply_to_message_id.clone(),
    }
}

/// 按当前 Feishu 配置建立进度客户端；配置或凭据不可用时投影整体降级。
fn connect_progress_client(
    axon_root: &std::path::Path,
    config_path: &std::path::Path,
) -> Option<RemoteFeishuProgressClient> {
    let _ = axon_root;
    if let Err(error) = crate::run_commands::load_run_env_file(
        &crate::setup_commands::env_path_for_config(config_path),
    ) {
        axon_logger::warning(&format!("Feishu 进度投影加载凭据失败: {error}"));
        return None;
    }
    let loaded = match axon_config::ConfigLoader::load_from_path(config_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            axon_logger::warning(&format!("Feishu 进度投影读取配置失败: {error:?}"));
            return None;
        }
    };
    let config = loaded.config.remote.feishu_websocket;
    if !config.enabled || !config.enable_notification_sending {
        return None;
    }
    let Some(app_secret) = axon_config::provider_credential(&config.app_secret_env)
        .filter(|secret| !secret.trim().is_empty())
    else {
        axon_logger::warning("Feishu 进度投影缺少 app_secret，实时投影降级");
        return None;
    };
    let api_base_url = (!config.api_base_url.trim().is_empty()).then_some(config.api_base_url);
    match RemoteFeishuProgressClient::connect(
        &config.app_id,
        &app_secret,
        api_base_url.as_deref(),
        PROGRESS_REQUEST_TIMEOUT,
    ) {
        Ok(client) => Some(client),
        Err(error) => {
            axon_logger::warning(&format!("Feishu 进度投影建连失败，实时投影降级: {error:?}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppression_requires_full_projection_without_media() {
        assert!(
            TurnProjection {
                outcome: ProjectionOutcome::FullyProjected,
                has_media_directive: false,
            }
            .suppresses_durable_reply()
        );
        assert!(
            !TurnProjection {
                outcome: ProjectionOutcome::FullyProjected,
                has_media_directive: true,
            }
            .suppresses_durable_reply(),
            "携带附件指令时正文必须留给投递链路处理附件"
        );
        assert!(!TurnProjection::unavailable().suppresses_durable_reply());
    }

    #[test]
    fn finished_turn_shortlist_drops_late_deltas_and_stays_bounded() {
        let mut worker = ProgressWorker::new(
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("missing-config.toml"),
        );
        assert!(!worker.is_finished("dispatch-late"));
        worker.note_finished("dispatch-late");
        assert!(
            worker.is_finished("dispatch-late"),
            "收口后的迟到增量必须被丢弃，否则会在 outbox 补偿之外多写一张进度卡"
        );

        // 短名单有界：超出容量后最早记录被淘汰，内存不随历史回合数增长。
        for index in 0..PROGRESS_FINISHED_TURNS {
            worker.note_finished(&format!("dispatch-{index}"));
        }
        assert_eq!(worker.finished_turns.len(), PROGRESS_FINISHED_TURNS);
        assert!(!worker.is_finished("dispatch-late"));
    }

    #[test]
    fn turn_registry_routes_events_and_releases_on_finish() {
        let turn = ProgressTurn {
            dispatch_id: "dispatch-registry".to_owned(),
            chat_id: "oc_chat".to_owned(),
            reply_to_message_id: None,
        };
        register_turn(
            std::path::PathBuf::from("."),
            std::path::PathBuf::from("missing-config.toml"),
            "session-registry",
            turn,
        );
        let service = PROGRESS_SERVICE.get().expect("登记必须初始化投影服务");
        assert_eq!(service.active_turn_count(), 1);
        // 未登记 session 的事件必须静默忽略，不影响非 Feishu 回合。
        project_committed_event(
            "session-unknown",
            ProjectionEvent::AssistantDelta {
                part_id: "p".to_owned(),
                text: "ignored".to_owned(),
            },
        );
        project_committed_event(
            "session-registry",
            ProjectionEvent::AssistantDelta {
                part_id: "p".to_owned(),
                text: "实时正文".to_owned(),
            },
        );

        // 无可用配置/凭据时投影整体降级：结论必须是「未投影」，由 durable outbox 兜底。
        let projection = finish_registered_turn("session-registry", false);
        assert_eq!(projection.outcome, ProjectionOutcome::Unprojected);
        assert!(!projection.suppresses_durable_reply());
        assert_eq!(
            service.active_turn_count(),
            0,
            "收口后必须移除登记，避免在途回合表泄漏"
        );
    }
}
