//! 交互历史页：按归属读取一条轮次的 durable 帧。
//!
//! 两级读取，顺序固定：
//!
//! 1. **归属列**：按 Interaction 指纹只解压这条轮次自己的帧，这是正常路径。
//! 2. **同一条派发 join**：老记录没有归属列。子代理会话的整段帧就是那条派发轮次的
//!    正文，因此当目标是派发轮次、且该子会话的派发唯一时，整段会话帧即它的过程。
//!
//! 两级都取不到就如实报「该轮次没有归属」，**不再**退回载荷子串扫描：载荷里的
//! `interactionId` 恰恰是老记录缺归属时的不可信来源，拿它冒充归属既会把别条轮次的帧
//! 挂到当前轮次上，也会让「过程在库里但无法归位」在界面上长得像「这一轮本来就没有过程」。

use crate::infrastructure::gateway_kernel::{GatewayEvent, GatewayKernelError};
use axon_domain::{
    AGENT_DISPATCH_SOURCE_PREFIX, DaemonSessionId, InteractionId, InteractionRecord,
};
use axon_session::{RuntimeStore, SESSION_EVENT_MISSING_ARCHIVE_TYPE};
use std::collections::VecDeque;

use super::{
    DEFAULT_GATEWAY_REPLAY_PAGE_SIZE, INTERACTION_STATUS_EVENT, MAX_LEGACY_SCAN_BYTES,
    MAX_LEGACY_SCAN_PAGES, append_replayed_terminal_status, live_ceiling_cursor,
};

/// 派发 join 的扫描上限：一条子会话的派发行不会很多，取一页足够判断归属是否唯一。
///
/// 超过这个窗口就无法证明「这条子会话只有一条派发」，此时宁可如实报「没有归属」，
/// 也不能把整段会话的帧挂到一条轮次上。
const DISPATCH_JOIN_SCAN_LIMIT: u32 = 256;

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
    /// 因正文归档已被回收而跳过的帧数。
    ///
    /// 占位帧不会交给客户端（它们的载荷是回收事实而不是正文），但「跳过」这一事实
    /// 必须下发给消费方：否则界面会把「正文已回收」读成「这一轮本来就没有过程」。
    pub skipped_archived_frames: u32,
    /// 这条轮次读不到归属时的原因；为空表示没有这种情况。
    ///
    /// 消费方据此把「这一轮确实没有过程」与「过程在库里但归属无法唯一确定」分开：
    /// 后者不得用空正文冒充。
    pub unattributed_reason: Option<String>,
}

/// 派发 join 的结论。
enum DispatchJoin {
    /// 目标不是派发轮次：没有第二级可查。
    NotDispatch,
    /// 派发轮次且归属唯一：该子会话的整段帧就是这条轮次的正文。
    Resolved(DaemonSessionId),
    /// 派发轮次但归属不唯一：不得猜测，如实给出原因。
    Ambiguous(String),
}

/// 读取一段历史帧：正文一次取回，实时尾才交给订阅流。
///
/// `tail` 为真时，只读目标 Interaction 自身的尾窗：一次往返拿到可渲染的尾部
/// 窗口，不再先探整条会话的尾序号再算窗口起点。老交互的帧序号远小于整条会话
/// 的尾序号，先探再算会把它整个截空；定向读取从根上避免那个问题。
///
/// `tail` 为假时从前向后翻这条 Interaction 自己的帧。两条路径都按键取帧，不再为
/// 一条 Interaction 把整段会话解压一遍；按键一条都没命中时才走派发 join。
pub fn interaction_history_page<S>(
    store: &S,
    interaction_id: &InteractionId,
    after_sequence: u64,
    limit: u32,
    tail: bool,
) -> Result<InteractionHistoryPage, GatewayKernelError>
where
    S: RuntimeStore,
{
    let interaction = store
        .get_interaction(interaction_id)
        .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?
        .ok_or_else(|| GatewayKernelError::NotFound("历史目标 Interaction 不存在".to_owned()))?;
    /*
     * `tail` 与 `after` 是两种互斥的读取语义：前者要「这一轮的最后一窗」，后者要
     * 「从某个序号往后」。同时给出时旧实现静默忽略 `after`，`nextAfter` 还可能小于
     * 调用方给的序号，客户端按它续翻就是游标回退、重复帧与中间整段丢失。
     * 明确拒绝，让调用方改参数，而不是给出自相矛盾的分页。
     */
    if tail && after_sequence > 0 {
        return Err(GatewayKernelError::InvalidQuery(
            "tail 与 after 不能同时给出：前者取尾窗，后者按序号续读".to_owned(),
        ));
    }
    let session_id = crate::application::primary_session_id_for_interaction(&interaction)
        .map_err(GatewayKernelError::Ingress)?;
    let ceiling = live_ceiling_cursor(store, &session_id, None)?;
    let tail_sequence = ceiling.sequence;
    let limit = usize::try_from(limit.max(1)).unwrap_or(usize::MAX);

    if tail {
        let page = interaction_tail_page(
            store,
            &interaction,
            interaction_id,
            &session_id,
            tail_sequence,
            limit,
        )?;
        return Ok(page);
    }

    // 多取一条只为判定 `has_more`：交互页的下一页是否存在，不能拿整会话尾序号猜。
    let probe_limit = limit.saturating_add(1);
    let probe_limit = u32::try_from(probe_limit).unwrap_or(u32::MAX);
    let records = store
        .replay_interaction_events(&session_id, interaction_id, after_sequence, probe_limit)
        .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
    if !records.is_empty() {
        return interaction_history_page_from_records(
            store,
            interaction_id,
            records,
            limit,
            after_sequence,
            tail_sequence,
        );
    }
    match dispatch_join(store, &interaction)? {
        DispatchJoin::Resolved(dispatch_session) => interaction_history_page_from_session(
            store,
            &dispatch_session,
            interaction_id,
            after_sequence,
            limit,
            tail_sequence,
            false,
        ),
        DispatchJoin::Ambiguous(reason) => empty_history_page(
            store,
            &session_id,
            interaction_id,
            tail_sequence,
            Some(reason),
        ),
        DispatchJoin::NotDispatch => {
            empty_history_page(store, &session_id, interaction_id, tail_sequence, None)
        }
    }
}

/// 尾窗路径：先按归属列取这条轮次自己的最后一窗，取不到再走派发 join。
fn interaction_tail_page<S>(
    store: &S,
    interaction: &InteractionRecord,
    interaction_id: &InteractionId,
    session_id: &DaemonSessionId,
    tail_sequence: u64,
    limit: usize,
) -> Result<InteractionHistoryPage, GatewayKernelError>
where
    S: RuntimeStore,
{
    let window_limit = u32::try_from(limit).unwrap_or(u32::MAX);
    let window = store
        .interaction_tail_window(session_id, interaction_id, window_limit)
        .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
    if !window.is_empty() {
        let mut missing_archives = 0_usize;
        let mut first_missing_sequence = None;
        let mut pending: VecDeque<GatewayEvent> = VecDeque::new();
        for event in &window {
            if event.event_type == SESSION_EVENT_MISSING_ARCHIVE_TYPE {
                // 外置正文缺失已被存储层降级成占位帧：网关不把它交给客户端，但必须
                // 留痕，否则归档根配错、权限丢失这类系统性故障只会表现为「历史莫名缺帧」。
                missing_archives += 1;
                first_missing_sequence.get_or_insert(event.sequence);
                continue;
            }
            pending.push_back(history_event(event));
        }
        if missing_archives > 0 {
            axon_logger::warning(&format!(
                "[gateway-http] session={} interaction={} 尾窗跳过 {} 条缺失外置正文的事件（首条 sequence={}）",
                session_id.as_str(),
                interaction_id.as_str(),
                missing_archives,
                first_missing_sequence.unwrap_or(0)
            ));
        }
        let next_after = window.last().map_or(tail_sequence, |event| event.sequence);
        append_replayed_terminal_status(&mut pending, store, interaction_id)?;
        return Ok(InteractionHistoryPage {
            events: pending.into_iter().collect(),
            next_after,
            // 尾窗已经取到该交互自身的最后一帧：再往后翻只会扫到别的交互，
            // 直接判定到底，客户端据 tailCursor 接实时尾即可。
            has_more: false,
            tail_sequence,
            // 订阅游标必须是该交互最后一帧的真实 durable 游标，与扫描路径取同一
            // 种游标：会话上界游标不带 Host generation，网关按代际过滤时会丢弃
            // 它，客户端随即从序号 0 整条重放，造成重复帧、乱序与用量翻倍。
            tail_cursor: window
                .last()
                .and_then(|event| serde_json::to_string(&event.cursor()).ok()),
            skipped_archived_frames: u32::try_from(missing_archives).unwrap_or(u32::MAX),
            unattributed_reason: None,
        });
    }
    let dispatch_session = match dispatch_join(store, interaction)? {
        DispatchJoin::Resolved(dispatch_session) => dispatch_session,
        DispatchJoin::Ambiguous(reason) => {
            return empty_history_page(
                store,
                session_id,
                interaction_id,
                tail_sequence,
                Some(reason),
            );
        }
        DispatchJoin::NotDispatch => {
            return empty_history_page(store, session_id, interaction_id, tail_sequence, None);
        }
    };
    // 先按尾序号截窗读一遍，读空说明这条派发的帧远在窗口之前，再从头读并保尾。
    let window_start = tail_sequence.saturating_sub(u64::try_from(limit).unwrap_or(u64::MAX));
    let windowed = interaction_history_page_from_session(
        store,
        &dispatch_session,
        interaction_id,
        window_start,
        limit,
        tail_sequence,
        true,
    )?;
    // 只认真正读到的事件帧：补出来的终态帧会让空页看起来「有内容」，
    // 从而把退回整段读取这一步挡掉，老轮次依旧展开成空白。
    if windowed
        .events
        .iter()
        .any(|event| event.event_type != INTERACTION_STATUS_EVENT)
    {
        return Ok(windowed);
    }
    interaction_history_page_from_session(
        store,
        &dispatch_session,
        interaction_id,
        0,
        limit,
        tail_sequence,
        true,
    )
}

/// 把按键取到的记录组装成历史页（`has_more` 由多取的一条决定）。
fn interaction_history_page_from_records<S>(
    store: &S,
    interaction_id: &InteractionId,
    records: Vec<axon_session::SessionEventRecord>,
    limit: usize,
    after_sequence: u64,
    tail_sequence: u64,
) -> Result<InteractionHistoryPage, GatewayKernelError>
where
    S: RuntimeStore,
{
    let has_more = records.len() > limit;
    let mut missing_archives = 0_usize;
    let mut first_missing_sequence = None;
    let mut pending: VecDeque<GatewayEvent> = VecDeque::new();
    for event in records.iter().take(limit) {
        if event.event_type == SESSION_EVENT_MISSING_ARCHIVE_TYPE {
            missing_archives += 1;
            first_missing_sequence.get_or_insert(event.sequence);
            continue;
        }
        pending.push_back(history_event(event));
    }
    if missing_archives > 0 {
        axon_logger::warning(&format!(
            "[gateway-http] session 交互页跳过 {missing_archives} 条缺失外置正文的事件（首条 sequence={}）",
            first_missing_sequence.unwrap_or(0)
        ));
    }
    let next_after = records
        .get(limit.saturating_sub(1))
        .map_or(after_sequence, |event| event.sequence);
    if !has_more {
        // 补到尾才补终态：翻页中途收口会让界面提前把这一轮判成结束。
        append_replayed_terminal_status(&mut pending, store, interaction_id)?;
    }
    Ok(InteractionHistoryPage {
        events: pending.into_iter().collect(),
        next_after,
        has_more,
        tail_sequence,
        tail_cursor: records
            .get(limit.saturating_sub(1))
            .and_then(|event| serde_json::to_string(&event.cursor()).ok()),
        skipped_archived_frames: u32::try_from(missing_archives).unwrap_or(u32::MAX),
        unattributed_reason: None,
    })
}

/// 整段会话读取路径：从 `after_sequence` 起读到 `tail_sequence`，不再按载荷过滤。
///
/// 这条路只服务派发 join：一条子会话的整段帧就是那条派发轮次的过程，而这个会话里
/// 本来就不该有别的轮次的帧。`keep_tail` 为真时读满 `limit` 也不收工，改为滑动保留
/// 最后 `limit` 帧：派发的帧往往远在窗口起点之前。
fn interaction_history_page_from_session<S>(
    store: &S,
    session_id: &DaemonSessionId,
    interaction_id: &InteractionId,
    after_sequence: u64,
    limit: usize,
    tail_sequence: u64,
    keep_tail: bool,
) -> Result<InteractionHistoryPage, GatewayKernelError>
where
    S: RuntimeStore,
{
    let mut pending = VecDeque::new();
    let mut after = after_sequence.min(tail_sequence);
    let mut tail_cursor = None;
    let mut missing_archives = 0usize;
    let mut first_missing_seq = None;
    let mut scanned_pages = 0usize;
    let mut scanned_bytes = 0usize;
    let mut budget_exhausted = false;
    while (keep_tail || pending.len() < limit) && after < tail_sequence {
        if scanned_pages >= MAX_LEGACY_SCAN_PAGES || scanned_bytes >= MAX_LEGACY_SCAN_BYTES {
            // 触顶就停在已保住的尾窗上：继续读下去只会把首屏拖过客户端的超时预算，
            // 结果从「少一段历史」变成「整页打不开」。留告警供定位。
            budget_exhausted = true;
            break;
        }
        let page = store
            .replay_events(session_id, after, DEFAULT_GATEWAY_REPLAY_PAGE_SIZE)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        if page.is_empty() {
            break;
        }
        scanned_pages += 1;
        for event in page {
            scanned_bytes = scanned_bytes.saturating_add(event.payload_json.len());
            // 游标要记在**所有**读过的事件上：客户端据它订阅实时尾，跳过的事件也必须
            // 从它们之后开始，否则会把同一段正文重复读一遍。
            tail_cursor = serde_json::to_string(&event.cursor()).ok();
            // 续读游标只推进到**真正消费过**的这一条：本页凑满上限就收工时，
            // 同一读页里还没看的事件必须留到下一次，否则它们会被静默跳过。
            after = event.sequence;
            if event.event_type == SESSION_EVENT_MISSING_ARCHIVE_TYPE {
                missing_archives = missing_archives.saturating_add(1);
                first_missing_seq.get_or_insert(event.sequence);
                continue;
            }
            pending.push_back(history_event(&event));
            if keep_tail {
                // 保尾：从头读的路径只有丢掉最早的那些帧，才能留下可渲染的尾窗。
                if pending.len() > limit {
                    pending.pop_front();
                }
            } else if pending.len() >= limit {
                break;
            }
        }
    }
    if missing_archives > 0 {
        axon_logger::warning(&format!(
            "[gateway-http] session={} interaction={} 历史页跳过 {} 条缺失外置正文的事件（首条 sequence={}）",
            session_id.as_str(),
            interaction_id.as_str(),
            missing_archives,
            first_missing_seq.unwrap_or(0)
        ));
    }
    if budget_exhausted {
        axon_logger::warning(&format!(
            "[gateway-http] session={} interaction={} 整段读取触达预算上限（pages={} bytes={}），已返回部分尾窗",
            session_id.as_str(),
            interaction_id.as_str(),
            scanned_pages,
            scanned_bytes
        ));
    }
    let has_more = !keep_tail && after < tail_sequence;
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
        skipped_archived_frames: u32::try_from(missing_archives).unwrap_or(u32::MAX),
        unattributed_reason: None,
    })
}

/// 判断目标的第二级归属：它是不是派发轮次，以及这条子会话的派发是否唯一。
fn dispatch_join<S>(
    store: &S,
    interaction: &InteractionRecord,
) -> Result<DispatchJoin, GatewayKernelError>
where
    S: RuntimeStore,
{
    if !interaction
        .source
        .source_message_id()
        .starts_with(AGENT_DISPATCH_SOURCE_PREFIX)
    {
        return Ok(DispatchJoin::NotDispatch);
    }
    let session_id = interaction.session_id.clone();
    let rows = store
        .interactions_for_session(session_id.as_str(), DISPATCH_JOIN_SCAN_LIMIT, None)
        .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
    let window_full = rows.len() >= usize::try_from(DISPATCH_JOIN_SCAN_LIMIT).unwrap_or(usize::MAX);
    let dispatches = rows
        .iter()
        .filter(|row| {
            row.source
                .source_message_id()
                .starts_with(AGENT_DISPATCH_SOURCE_PREFIX)
        })
        .count();
    let target_in_window = rows
        .iter()
        .any(|row| row.interaction_id == interaction.interaction_id);
    if window_full || !target_in_window || dispatches != 1 {
        let reason = if window_full || !target_in_window {
            format!(
                "该轮次没有归属：子会话 {} 的轮次窗口已满（{} 条），无法证明这条派发唯一",
                session_id.as_str(),
                rows.len()
            )
        } else {
            format!(
                "该轮次没有归属：子会话 {} 有 {dispatches} 条派发轮次，无法唯一确定这批帧属于哪一轮",
                session_id.as_str()
            )
        };
        return Ok(DispatchJoin::Ambiguous(reason));
    }
    Ok(DispatchJoin::Resolved(session_id))
}

/// 一条轮次读不到任何归属帧时的页：显式给出原因，并照样交出实时尾锚点。
fn empty_history_page<S>(
    store: &S,
    session_id: &DaemonSessionId,
    interaction_id: &InteractionId,
    tail_sequence: u64,
    unattributed_reason: Option<String>,
) -> Result<InteractionHistoryPage, GatewayKernelError>
where
    S: RuntimeStore,
{
    let mut pending = VecDeque::new();
    append_replayed_terminal_status(&mut pending, store, interaction_id)?;
    Ok(InteractionHistoryPage {
        events: pending.into_iter().collect(),
        next_after: tail_sequence,
        // 没有归属帧就没有下一页可翻：再翻只会原地打转。
        has_more: false,
        tail_sequence,
        tail_cursor: session_tail_cursor(store, session_id, tail_sequence),
        skipped_archived_frames: 0,
        unattributed_reason,
    })
}

/// 空页也要给出实时尾锚点：客户端据此从 durable 尾接上实时流，而不是从序号 0 重放。
///
/// 读不到尾游标只影响续订起点，不影响这一页的结论，因此按可选值处理并留告警。
fn session_tail_cursor<S>(
    store: &S,
    session_id: &DaemonSessionId,
    tail_sequence: u64,
) -> Option<String>
where
    S: RuntimeStore,
{
    if tail_sequence == 0 {
        return None;
    }
    match store.replay_events(session_id, tail_sequence.saturating_sub(1), 1) {
        Ok(page) => page
            .last()
            .and_then(|event| serde_json::to_string(&event.cursor()).ok()),
        Err(error) => {
            axon_logger::warning(&format!(
                "[gateway-http] session={} 读取实时尾游标失败：{error}",
                session_id.as_str()
            ));
            None
        }
    }
}

/// 一条 durable 事件投影成历史页里的帧。
fn history_event(event: &axon_session::SessionEventRecord) -> GatewayEvent {
    GatewayEvent {
        event_type: event.event_type.clone(),
        payload_json: event.payload_json.clone(),
        cursor: serde_json::to_string(&event.cursor()).ok(),
        replay: true,
    }
}
