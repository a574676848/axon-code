//! Gateway durable replay：一次订阅把 durable 日志补齐到当前尾游标。
//!
//! 顺序不变量：durable 尾游标必须先于实时订阅读取。它既是重放上界，也是
//! "重放已同步到哪"的水位依据；只重放一页或颠倒顺序，都会让订阅者永久漏收
//! durable 事件（例如快速失败回合的 `turn_completed`）。
//!
//! 补齐的第二件事：目标 Interaction 已经终态时，末尾再补一条状态帧。终态是
//! 「这一轮已经结束」的唯一事实，平时只由实时通道在状态跃迁时发出；订阅一个早就
//! 结束、甚至从未产生过事件的 Interaction 时，实时通道永远不会再发它，订阅方就
//! 没有任何依据收口（界面停在「生成中」）。补在末尾而不是开头：客户端按「最后一条
//! 未收口的轮次」归约，先收口会让随后回放的正文落到上一轮去。
//!
//! 截断语义（INV-5）：单页上限与载荷预算都只是「一次调用的窗口」，都不决定「是否读到
//! 尾」。唯一硬上界是页数兜底；触顶才可能截断，而一旦截断，结论里必须带上**真实已读
//! 游标**：回执谎报「已到尾」会让未重放的中间段在下次续订时按已到尾的游标被永久跳过。
//! 截断时宁可让实时帧与已重放的帧重叠（客户端按序号去重即可），也绝不丢帧：去重水位
//! 与回执游标同源，都是这次重放真正覆盖到的位置。

use super::gateway_kernel::{GatewayEvent, GatewayKernelError, GatewayReplayReport};
use crate::application::{EventStreamBridge, EventStreamSubscription};
use axon_domain::{ConsumerCursor, InteractionId, InteractionStatus};
use axon_session::{RuntimeStore, RuntimeStoreError, SESSION_EVENT_MISSING_ARCHIVE_TYPE};
use std::collections::VecDeque;

/// Interaction 状态帧的事件类型；实时通知与回放补齐共用这一份定义。
pub(super) const INTERACTION_STATUS_EVENT: &str = "interaction.status";

/// durable replay 单页事件数；page size 不是续传总量上限。
pub(super) const DEFAULT_GATEWAY_REPLAY_PAGE_SIZE: u32 = 100;
/// durable replay 一次续传读取的事件载荷字节预算。
///
/// 它**不参与**「是否读到尾」的判定：只作为「单次调用一次性载荷」的量级告警，超过时留
/// 一条审计日志并继续读到 durable 尾。真正会截断回放的是 [`MAX_GATEWAY_REPLAY_PAGES`]。
/// 页数不能充当「续传总量上限」：订阅的契约是补齐到 durable 尾（见本模块文档），页数
/// 触顶会让长会话里最新的 Interaction 永久漏投（实测：会话事件过万后，桌面端订阅最新
/// 交互只收到 subscribed、正文整屏空白，并一直停在「生成回答中」）。
const MAX_GATEWAY_REPLAY_BYTES: usize = 64 * 1024 * 1024;
/// durable replay 页数兜底：唯一会截断回放的硬上界，正常会话不可达。
const MAX_GATEWAY_REPLAY_PAGES: usize = 100_000;
/// 老记录「整段会话读取」的页数上限。
///
/// 升级前写入的事件没有归属列，第二级只能整段会话读一遍；这条路径没有上界时，十万帧级
/// 会话的首屏会退化成全量逐帧读盘，而桌面端历史帧页只有 60 秒预算，最终表现为
/// 「打开历史超时」。按单页 100 帧计，2000 页覆盖 20 万帧，高于已知最长会话。
const MAX_LEGACY_SCAN_PAGES: usize = 2_000;
/// 老记录「整段会话读取」的载荷字节上限，只挡病态长日志，不参与读到尾的判定。
const MAX_LEGACY_SCAN_BYTES: usize = 32 * 1024 * 1024;

/// 一次 durable 回放的读取参数。
///
/// 三项各司其职：`page_size` 是分页窗口，`max_pages` 是唯一决定「是否读到尾」的硬上界，
/// `max_bytes` 只用于告警与病态日志保护，不参与读到尾的判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayReadLimits {
    /// 单页事件数：分页窗口，不是续传总量。
    pub page_size: u32,
    /// 单次调用最多读多少页：唯一硬上界，触顶才可能截断。
    pub max_pages: usize,
    /// 单次调用的载荷预算：只用于告警，不参与读到尾的判定。
    pub max_bytes: usize,
}

impl ReplayReadLimits {
    /// 生产口径：给定页大小，页数兜底与载荷预算取模块常量。
    #[must_use]
    pub fn for_page_size(page_size: u32) -> Self {
        Self {
            page_size,
            max_pages: MAX_GATEWAY_REPLAY_PAGES,
            max_bytes: MAX_GATEWAY_REPLAY_BYTES,
        }
    }
}

/// 建立实时订阅并把 durable 日志从 `resume_cursor` 补齐到 `live_ceiling`。
///
/// 返回的四元与 [`subscribe_and_replay_with_limits`] 同构：第三元是去重水位，
/// 第四元是回执要用的缺口结论。
pub(super) fn subscribe_and_replay<S>(
    store: &S,
    stream_bridge: &EventStreamBridge,
    session_id: &axon_domain::DaemonSessionId,
    interaction_id: &InteractionId,
    resume_cursor: Option<ConsumerCursor>,
    replay_page_size: u32,
    live_ceiling: ConsumerCursor,
) -> Result<
    (
        EventStreamSubscription,
        VecDeque<GatewayEvent>,
        Option<ConsumerCursor>,
        GatewayReplayReport,
    ),
    GatewayKernelError,
>
where
    S: RuntimeStore,
{
    subscribe_and_replay_with_limits(
        store,
        stream_bridge,
        session_id,
        interaction_id,
        resume_cursor,
        ReplayReadLimits::for_page_size(replay_page_size),
        live_ceiling,
    )
}

/// 按显式读取参数建立实时订阅并补齐 durable 日志。
///
/// 与 [`subscribe_and_replay`] 的唯一差别是读取上限可由调用方给出：生产路径用模块常量，
/// 诊断与测试可以把页数压到极小，从而复现「真截断」并核对回执里的续读游标。
pub fn subscribe_and_replay_with_limits<S>(
    store: &S,
    stream_bridge: &EventStreamBridge,
    session_id: &axon_domain::DaemonSessionId,
    interaction_id: &InteractionId,
    resume_cursor: Option<ConsumerCursor>,
    limits: ReplayReadLimits,
    live_ceiling: ConsumerCursor,
) -> Result<
    (
        EventStreamSubscription,
        VecDeque<GatewayEvent>,
        Option<ConsumerCursor>,
        GatewayReplayReport,
    ),
    GatewayKernelError,
>
where
    S: RuntimeStore,
{
    let subscription = stream_bridge
        .subscribe(
            session_id.clone(),
            interaction_id.clone(),
            resume_cursor.clone(),
        )
        .map_err(|error| GatewayKernelError::Ingress(format!("{error:?}")))?;
    let (mut pending, report, replay_watermark) = replay_until_ceiling(
        store,
        session_id,
        interaction_id,
        resume_cursor.as_ref().map_or(0, |value| value.sequence),
        limits,
        &live_ceiling,
    )?;
    append_replayed_terminal_status(&mut pending, store, interaction_id)?;
    // 第三元是「与实时帧去重用的水位」，与回执里的续读游标同源：两者都是这次重放真正
    // 覆盖到的位置。截断时它不再等于 durable 尾，实时通道因此会把缺口那一段补上，
    // 代价只是与已重放的帧重叠（客户端按序号去重），不会丢帧。
    Ok((subscription, pending, replay_watermark, report))
}

/// Interaction 状态帧：载荷就是状态字符串，实时与回放两条路径形状一致。
pub(super) fn interaction_status_event(status: InteractionStatus, replay: bool) -> GatewayEvent {
    GatewayEvent {
        event_type: INTERACTION_STATUS_EVENT.to_owned(),
        payload_json: serde_json::to_string(&status).unwrap_or_else(|_| "null".to_owned()),
        cursor: None,
        replay,
    }
}

/// 回放补齐：目标 Interaction 已是终态时，在 durable 事件之后补一条状态帧。
///
/// 非终态不补：那一轮还在跑，实时通道会给出真正的收口帧。记录已被保留期清理时
/// 也没有权威状态可补，如实什么都不发。
fn append_replayed_terminal_status<S>(
    pending: &mut VecDeque<GatewayEvent>,
    store: &S,
    interaction_id: &InteractionId,
) -> Result<(), GatewayKernelError>
where
    S: RuntimeStore,
{
    let interaction = store
        .get_interaction(interaction_id)
        .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
    let Some(interaction) = interaction else {
        return Ok(());
    };
    let terminal = match interaction.status {
        InteractionStatus::Received | InteractionStatus::Processing => return Ok(()),
        InteractionStatus::Understood
        | InteractionStatus::Rejected
        | InteractionStatus::Interrupted => interaction.status,
    };
    pending.push_back(interaction_status_event(terminal, true));
    Ok(())
}

/// 返回 Session 当前 durable 尾游标，作为一次 replay 的封闭上界。
///
/// Session 尚未建立时按序号 0 处理，行为与 IPC 事件订阅一致；generation 只作
/// 游标标注，Host 尚未启动时留空，不参与上界的序号判定。
pub(super) fn live_ceiling_cursor<S>(
    store: &S,
    session_id: &axon_domain::DaemonSessionId,
    generation: Option<&str>,
) -> Result<ConsumerCursor, GatewayKernelError>
where
    S: RuntimeStore,
{
    let sequence = match store.get_session(session_id) {
        Ok(runtime) => runtime.last_event_sequence,
        Err(RuntimeStoreError::NotFound(_)) => 0,
        Err(error) => return Err(GatewayKernelError::Ingress(error.to_string())),
    };
    Ok(ConsumerCursor::new(
        generation.unwrap_or_default(),
        sequence,
    ))
}

/// 从 `after_sequence` 起分页重放至 durable 尾，返回按序号升序的匹配事件、缺口统计与
/// 去重水位。
///
/// 单页上限只是 window，不是总量；一次 Subscribe 必须读到 durable 尾，否则尾部
/// （例如快速失败回合的 `turn_completed`）会永久漏收。载荷预算触顶只留一条审计日志，
/// 不再中断读取；唯一会中断的是页数兜底。缺口结论随订阅确认帧下发，让客户端自己知道
/// 这次回放是残缺的，并拿到**真实已读游标**去续读。
fn replay_until_ceiling<S>(
    store: &S,
    session_id: &axon_domain::DaemonSessionId,
    interaction_id: &InteractionId,
    after_sequence: u64,
    limits: ReplayReadLimits,
    ceiling: &ConsumerCursor,
) -> Result<
    (
        VecDeque<GatewayEvent>,
        GatewayReplayReport,
        Option<ConsumerCursor>,
    ),
    GatewayKernelError,
>
where
    S: RuntimeStore,
{
    let mut pending = VecDeque::new();
    let mut after = after_sequence;
    let mut scanned_bytes = 0usize;
    // 真正读到的最后一条事件的游标：截断时它就是本页的续读位置。
    let mut last_read: Option<ConsumerCursor> = None;
    let mut missing_archives = 0usize;
    let mut first_missing_seq = None;
    // 该 Interaction 自己的帧是否已经读完：读完就不算截断，即使序号还没到会话尾。
    let mut exhausted = false;
    for _ in 0..limits.max_pages {
        if after >= ceiling.sequence {
            exhausted = true;
            break;
        }
        // 按 Interaction 取帧，不按载荷过滤：归属的权威是事件帧的归属列，子代理的
        // 逐轮正文从来不在载荷里重复写 `interactionId`。按载荷判定会让「只有归属列」
        // 的帧一条都匹配不到，订阅方因此收到空正文（子代理详情空白的直接原因）。
        let page = store
            .replay_interaction_events(session_id, interaction_id, after, limits.page_size)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let Some(last) = page.last() else {
            // 这条 Interaction 已经没有更晚的帧：剩下的少先序列属于别的轮次，不必再读。
            exhausted = true;
            break;
        };
        after = last.sequence;
        last_read = Some(last.cursor());
        let page_len = page.len();
        for event in page {
            scanned_bytes = scanned_bytes.saturating_add(event.payload_json.len());
            if event.event_type == SESSION_EVENT_MISSING_ARCHIVE_TYPE {
                missing_archives = missing_archives.saturating_add(1);
                first_missing_seq.get_or_insert(event.sequence);
                continue;
            }
            let cursor = serde_json::to_string(&event.cursor()).ok();
            pending.push_back(GatewayEvent {
                event_type: event.event_type,
                payload_json: event.payload_json,
                cursor,
                replay: true,
            });
        }
        if page_len < limits.page_size as usize {
            exhausted = true;
            break;
        }
    }
    if missing_archives > 0 {
        axon_logger::warning(&format!(
            "[gateway-ws] session={} interaction={} 回放跳过 {} 条缺失外置正文的事件（首条 sequence={}）",
            session_id.as_str(),
            interaction_id.as_str(),
            missing_archives,
            first_missing_seq.unwrap_or(0)
        ));
    }
    let truncated = !exhausted && after < ceiling.sequence;
    if truncated {
        axon_logger::warning(&format!(
            "[gateway-ws] session={} 重放触达页数硬上界，仍落后 durable 尾（已读 {after}，尾 {}）：回执将带真实已读游标",
            session_id.as_str(),
            ceiling.sequence
        ));
    } else if scanned_bytes >= limits.max_bytes {
        // 预算只报告、不截断：单次调用的载荷已经很大，但「读到尾」的契约优先。
        axon_logger::warning(&format!(
            "[gateway-ws] session={} 单次回放载荷 {} 字节，超过预算 {} 字节（按告警继续读到 durable 尾）",
            session_id.as_str(),
            scanned_bytes,
            limits.max_bytes
        ));
    }
    /*
     * 续读游标只有两种取值，且都必须如实反映「这次重放覆盖到哪里」：
     * 读到尾时就是 durable 尾（行为与修前一致）；截断时是最后一条真读到的帧，
     * 绝不能拿 durable 尾冒充，否则中间段在下次续订时会被永久跳过。
     */
    let (resume_cursor, replay_watermark) = if truncated {
        (
            last_read
                .as_ref()
                .and_then(|cursor| serde_json::to_string(cursor).ok()),
            last_read,
        )
    } else {
        (serde_json::to_string(ceiling).ok(), Some(ceiling.clone()))
    };
    Ok((
        pending,
        GatewayReplayReport {
            skipped_archived_frames: missing_archives as u64,
            replay_truncated: truncated,
            resume_cursor,
        },
        replay_watermark,
    ))
}

/// 判断 durable 事件是否属于目标 Interaction；payload 缺失该字段时视为不匹配。
///
/// 这条判据只服务实时订阅的补看路径（`daemon_host_event_subscription`）：那里的记录
/// 可能来自还没有归属列的旧版本。历史页不再用它做兜底，见 `interaction_history_page`。
pub(crate) fn event_matches_interaction(
    event: &axon_session::SessionEventRecord,
    interaction_id: &str,
) -> bool {
    if !event.payload_json.contains(interaction_id) {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .ok()
        .and_then(|payload| {
            payload
                .get("interactionId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|value| value == interaction_id)
}

#[path = "gateway_replay_history.rs"]
mod history;

pub use history::{InteractionHistoryPage, interaction_history_page};

#[cfg(test)]
#[path = "gateway_replay_interaction_tests.rs"]
mod interaction_tests;
#[cfg(test)]
#[path = "gateway_replay_tests.rs"]
mod tests;
