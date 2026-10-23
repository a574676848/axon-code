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

use super::gateway_kernel::{GatewayEvent, GatewayKernelError};
use crate::application::{EventStreamBridge, EventStreamSubscription};
use axon_domain::{ConsumerCursor, InteractionId, InteractionStatus};
use axon_session::{RuntimeStore, RuntimeStoreError};
use std::collections::VecDeque;

/// Interaction 状态帧的事件类型；实时通知与回放补齐共用这一份定义。
pub(super) const INTERACTION_STATUS_EVENT: &str = "interaction.status";

/// durable replay 单页事件数；page size 不是续传总量上限。
pub(super) const DEFAULT_GATEWAY_REPLAY_PAGE_SIZE: u32 = 100;
/// durable replay 一次续传读取的事件载荷字节预算。
///
/// 页数上限不能充当「续传总量上限」：订阅的契约是补齐到 durable 尾（见本模块文档），
/// 页数触顶会让长会话里最新的 Interaction 永久漏投（实测：会话事件过万后，桌面端
/// 订阅最新交互只收到 subscribed、正文整屏空白，并一直停在「生成回答中」）。
/// 因此这里按读取字节给预算：它只挡病态长日志的内存与时间，不参与「是否读到尾」的
/// 判定，正常会话（十万级事件也只有数 MB 载荷）都能读到尾。
const MAX_GATEWAY_REPLAY_BYTES: usize = 64 * 1024 * 1024;
/// durable replay 页数兜底；字节预算之外的第二道防线，正常会话不可达。
const MAX_GATEWAY_REPLAY_PAGES: usize = 100_000;

/// 建立实时订阅并把 durable 日志从 `resume_cursor` 补齐到 `live_ceiling`。
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
    let mut pending = replay_until_ceiling(
        store,
        session_id,
        interaction_id,
        resume_cursor.as_ref().map_or(0, |value| value.sequence),
        replay_page_size,
        live_ceiling.sequence,
    )?;
    append_replayed_terminal_status(&mut pending, store, interaction_id)?;
    Ok((subscription, pending, Some(live_ceiling)))
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

/// 会话级订阅并重放：不按 Interaction 过滤。
///
/// 子代理会话用它：那是「把这条会话的逐字稿照原样收下」，与主会话的交互订阅
/// 走同一套 cursor/上界语义，只是没有 Interaction 这一层过滤。
pub(super) fn subscribe_and_replay_session<S>(
    store: &S,
    stream_bridge: &EventStreamBridge,
    session_id: &axon_domain::DaemonSessionId,
    resume_cursor: Option<ConsumerCursor>,
    replay_page_size: u32,
    live_ceiling: ConsumerCursor,
) -> Result<
    (
        EventStreamSubscription,
        VecDeque<GatewayEvent>,
        Option<ConsumerCursor>,
    ),
    GatewayKernelError,
>
where
    S: RuntimeStore,
{
    let subscription = stream_bridge
        .subscribe_session(session_id.clone(), resume_cursor.clone())
        .map_err(|error| GatewayKernelError::Ingress(format!("{error:?}")))?;
    let pending = replay_session_until_ceiling(
        store,
        session_id,
        resume_cursor.as_ref().map_or(0, |value| value.sequence),
        replay_page_size,
        live_ceiling.sequence,
    )?;
    Ok((subscription, pending, Some(live_ceiling)))
}

/// 会话级重放：不按 Interaction 过滤，只按序号推进。
fn replay_session_until_ceiling<S>(
    store: &S,
    session_id: &axon_domain::DaemonSessionId,
    after_sequence: u64,
    replay_page_size: u32,
    ceiling_sequence: u64,
) -> Result<VecDeque<GatewayEvent>, GatewayKernelError>
where
    S: RuntimeStore,
{
    let mut pending = VecDeque::new();
    let mut after = after_sequence;
    let mut budget = MAX_GATEWAY_REPLAY_BYTES;
    for _ in 0..MAX_GATEWAY_REPLAY_PAGES {
        if after >= ceiling_sequence {
            break;
        }
        let page = store
            .replay_events(session_id, after, replay_page_size)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.sequence;
        let page_len = page.len();
        for event in page {
            budget = budget.saturating_sub(event.payload_json.len());
            let cursor = serde_json::to_string(&event.cursor()).ok();
            pending.push_back(GatewayEvent {
                event_type: event.event_type,
                payload_json: event.payload_json,
                cursor,
                replay: true,
            });
        }
        if page_len < replay_page_size as usize || budget == 0 {
            break;
        }
    }
    if after < ceiling_sequence {
        axon_logger::warning(&format!(
            "[gateway-ws] session={} 重放字节预算耗尽，仍落后 durable 尾（已读 {after}，尾 {ceiling_sequence}）",
            session_id.as_str()
        ));
    }
    Ok(pending)
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

/// 从 `after_sequence` 起分页重放至 durable 尾，返回按序号升序的匹配事件。
///
/// 单页上限只是 window，不是总量；一次 Subscribe 必须读到 durable 尾，否则
/// 尾部（例如快速失败回合的 `turn_completed`）会永久漏收。字节预算触顶时走 logger
/// 显式审计（安装态 stderr 不落任何文件），避免"安安静静少投递"。
fn replay_until_ceiling<S>(
    store: &S,
    session_id: &axon_domain::DaemonSessionId,
    interaction_id: &InteractionId,
    after_sequence: u64,
    replay_page_size: u32,
    ceiling_sequence: u64,
) -> Result<VecDeque<GatewayEvent>, GatewayKernelError>
where
    S: RuntimeStore,
{
    let mut pending = VecDeque::new();
    let mut after = after_sequence;
    let mut budget = MAX_GATEWAY_REPLAY_BYTES;
    for _ in 0..MAX_GATEWAY_REPLAY_PAGES {
        if after >= ceiling_sequence {
            break;
        }
        let page = store
            .replay_events(session_id, after, replay_page_size)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.sequence;
        let page_len = page.len();
        for event in page {
            budget = budget.saturating_sub(event.payload_json.len());
            if event_matches_interaction(&event, interaction_id.as_str()) {
                let cursor = serde_json::to_string(&event.cursor()).ok();
                pending.push_back(GatewayEvent {
                    event_type: event.event_type,
                    payload_json: event.payload_json,
                    cursor,
                    replay: true,
                });
            }
        }
        if page_len < replay_page_size as usize || budget == 0 {
            break;
        }
    }
    if after < ceiling_sequence {
        axon_logger::warning(&format!(
            "[gateway-ws] session={} 重放字节预算耗尽，仍落后 durable 尾（已读 {after}，尾 {ceiling_sequence}）",
            session_id.as_str()
        ));
    }
    Ok(pending)
}

/// 判断 durable 事件是否属于目标 Interaction；payload 缺失该字段时视为不匹配。
///
/// 历史页要逐条扫过整段会话日志，先用子串做必要条件的粗筛：命中不了 Interaction
/// 标识的载荷不必解析 JSON，长会话因此少掉绝大多数解析开销。
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

/// 一次历史页读取的结论。
#[derive(Debug, Clone)]
pub struct InteractionHistoryPage {
    /// 这一页的帧，按序号升序。
    pub events: Vec<GatewayEvent>,
    /// 下一次读取的起点：已扫描到的最大序号（含不属于该 Interaction 的事件）。
    pub next_after: u64,
    /// 是否还有更晚的事件没有读完；为假表示这一页已经补到 durable 尾。
    pub has_more: bool,
    /// 读取时该会话的 durable 尾序号。
    pub tail_sequence: u64,
    /// 从 durable 尾继续订阅时用的游标；为空表示会话还没有任何事件。
    pub tail_cursor: Option<String>,
}

/// 读取一段历史帧：正文一次取回，实时尾才交给订阅流。
///
/// 与订阅回放共用同一份「按 Interaction 过滤 + 终态补帧」语义，区别只有一个：
/// 这里不建立实时订阅，因此历史不再逐帧穿过事件通道；客户端读完历史后，从
/// [`InteractionHistoryPage::next_after`] 起订阅，事件流上只会出现真正的增量。
pub fn interaction_history_page<S>(
    store: &S,
    interaction_id: &InteractionId,
    after_sequence: u64,
    limit: u32,
) -> Result<InteractionHistoryPage, GatewayKernelError>
where
    S: RuntimeStore,
{
    let interaction = store
        .get_interaction(interaction_id)
        .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?
        .ok_or_else(|| GatewayKernelError::Ingress("历史目标 Interaction 不存在".to_owned()))?;
    let session_id = crate::application::primary_session_id_for_interaction(&interaction)
        .map_err(GatewayKernelError::Ingress)?;
    let tail_sequence = live_ceiling_cursor(store, &session_id, None)?.sequence;
    let limit = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);
    let mut pending = VecDeque::new();
    let mut after = after_sequence.min(tail_sequence);
    let mut tail_cursor = None;
    while pending.len() < limit && after < tail_sequence {
        let page = store
            .replay_events(&session_id, after, DEFAULT_GATEWAY_REPLAY_PAGE_SIZE)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        if page.is_empty() {
            break;
        }
        for event in page {
            // 游标要记在**所有**扫过的事件上：客户端据它订阅实时尾，跳过不匹配的
            // 事件也必须从它们之后开始，否则会把别条交互的正文重复读一遍。
            tail_cursor = serde_json::to_string(&event.cursor()).ok();
            // 续读游标只推进到**真正消费过**的这一条：本页凑满上限就收工时，
            // 同一读页里还没看的事件必须留到下一次，否则它们会被静默跳过。
            after = event.sequence;
            if !event_matches_interaction(&event, interaction_id.as_str()) {
                continue;
            }
            let cursor = serde_json::to_string(&event.cursor()).ok();
            pending.push_back(GatewayEvent {
                event_type: event.event_type,
                payload_json: event.payload_json,
                cursor,
                replay: true,
            });
            if pending.len() >= limit {
                break;
            }
        }
    }
    let has_more = after < tail_sequence;
    if !has_more {
        // 补到尾才补终态：翻页中途收口会让界面提前把这一轮判成结束。
        append_replayed_terminal_status(&mut pending, store, interaction_id)?;
    }
    Ok(InteractionHistoryPage {
        events: pending.into_iter().collect(),
        next_after: after,
        has_more,
        tail_sequence,
        tail_cursor,
    })
}

#[cfg(test)]
#[path = "gateway_replay_tests.rs"]
mod tests;
