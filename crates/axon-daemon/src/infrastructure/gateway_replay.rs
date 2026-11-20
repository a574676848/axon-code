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
use axon_session::{RuntimeStore, RuntimeStoreError, SESSION_EVENT_MISSING_ARCHIVE_TYPE};
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
/// 老记录「整会话保尾扫描」的页数上限。
///
/// 升级前写入的事件没有归属列，只能整条会话扫一遍；这条路径没有上界时，十万帧级
/// 会话的首屏会退化成全量逐帧读盘，而桌面端历史帧页只有 60 秒预算，最终表现为
/// 「打开历史超时」。按单页 100 帧计，2000 页覆盖 20 万帧，高于已知最长会话。
const MAX_LEGACY_SCAN_PAGES: usize = 2_000;
/// 老记录「整会话保尾扫描」的载荷字节上限，只挡病态长日志，不参与读到尾的判定。
const MAX_LEGACY_SCAN_BYTES: usize = 32 * 1024 * 1024;

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
    let mut missing_archives = 0usize;
    let mut first_missing_seq = None;
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
            if event.event_type == SESSION_EVENT_MISSING_ARCHIVE_TYPE {
                missing_archives = missing_archives.saturating_add(1);
                first_missing_seq.get_or_insert(event.sequence);
                continue;
            }
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
    if missing_archives > 0 {
        axon_logger::warning(&format!(
            "[gateway-ws] session={} interaction={} 回放跳过 {} 条缺失外置正文的事件（首条 sequence={}）",
            session_id.as_str(),
            interaction_id.as_str(),
            missing_archives,
            first_missing_seq.unwrap_or(0)
        ));
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
    /// 因正文归档已被回收而跳过的帧数。
    ///
    /// 占位帧不会交给客户端（它们的载荷是回收事实而不是正文），但「跳过」这一事实
    /// 必须下发给消费方：否则界面会把「正文已回收」读成「这一轮本来就没有过程」。
    pub skipped_archived_frames: u32,
}

/// 读取一段历史帧：正文一次取回，实时尾才交给订阅流。
///
/// `tail` 为真时，只读目标 Interaction 自身的尾窗：一次往返拿到可渲染的尾部
/// 窗口，不再先探整条会话的尾序号再算窗口起点。老交互的帧序号远小于整条会话
/// 的尾序号，先探再算会把它整个截空；定向读取从根上避免那个问题。
///
/// `tail` 为假时保持原有语义：从 `after_sequence` 起按整条会话扫描，按
/// Interaction 过滤。老记录没有 `interaction_id` 时也走这条路径。
/// `tail` 为假时从前向后翻这条 Interaction 自己的帧；`tail` 为真时只读它自己的
/// 尾窗。两条路径都按键取帧，不再为一条 Interaction 把整段会话解压一遍。
///
/// 老记录可能只把 `interactionId` 写在载荷里而没有归属列：按键读取会一条都命中
/// 不到，此时退回「整条会话扫描 + 载荷匹配」的兼容路径，宁可慢也不能让老交互
/// 展开成空白。
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
        let window_limit = u32::try_from(limit).unwrap_or(u32::MAX);
        let window = store
            .interaction_tail_window(&session_id, interaction_id, window_limit)
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
            });
        }
        // 尾窗为空：老记录只把归属写在载荷里。先按尾序号截窗扫一遍，扫空说明这个
        // 交互的帧远在窗口之前，再退回从头扫并保尾。
        let tail_start = tail_sequence.saturating_sub(u64::try_from(limit).unwrap_or(u64::MAX));
        let tailed = interaction_history_page_from_scan(
            store,
            &session_id,
            interaction_id,
            tail_start,
            limit,
            tail_sequence,
            false,
        )?;
        // 只认真正读到的事件帧：补出来的终态帧会让空页看起来「有内容」，
        // 从而把退回全量扫描这一步挡掉，老交互依旧展开成空白。
        if tailed
            .events
            .iter()
            .any(|event| event.event_type != INTERACTION_STATUS_EVENT)
        {
            return Ok(tailed);
        }
        return interaction_history_page_from_scan(
            store,
            &session_id,
            interaction_id,
            0,
            limit,
            tail_sequence,
            true,
        );
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
    // 按键一条都没命中：退回载荷匹配的整会话扫描，兼容没有归属列的老记录。
    interaction_history_page_from_scan(
        store,
        &session_id,
        interaction_id,
        after_sequence,
        limit,
        tail_sequence,
        false,
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
    })
}

/// 整会话扫描路径：从 `after_sequence` 起扫到 `tail_sequence`，按 Interaction 过滤。
///
/// `keep_tail` 为真时扫满 `limit` 也不收工，改为滑动保留最后 `limit` 帧：老记录
/// 没有归属列，只能整条会话扫一遍，而它们的帧往往远在窗口起点之前。
fn interaction_history_page_from_scan<S>(
    store: &S,
    session_id: &axon_domain::DaemonSessionId,
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
            // 触顶就停在已保住的尾窗上：继续扫下去只会把首屏拖过客户端的超时预算，
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
            // 游标要记在**所有**扫过的事件上：客户端据它订阅实时尾，跳过不匹配的
            // 事件也必须从它们之后开始，否则会把别条交互的正文重复读一遍。
            tail_cursor = serde_json::to_string(&event.cursor()).ok();
            // 续读游标只推进到**真正消费过**的这一条：本页凑满上限就收工时，
            // 同一读页里还没看的事件必须留到下一次，否则它们会被静默跳过。
            after = event.sequence;
            if event.event_type == SESSION_EVENT_MISSING_ARCHIVE_TYPE {
                missing_archives = missing_archives.saturating_add(1);
                first_missing_seq.get_or_insert(event.sequence);
                continue;
            }
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
            if keep_tail {
                // 保尾：从头扫的路径只有丢掉最早的那些帧，才能留下可渲染的尾窗。
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
            "[gateway-http] session={} interaction={} 保尾扫描触达预算上限（pages={} bytes={}），已返回部分尾窗",
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
    })
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

#[cfg(test)]
#[path = "gateway_replay_interaction_tests.rs"]
mod interaction_tests;
#[cfg(test)]
#[path = "gateway_replay_tests.rs"]
mod tests;
