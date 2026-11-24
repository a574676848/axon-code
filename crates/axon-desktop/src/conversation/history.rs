//! 按车道枚举历史 Interaction。
//!
//! 会话正文的事实源是订阅流，历史回放的通道也只有一条：逐条订阅该车道下的
//! Interaction 并让其 durable 日志全量重放。因此这里只需要拿到「有哪些
//! Interaction、按什么顺序」，正文不从这条只读接口取。

use serde::{Deserialize, Serialize};

use std::time::Duration;

use crate::conversation::wire::{INTERACTIONS_PATH, SESSIONS_PATH};
use crate::gateway;

/// 单次拉取的历史条数上限。
const MAX_HISTORY_LIMIT: u32 = 1_000;
/// 单次拉取的历史**帧数**上限。
///
/// 与会话条数上限分开：帧页的网关上限是 2000（`workbench_http_adapter.rs` 的
/// `MAX_INTERACTION_EVENTS_LIMIT`），此前复用条数上限等于把客户端锁在一半的窗口上，
/// 数值碰巧一致只是巧合，不是契约。
const MAX_EVENT_PAGE: u32 = 2_000;
/// 默认拉取的历史条数。
const DEFAULT_HISTORY_LIMIT: u32 = 200;
/// 单次历史帧页的默认帧数：一次取够一屏正文，又不至于让单次响应过大。
const DEFAULT_EVENT_PAGE: u32 = 800;
/// 历史帧页的总超时。
///
/// 这里读的是持久日志：一条几十万帧的长轮次，一页本身就要扫上万行并序列化几百 KB，
/// 通用的 15 秒预算会把「还在读」报成「网关不可达」。慢与断是两件事，预算与文案都要分开。
const HISTORY_PAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// 一段历史帧页。
///
/// 正文一次取回，客户端读完这一段后再从 [`next_after`](Self::next_after) 起订阅
/// 实时尾：事件通道上因此只出现真正的增量，不再承担整段回放。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionHistoryPage {
    /// 交互标识。
    pub interaction_id: String,
    /// 这一页的帧；字段与事件通道上的帧同形，前端一套解析。
    pub events: Vec<HistoryFrame>,
    /// 继续读取的序号。
    pub next_after: u64,
    /// 是否还有更晚的帧没读完。
    pub has_more: bool,
    /// 读取时该会话的 durable 尾序号。
    pub tail_sequence: u64,
    /// 从 durable 尾继续订阅时用的游标；为空表示会话还没有任何事件。
    #[serde(default)]
    pub tail_cursor: Option<String>,
    /// 因正文归档已回收而被跳过的帧数；老网关没有该字段时为 0。
    #[serde(default)]
    pub skipped_archived_frames: u32,
}

/// 历史页里的一帧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFrame {
    /// 事件类型。
    pub event_type: String,
    /// 事件载荷，JSON 文本。
    pub payload_json: String,
    /// durable 游标。
    #[serde(default)]
    pub cursor: Option<String>,
    /// 是否来自历史回放；历史页恒为真，缺省按真处理。
    #[serde(default = "default_replay")]
    pub replay: bool,
}

/// 历史帧缺省即回放帧。
const fn default_replay() -> bool {
    true
}

/// 车道内一条历史 Interaction。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionSummary {
    /// Interaction 标识。
    pub interaction_id: String,
    /// 处理状态：received、processing、understood、rejected 或 interrupted。
    pub status: String,
    /// 来源渠道。
    pub channel: String,
    /// 发起这一轮的客户端；旧网关缺该字段时为空串。
    #[serde(default)]
    pub client: String,
    /// 接收时间（毫秒）。
    pub received_at_ms: u64,
    /// 入站正文。
    #[serde(default)]
    pub content: String,
    /// 入站时落地的本地附件绝对路径。
    #[serde(default)]
    pub attachments: Vec<String>,
    /// 该轮显式指定的模型；入口没有选择时为空。
    ///
    /// 折叠成一行时它是最有信息量的那一项：索引不读事件流，模型是唯一能「不花一次
    /// 扫描就说清这轮怎么跑的」的聚合事实。缺省即旧网关，界面退回状态短语。
    #[serde(default)]
    pub model: Option<String>,
    /// 该轮显式指定的推理强度；入口没有选择时为空。
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// 本轮最终交付正文；老网关、非正常收口或正文已过保留期时为空。
    ///
    /// 历史轮折叠后要能直接读到结论，因此正文随索引一起回来，客户端不必为看见它
    /// 而回放该轮的历史帧。
    #[serde(default)]
    pub answer_text: Option<String>,
    /// 本轮用过的工具名，按出现顺序、已去重。
    ///
    /// 索引轮的过程抬头据此立起与实时同一句动作摘要，读者不必先展开才知道它做过什么。
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// 已收口轮次的墙钟用时（毫秒）；只在没有指标事件的老记录上出现。
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    /// 这一轮的读数（用时、token、首字、速率）。
    ///
    /// 事实源是该轮自己的 `turn_metrics` 事件，与整条会话的累计投影同源；索引页因此
    /// 不必为了显示一行指标去回放整轮正文。读不到任何事实时为空。
    #[serde(default)]
    pub metrics: Option<TurnMetricsReading>,
}

/// 一轮的读数：字段与 `turn_metrics` 事件一一对应，未测得的一项缺省。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnMetricsReading {
    /// 本轮墙钟用时（毫秒）。
    pub active_ms: u64,
    /// 首字延迟；未测得时为空。
    #[serde(default)]
    pub ttft_ms: Option<u64>,
    /// 生成速率（0.1 tok/s 的整数刻度）；未测得时为空。
    #[serde(default)]
    pub generation_speed_tps_x10: Option<u64>,
    /// 工具调用次数。
    pub tool_calls: u32,
    /// 未缓存输入。
    pub input_tokens: u64,
    /// 缓存读取。
    pub cache_read_tokens: u64,
    /// 缓存写入。
    pub cache_write_tokens: u64,
    /// 输出。
    pub output_tokens: u64,
    /// 推理输出。
    pub reasoning_tokens: u64,
}

/// 网关载荷。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionInteractionsPayload {
    #[serde(default)]
    interactions: Vec<InteractionSummary>,
    /// 整条车道的累计用量；网关没给结论时缺省为空。
    #[serde(default)]
    session_usage: Option<SessionUsageTotals>,
    /// 整条车道已收口轮次的累计墙钟耗时；网关没给结论时缺省为空。
    #[serde(default)]
    session_elapsed_ms: Option<u64>,
    /// 会话级计划快照：网关索引扫描窗口里最近一次 `update_plan`。
    #[serde(default)]
    session_plan: Option<SessionPlanSnapshot>,
    /// 车道当前模型的权威事实；网关读不到时缺省为空。
    #[serde(default)]
    effective_model: Option<SessionEffectiveModel>,
    /// 最近一次记忆反思的提示文本；该会话还没有记忆动作时缺省为空。
    #[serde(default)]
    reflection_notice: Option<String>,
    /// 会话能力与身份事实（可写性、可中断、可答问）。
    ///
    /// 形状由网关定义、界面按键读取，因此原样透传：桌面端不重新建模一份，避免两侧
    /// 字段各改一次才对齐。网关首屏恒返回该键，值可以是 `null`（会话行不存在）；
    /// 翻页档不带该键，缺省即 `null`。
    #[serde(default)]
    session: serde_json::Value,
    /// 会话级后台作业清单：与打开哪一轮无关，界面据此还原时间线与作业读数。
    #[serde(default)]
    jobs: Vec<serde_json::Value>,
}

/// 车道当前模型的权威事实。
///
/// 网关只读该车道运行会话的**最新一轮**记录（一条 `ORDER BY sequence DESC LIMIT 1`），
/// 不遍历历史轮次：界面打开会话时据此显示真实模型，不必等逐条回放。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEffectiveModel {
    /// 该轮使用的 provider。
    pub provider: String,
    /// 该轮使用的 upstream 模型 ID。
    pub model: String,
    /// 用户当时选择的模型别名；旧记录没有该字段时为空。
    #[serde(default)]
    pub name: Option<String>,
    /// 该轮实际请求的思考强度；未设置时为空。
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

/// 一条会话的累计用量：总量与分桶。
///
/// 分桶用于指标条上的用量浮层：读者要的是「这些 token 花在哪了」，只有总量答不了。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageTotals {
    /// 总量。
    pub total_tokens: u64,
    /// 未缓存输入。
    pub input_tokens: u64,
    /// 缓存读取。
    pub cache_read_tokens: u64,
    /// 缓存写入。
    pub cache_write_tokens: u64,
    /// 输出。
    pub output_tokens: u64,
    /// 推理输出。
    pub reasoning_tokens: u64,
}

/// 会话级计划快照：网关索引里带回来的「当前那一份计划」。
///
/// 与时间线上的计划单元同源：那边保留每次更新的原子记录，这里是最近一次。字段与网关
/// 响应同名同形，桌面端不另造一套命名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPlanSnapshot {
    /// 计划标题。
    pub title: String,
    /// 步骤明细。
    pub steps: Vec<SessionPlanStep>,
}

/// 计划快照里的一步。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPlanStep {
    /// 步骤内容。
    pub step: String,
    /// 步骤状态；界面按后端原值归类。
    pub status: String,
}

/// 一页车道历史：这一页的 Interaction，加上整条车道的累计读数。
///
/// 两个累计读数都会话翻到第几页无关：界面要的是整条会话的消耗与耗时，因此它们由网关
/// 按车道聚合，客户端不靠自己已经回放到哪一条去推算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInteractions {
    /// 这一页的 Interaction，按接收时间升序。
    pub interactions: Vec<InteractionSummary>,
    /// 整条车道截至当前的累计用量；读不到结论时为空。
    #[serde(default)]
    pub session_usage: Option<SessionUsageTotals>,
    /// 整条车道已收口轮次的累计墙钟耗时；读不到结论时为空。
    #[serde(default)]
    pub session_elapsed_ms: Option<u64>,
    /// 会话级计划快照；网关扫描窗口里没有计划帧时为空。
    ///
    /// 计划坞是会话级事实：打开会话时不必等回放刚好读到 `update_plan` 那一帧，
    /// 首屏就能立起当前进度。比窗口更早的计划仍为空，那是这一档读取的边界。
    #[serde(default)]
    pub session_plan: Option<SessionPlanSnapshot>,
    /// 车道当前模型的权威事实；读不到结论时为空。
    #[serde(default)]
    pub effective_model: Option<SessionEffectiveModel>,
    /// 最近一次记忆反思的提示文本；该会话还没有记忆动作时为空。
    ///
    /// 与 `effective_model` 同一时机取回：都是「这条车道现在能对外展示的最新事实」，
    /// 不随翻页回到更早的轮次。反思是轮后异步治理的产物，客户端按「上一次记忆动作」
    /// 呈现，不承诺与当前轮同步。
    #[serde(default)]
    pub reflection_notice: Option<String>,
    /// 会话能力与身份事实（可写性、可中断、可答问）；原样透传网关形状。
    pub session: serde_json::Value,
    /// 会话级后台作业清单；网关按会话聚合，与翻到第几页无关。
    #[serde(default)]
    pub jobs: Vec<serde_json::Value>,
}

/// 反向分页游标：按「已加载窗口里最早那一条」的排序键向前翻。
///
/// 两个字段就是网关查询的排序键 `(received_at_ms, interaction_id)`：只给时间
/// 会在同一毫秒内落库多条时漏读或重读，因此必须成对使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCursor {
    /// 排序主键：接收时间（毫秒）。
    pub received_at_ms: u64,
    /// 同一毫秒内的次序键。
    pub interaction_id: String,
}

/// 列出某条车道下的 Interaction，按接收时间升序。
///
/// `before` 为空取最近一页；给出游标则取它之前的一页，用于逐条向前翻。
pub fn session_interactions(
    session_id: &str,
    limit: Option<u32>,
    before: Option<&HistoryCursor>,
) -> Result<SessionInteractions, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let limit = limit
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
        .clamp(1, MAX_HISTORY_LIMIT);
    let mut path = format!("{SESSIONS_PATH}/{session_id}/interactions?limit={limit}");
    if let Some(cursor) = before {
        // 车道标识只允许 URL 安全字符，游标仍需转义：它来自网关下发的标识，
        // 不该由客户端假定其字符集。
        let encoded: String =
            url::form_urlencoded::byte_serialize(cursor.interaction_id.as_bytes()).collect();
        path.push_str(&format!(
            "&before_ms={}&before_id={encoded}&brief=1",
            cursor.received_at_ms
        ));
    }
    let payload = gateway::get_json_at(&path)?;
    serde_json::from_value::<SessionInteractionsPayload>(payload)
        .map(|parsed| SessionInteractions {
            interactions: parsed.interactions,
            session_usage: parsed.session_usage,
            session_elapsed_ms: parsed.session_elapsed_ms,
            session_plan: parsed.session_plan,
            effective_model: parsed.effective_model,
            reflection_notice: parsed.reflection_notice,
            session: parsed.session,
            jobs: parsed.jobs,
        })
        .map_err(|error| format!("解析车道历史失败：{error}"))
}

/// 读取一条 Interaction 的一段历史帧。
///
/// `after` 为空从头读；`has_more` 为真时按 `next_after` 继续翻页，直到补到 durable 尾。
/// `tail` 为真时直接取该 Interaction 自身的尾窗：一次往返拿到可渲染的尾部窗口，
/// 不再先探整条会话的尾序号再算窗口起点。
pub fn interaction_events(
    interaction_id: &str,
    after: Option<u64>,
    limit: Option<u32>,
    tail: bool,
) -> Result<InteractionHistoryPage, String> {
    let interaction_id = interaction_id.trim();
    if interaction_id.is_empty() {
        return Err("交互标识不能为空".to_string());
    }
    let limit = limit.unwrap_or(DEFAULT_EVENT_PAGE).clamp(1, MAX_EVENT_PAGE);
    // 交互标识来自网关下发，仍按 URL 段转义：客户端不假定它的字符集。
    let encoded: String = url::form_urlencoded::byte_serialize(interaction_id.as_bytes()).collect();
    let after = after.unwrap_or(0);
    let tail_query = if tail { "&tail=1" } else { "" };
    let path =
        format!("{INTERACTIONS_PATH}/{encoded}/events?after={after}&limit={limit}{tail_query}");
    let payload = gateway::get_json_at_with_timeout(&path, HISTORY_PAGE_TIMEOUT)?;
    serde_json::from_value(payload).map_err(|error| format!("解析历史帧失败：{error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn history_limit_is_clamped_into_the_supported_window() {
        assert_eq!(super::MAX_HISTORY_LIMIT.min(5_000), 1_000);
        assert_eq!(
            super::DEFAULT_HISTORY_LIMIT.clamp(1, super::MAX_HISTORY_LIMIT),
            200
        );
    }

    #[test]
    fn empty_session_id_is_rejected_before_any_request() {
        assert!(super::session_interactions("  ", None, None).is_err());
    }

    #[test]
    fn a_cursor_keeps_both_sort_keys_for_the_next_page() {
        let cursor = super::HistoryCursor {
            received_at_ms: 1_730_000_000_000,
            interaction_id: "interaction:a/b".to_owned(),
        };
        // 游标拼进查询串时必须转义：标识里的分隔符不该被当成查询串结构。
        let encoded: String =
            url::form_urlencoded::byte_serialize(cursor.interaction_id.as_bytes()).collect();
        assert!(!encoded.contains('/'), "斜杠必须被转义：{encoded}");
        assert!(!encoded.contains(':'), "冒号必须被转义：{encoded}");
    }
}
