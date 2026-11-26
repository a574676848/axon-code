//! Provider prompt cache 遥测：按每次 provider 请求登记缓存形状与断裂原因。
//!
//! 缓存命中率掉下来时，日志里只有「这一轮很贵」的结果，没有「哪一段断了」的依据：
//! 同一个会话换渠道、工具表变一行、头部上下文重算一次，症状都是「首调整段未命中」。
//! 这里按 `resource-metrics` 同一范式补上量化面：每次请求追加一行 JSONL，按日期分片，
//! 上限 4 MiB，写入失败只告警、绝不影响主流程。
//!
//! 与 `resource-metrics` 的差别是「登记」按日志等级分流：样本恒落盘（体积小、是事后
//! 唯一能按时间轴还原断裂点的证据），日志行则按断裂严重度选取级别，`first_request`
//! 与 `history_appended` 属于正常路径，只走 DEBUG；一旦 system、tools、请求选项变化
//! 或历史被改写，就升到 WARN，让默认日志级别也能直接看到「这一轮为什么没命中」。
//!
//! 体量有三道闸，避免遥测自己变成磁盘与内存负担：
//! * 哈希只留 16 位指纹（关联用，不做完整性校验），单行几百字节；
//! * 每进程每自然日样本数封顶，超出只告警一次并停写，形状记忆照常更新；
//! * 形状记忆按会话分区并限长，先进先出淘汰，历史再长也不会随会话数线性膨胀。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use axon_agent_runtime::{
    ProviderStreamProtocol, ProviderVisibleCacheBreakReason, ProviderVisibleCacheShape,
    ProviderVisibleCacheShapeComparison, TokenUsageTelemetry,
};
use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

/// 样本目录名，与 `resource-metrics`、`channel-metrics`、`thread-io-metrics` 并列。
pub(crate) const PROMPT_CACHE_METRICS_DIR_NAME: &str = "prompt-cache-metrics";
const PROMPT_CACHE_METRICS_FILE_PREFIX: &str = "samples-";
const PROMPT_CACHE_METRICS_FILE_SUFFIX: &str = ".jsonl";
const PROMPT_CACHE_METRICS_MAX_SHARD_BYTES: u64 = 4 * 1024 * 1024;
const PROMPT_CACHE_METRICS_SHARD_STORE: ShardedJsonlStore =
    ShardedJsonlStore::with_max_shard_bytes(PROMPT_CACHE_METRICS_MAX_SHARD_BYTES);

/// 哈希指纹长度：只用于跨行关联，16 个十六进制字符足以区分同进程内的请求。
const HASH_FINGERPRINT_CHARS: usize = 16;
/// 单进程每自然日的样本上限：按单行约 300 字节估算，单日上限不足 2 MiB。
const PROMPT_CACHE_MAX_SAMPLES_PER_DAY: u64 = 4_096;
/// 形状记忆上限：按会话数封顶，历史再长也不会线性膨胀。
const PROMPT_CACHE_SHAPE_MEMORY_CAP: usize = 32;
/// 会话哨兵上限：只用来识别渠道切换，按最近活跃会话滚动保留。
const PROMPT_CACHE_RECENT_SCOPE_CAP: usize = 64;

/// 一次 provider 请求的缓存形状采样。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptCacheSample {
    /// 采样时刻（毫秒时间戳）。
    pub sampled_at_ms: u64,
    /// 进程标识。
    pub pid: u32,
    /// Provider 名称。
    pub provider: String,
    /// 模型标识。
    pub model: String,
    /// Provider 请求协议。
    pub protocol: String,
    /// 第几次尝试。
    pub attempt_index: u32,
    /// 本次请求是否拿到可用响应（由调用方按 transport 结果判定，不靠有无用量反推）。
    pub ok: bool,
    /// 相对上一轮的最大变化原因。
    pub cache_break_reason: String,
    /// 与上一轮的公共消息前缀条数。
    pub common_prefix_messages: u32,
    /// 上一轮消息条数。
    pub previous_message_count: u32,
    /// 本轮消息条数。
    pub current_message_count: u32,
    /// 历史是否被改写成长度更短的投影。
    pub conversation_rewritten: bool,
    /// 既有消息前缀是否发生变化。
    pub message_prefix_changed: bool,
    /// 本会话是否刚从其它渠道切到当前渠道（切换等于从零付前缀）。
    #[serde(default)]
    pub channel_switched: bool,
    /// 模型字段指纹。
    pub model_hash: String,
    /// system/instructions 字段指纹。
    pub system_hash: String,
    /// tools 字段指纹。
    pub tools_hash: String,
    /// 请求选项指纹。
    pub options_hash: String,
    /// 聚合请求指纹。
    pub prompt_hash: String,
    /// canonical 前缀指纹。
    pub canonical_prefix_hash: String,
    /// 非缓存输入 token。
    pub input_tokens: u64,
    /// 缓存读取 token。
    pub cached_input_tokens: u64,
    /// 完整输入 token（含缓存命中）。
    pub total_input_tokens: u64,
    /// 输出 token。
    pub output_tokens: u64,
    /// 命中率（万分之一为单位的基点）。
    pub cache_hit_rate_basis_points: u16,
}

impl PromptCacheSample {
    /// 本次请求是否构成一次真实的前缀断裂。
    ///
    /// 首轮冷启动与纯追加属于缓存正常工作方式，不构成断裂；其余原因都意味着
    /// 上一轮已经付过钱的前缀被作废。
    fn is_prefix_break(&self) -> bool {
        !matches!(
            self.cache_break_reason.as_str(),
            CACHE_REASON_FIRST_REQUEST | CACHE_REASON_HISTORY_APPENDED | CACHE_REASON_EXACT_REPLAY
        )
    }

    /// 是否需要升到告警等级：形状断裂，或本会话刚换了渠道。
    fn needs_attention(&self) -> bool {
        self.is_prefix_break() || self.channel_switched
    }
}

const CACHE_REASON_FIRST_REQUEST: &str = "first_request";
const CACHE_REASON_HISTORY_APPENDED: &str = "history_appended";
const CACHE_REASON_EXACT_REPLAY: &str = "exact_replay";

/// 遥测进程内状态：形状记忆、见过的会话与当日写盘配额。
#[derive(Default)]
struct TelemetryState {
    /// 各会话最近一次请求的缓存形状（已压缩为指纹）。
    shapes: HashMap<String, ProviderVisibleCacheShape>,
    /// 形状记忆的插入顺序，用于先进先出淘汰。
    order: VecDeque<String>,
    /// 近期出现过的会话键，用于识别渠道切换；独立于形状记忆限长，避免被淘汰后漏报。
    recent_scopes: VecDeque<String>,
    /// 当前配额所属的日期分片。
    sample_day: String,
    /// 当日已写样本数。
    sample_count: u64,
    /// 当日配额耗尽是否已经告警过。
    cap_warned: bool,
}

/// 一条样本的配额结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuotaOutcome {
    /// 可以落盘。
    Granted,
    /// 刚耗尽配额，需要告警一次。
    ExhaustedFirstTime,
    /// 配额仍然耗尽，静默跳过。
    Exhausted,
}

impl TelemetryState {
    /// 记下本会话并回答「此前是否见过它」；用于识别渠道切换。
    ///
    /// 判据不依赖形状记忆的容量：形状按「会话 + 渠道」分区、容量有限，
    /// 拿它当会话哨兵会把老会话的换渠道误记成首轮。
    fn note_scope(&mut self, scope_key: &str) -> bool {
        let seen = self.recent_scopes.iter().any(|scope| scope == scope_key);
        if !seen {
            self.recent_scopes.push_back(scope_key.to_owned());
            while self.recent_scopes.len() > PROMPT_CACHE_RECENT_SCOPE_CAP {
                self.recent_scopes.pop_front();
            }
        }
        seen
    }

    /// 记住本轮形状并返回同一会话上一轮的形状。
    fn remember_and_take(
        &mut self,
        key: &str,
        shape: &ProviderVisibleCacheShape,
    ) -> Option<ProviderVisibleCacheShape> {
        let previous = self.shapes.insert(key.to_owned(), shape.clone());
        if previous.is_none() {
            self.order.push_back(key.to_owned());
        }
        while self.order.len() > PROMPT_CACHE_SHAPE_MEMORY_CAP {
            if let Some(evicted) = self.order.pop_front() {
                self.shapes.remove(&evicted);
            }
        }
        previous
    }

    /// 申请一条当日写盘配额；跨日自动重置。
    fn reserve(&mut self, date: &str) -> QuotaOutcome {
        if self.sample_day != date {
            self.sample_day = date.to_owned();
            self.sample_count = 0;
            self.cap_warned = false;
        }
        if self.sample_count >= PROMPT_CACHE_MAX_SAMPLES_PER_DAY {
            if self.cap_warned {
                return QuotaOutcome::Exhausted;
            }
            self.cap_warned = true;
            return QuotaOutcome::ExhaustedFirstTime;
        }
        self.sample_count = self.sample_count.saturating_add(1);
        QuotaOutcome::Granted
    }
}

fn state() -> &'static Mutex<TelemetryState> {
    static STATE: OnceLock<Mutex<TelemetryState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(TelemetryState::default()))
}

/// 记录一次 provider 请求的缓存形状与用量。
///
/// 采样永远不影响主流程：目录创建、序列化、追加失败或配额耗尽都只留一条告警，
/// 调用方继续按原有结果返回。
pub(crate) fn record(
    config_path: &Path,
    scope_key: &str,
    provider: &str,
    model: &str,
    protocol: ProviderStreamProtocol,
    attempt_index: usize,
    shape: &ProviderVisibleCacheShape,
    ok: bool,
    usage: Option<&TokenUsageTelemetry>,
) {
    let sampled_at_ms = current_ms();
    let date = sample_date(sampled_at_ms);
    let key = shape_key(scope_key, provider, model, protocol);
    let compact = compact_shape(shape);
    let (previous, quota, channel_switched) = {
        let mut state = state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seen_scope = state.note_scope(scope_key);
        let previous = state.remember_and_take(&key, &compact);
        // 会话见过其它渠道、本次却是全新形状：渠道切换等价于从零开始付前缀。
        let channel_switched = seen_scope && previous.is_none();
        (previous, state.reserve(&date), channel_switched)
    };
    if quota == QuotaOutcome::ExhaustedFirstTime {
        axon_logger::warning(&format!(
            "[prompt-cache] 当日样本已达上限 {PROMPT_CACHE_MAX_SAMPLES_PER_DAY} 条，后续只保留日志不再落盘"
        ));
    }
    let comparison = compact.compare(previous.as_ref());
    let mut sample = build_sample(
        sampled_at_ms,
        provider,
        model,
        protocol,
        attempt_index,
        &compact,
        &comparison,
        ok,
        usage,
    );
    sample.channel_switched = channel_switched;
    if quota == QuotaOutcome::Granted
        && let Err(error) = append_sample(config_path, &sample)
    {
        axon_logger::warning(&format!("prompt cache 遥测落盘失败：{error}"));
    }
    log_sample(&sample);
}

/// 组装采样，供落盘与测试共用同一口径。
fn build_sample(
    sampled_at_ms: u64,
    provider: &str,
    model: &str,
    protocol: ProviderStreamProtocol,
    attempt_index: usize,
    shape: &ProviderVisibleCacheShape,
    comparison: &ProviderVisibleCacheShapeComparison,
    ok: bool,
    usage: Option<&TokenUsageTelemetry>,
) -> PromptCacheSample {
    let input_tokens = usage.map_or(0, |usage| usage.input_tokens);
    let cached_input_tokens = usage.map_or(0, |usage| usage.cache_read_input_tokens);
    let total_input_tokens = usage
        .map(|usage| usage.total_input_tokens)
        .filter(|total| *total > 0)
        .unwrap_or_else(|| input_tokens.saturating_add(cached_input_tokens));
    PromptCacheSample {
        sampled_at_ms,
        pid: std::process::id(),
        provider: provider.to_owned(),
        model: model.to_owned(),
        protocol: protocol_label(protocol).to_owned(),
        attempt_index: u32::try_from(attempt_index).unwrap_or(u32::MAX),
        ok,
        cache_break_reason: reason_label(&comparison.reason).to_owned(),
        common_prefix_messages: u32_from_usize(comparison.common_prefix_messages),
        previous_message_count: u32_from_usize(comparison.previous_message_count),
        current_message_count: u32_from_usize(comparison.current_message_count),
        conversation_rewritten: comparison.conversation_rewritten,
        message_prefix_changed: comparison.message_prefix_changed,
        channel_switched: false,
        model_hash: shape.model_hash.clone(),
        system_hash: shape.system_hash.clone(),
        tools_hash: shape.tools_hash.clone(),
        options_hash: shape.options_hash.clone(),
        prompt_hash: shape.prompt_hash.clone(),
        canonical_prefix_hash: shape.canonical_prefix_hash.clone(),
        input_tokens,
        cached_input_tokens,
        total_input_tokens,
        output_tokens: usage.map_or(0, |usage| usage.output_tokens),
        cache_hit_rate_basis_points: axon_domain::ratio_basis_points(
            cached_input_tokens,
            total_input_tokens,
        ),
    }
}

/// 压缩形状：哈希只留指纹，消息哈希列表同理。
///
/// 形状只用于相邻两轮比较与事后关联，不需要完整哈希；保留全长会让形状记忆
/// 随会话长度线性变重，也会让每行样本从三百字节涨到近两倍。
fn compact_shape(shape: &ProviderVisibleCacheShape) -> ProviderVisibleCacheShape {
    ProviderVisibleCacheShape {
        protocol: shape.protocol,
        model_hash: fingerprint(&shape.model_hash),
        system_hash: fingerprint(&shape.system_hash),
        tools_hash: fingerprint(&shape.tools_hash),
        options_hash: fingerprint(&shape.options_hash),
        message_hashes: shape
            .message_hashes
            .iter()
            .map(|hash| fingerprint(hash))
            .collect(),
        prompt_hash: fingerprint(&shape.prompt_hash),
        canonical_prefix_hash: fingerprint(&shape.canonical_prefix_hash),
        covered_message_count: shape.covered_message_count,
    }
}

fn fingerprint(hash: &str) -> String {
    hash.chars().take(HASH_FINGERPRINT_CHARS).collect()
}

/// 按断裂严重度选择日志级别：正常路径走 DEBUG，真实断裂走 WARN。
fn log_sample(sample: &PromptCacheSample) {
    if !sample.needs_attention() {
        axon_logger::debug(&format!(
            "[prompt-cache] {} reason={} prefix={}/{} hit={:.2}% cached={} total={}",
            sample.model,
            sample.cache_break_reason,
            sample.common_prefix_messages,
            sample.current_message_count,
            f64::from(sample.cache_hit_rate_basis_points) / 100.0,
            sample.cached_input_tokens,
            sample.total_input_tokens
        ));
        return;
    }
    axon_logger::warning(&format!(
        "[prompt-cache] 前缀断裂：provider={} model={} protocol={} reason={} channelSwitched={} commonPrefix={}/{} previous={} rewritten={} prefixChanged={} hit={:.2}% cached={} total={}",
        sample.provider,
        sample.model,
        sample.protocol,
        sample.cache_break_reason,
        sample.channel_switched,
        sample.common_prefix_messages,
        sample.current_message_count,
        sample.previous_message_count,
        sample.conversation_rewritten,
        sample.message_prefix_changed,
        f64::from(sample.cache_hit_rate_basis_points) / 100.0,
        sample.cached_input_tokens,
        sample.total_input_tokens
    ));
}

fn append_sample(config_path: &Path, sample: &PromptCacheSample) -> Result<(), String> {
    let directory = metrics_directory(config_path)?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("创建 prompt cache 遥测目录失败：{error}"))?;
    let line =
        serde_json::to_string(sample).map_err(|error| format!("序列化缓存样本失败：{error}"))?;
    let date = sample_date(sample.sampled_at_ms);
    PROMPT_CACHE_METRICS_SHARD_STORE
        .append_line(logical_path(&directory, &date), line.as_bytes())
        .map_err(|error| format!("追加缓存样本失败：{error}"))
}

/// 返回 prompt cache 遥测分片目录，供写出与回收共用同一口径。
pub(crate) fn metrics_directory(config_path: &Path) -> Result<PathBuf, String> {
    let data_dir =
        crate::runtime_paths::runtime_axon_data_path_static(config_path).map_err(str::to_owned)?;
    Ok(data_dir.join(PROMPT_CACHE_METRICS_DIR_NAME))
}

fn logical_path(directory: &Path, date: &str) -> PathBuf {
    directory.join(format!(
        "{PROMPT_CACHE_METRICS_FILE_PREFIX}{date}{PROMPT_CACHE_METRICS_FILE_SUFFIX}"
    ))
}

fn sample_date(timestamp_ms: u64) -> String {
    let timestamp_ms = i64::try_from(timestamp_ms).unwrap_or(i64::MAX);
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// 形状记忆键的分隔符：会话、渠道、模型、协议四段用 ASCII 单元分隔符拼接。
const SHAPE_KEY_SEPARATOR: char = '\u{1f}';

/// 请求之间的形状记忆按「会话 + 渠道 + 模型 + 协议」分区。
///
/// 键必须带会话：同一进程可能同时跑多条会话，跨会话比较只会制造假断裂。
fn shape_key(
    scope_key: &str,
    provider: &str,
    model: &str,
    protocol: ProviderStreamProtocol,
) -> String {
    format!(
        "{scope_key}{SHAPE_KEY_SEPARATOR}{provider}{SHAPE_KEY_SEPARATOR}{model}{SHAPE_KEY_SEPARATOR}{}",
        protocol_label(protocol)
    )
}

fn protocol_label(protocol: ProviderStreamProtocol) -> &'static str {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => "openai_responses",
        ProviderStreamProtocol::OpenAiCompatible => "openai_compatible",
        ProviderStreamProtocol::AnthropicMessages => "anthropic_messages",
    }
}

fn reason_label(reason: &ProviderVisibleCacheBreakReason) -> &'static str {
    match reason {
        ProviderVisibleCacheBreakReason::FirstRequest => CACHE_REASON_FIRST_REQUEST,
        ProviderVisibleCacheBreakReason::HistoryAppended => CACHE_REASON_HISTORY_APPENDED,
        ProviderVisibleCacheBreakReason::ExactReplay => CACHE_REASON_EXACT_REPLAY,
        ProviderVisibleCacheBreakReason::CompactionProjection => "compaction_projection",
        ProviderVisibleCacheBreakReason::ProtocolChanged => "protocol_changed",
        ProviderVisibleCacheBreakReason::ModelChanged => "model_changed",
        ProviderVisibleCacheBreakReason::SystemChanged => "system_changed",
        ProviderVisibleCacheBreakReason::ToolsChanged => "tools_changed",
        ProviderVisibleCacheBreakReason::RequestOptionsChanged => "request_options_changed",
        ProviderVisibleCacheBreakReason::ConversationRewritten => "conversation_rewritten",
        ProviderVisibleCacheBreakReason::MessagePrefixChanged => "message_prefix_changed",
    }
}

fn u32_from_usize(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn current_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(messages: &[&str], tools: &str) -> ProviderVisibleCacheShape {
        ProviderVisibleCacheShape {
            protocol: ProviderStreamProtocol::OpenAiCompatible,
            model_hash: "m".repeat(64),
            system_hash: "s".repeat(64),
            tools_hash: tools.to_owned(),
            options_hash: "o".repeat(64),
            message_hashes: messages.iter().map(|value| (*value).to_owned()).collect(),
            prompt_hash: "p".repeat(64),
            canonical_prefix_hash: "c".repeat(64),
            covered_message_count: messages.len(),
        }
    }

    fn usage(input_tokens: u64, cached: u64) -> TokenUsageTelemetry {
        TokenUsageTelemetry {
            input_tokens,
            cache_read_input_tokens: cached,
            total_input_tokens: input_tokens.saturating_add(cached),
            output_tokens: 7,
            ..TokenUsageTelemetry::default()
        }
    }

    fn sample(break_reason: &str) -> PromptCacheSample {
        PromptCacheSample {
            sampled_at_ms: 1,
            pid: 1,
            provider: "p".to_owned(),
            model: "m".to_owned(),
            protocol: "openai_compatible".to_owned(),
            attempt_index: 1,
            ok: true,
            cache_break_reason: break_reason.to_owned(),
            common_prefix_messages: 0,
            previous_message_count: 0,
            current_message_count: 0,
            conversation_rewritten: false,
            message_prefix_changed: false,
            channel_switched: false,
            model_hash: String::new(),
            system_hash: String::new(),
            tools_hash: String::new(),
            options_hash: String::new(),
            prompt_hash: String::new(),
            canonical_prefix_hash: String::new(),
            input_tokens: 0,
            cached_input_tokens: 0,
            total_input_tokens: 0,
            output_tokens: 0,
            cache_hit_rate_basis_points: 0,
        }
    }

    /// 采样以 camelCase 落盘并可原样回读，供事后按时间轴还原断裂点。
    #[test]
    fn samples_round_trip_through_json() {
        let current = compact_shape(&shape(&["a", "b"], "tools"));
        let comparison = ProviderVisibleCacheShapeComparison {
            reason: ProviderVisibleCacheBreakReason::HistoryAppended,
            common_prefix_messages: 1,
            previous_message_count: 1,
            current_message_count: 2,
            conversation_rewritten: false,
            message_prefix_changed: false,
        };
        let built = build_sample(
            1_700_000_000_000,
            "probe-provider",
            "probe-model",
            ProviderStreamProtocol::OpenAiCompatible,
            1,
            &current,
            &comparison,
            true,
            Some(&usage(100, 900)),
        );
        assert_eq!(built.cache_hit_rate_basis_points, 9_000);
        let line = serde_json::to_string(&built).expect("序列化");
        assert!(
            line.contains("\"cacheBreakReason\"") && line.contains("\"cachedInputTokens\""),
            "落盘字段为 camelCase：{line}"
        );
        let decoded: PromptCacheSample = serde_json::from_str(&line).expect("回读");
        assert_eq!(decoded, built);
        assert!(!decoded.is_prefix_break(), "纯追加不属于前缀断裂");
    }

    /// 哈希只保留指纹：单行体量与形状记忆都不随会话长度线性膨胀。
    #[test]
    fn shape_fingerprints_are_short_and_stable() {
        let long_hash = "a".repeat(64);
        let full = shape(&[long_hash.as_str()], "tools");
        let compact = compact_shape(&full);
        assert_eq!(compact.model_hash.len(), HASH_FINGERPRINT_CHARS);
        assert_eq!(compact.message_hashes[0].len(), HASH_FINGERPRINT_CHARS);
        assert_eq!(compact_shape(&full), compact);
    }

    /// 工具表变化是真实断裂，必须升到告警等级；纯追加与精确重放不算断裂。
    #[test]
    fn prefix_break_classification_covers_tools_and_append() {
        assert!(!sample(CACHE_REASON_HISTORY_APPENDED).is_prefix_break());
        assert!(!sample(CACHE_REASON_EXACT_REPLAY).is_prefix_break());
        assert!(!sample(CACHE_REASON_FIRST_REQUEST).is_prefix_break());
        assert!(sample("tools_changed").is_prefix_break());
        assert!(sample("system_changed").is_prefix_break());
    }

    /// 换渠道等价于从零付前缀：形状判据给不出断裂，也必须升到告警等级。
    #[test]
    fn channel_switch_escalates_even_without_shape_break() {
        let mut switched = sample(CACHE_REASON_FIRST_REQUEST);
        assert!(!switched.needs_attention());
        switched.channel_switched = true;
        assert!(switched.needs_attention());
    }

    /// 同键连续请求才会互相比较：跨会话比较只会制造假断裂。
    #[test]
    fn shape_memory_is_partitioned_by_scope() {
        let mut state = TelemetryState::default();
        let first = compact_shape(&shape(&["a"], "tools"));
        let key_a = shape_key(
            "session-a",
            "provider",
            "model",
            ProviderStreamProtocol::OpenAiCompatible,
        );
        assert!(state.remember_and_take(&key_a, &first).is_none());
        let key_b = shape_key(
            "session-b",
            "provider",
            "model",
            ProviderStreamProtocol::OpenAiCompatible,
        );
        assert!(state.remember_and_take(&key_b, &first).is_none());
        assert_eq!(state.remember_and_take(&key_a, &first), Some(first));
    }

    /// 形状记忆按上限先进先出淘汰，长期运行不会随会话数线性膨胀。
    #[test]
    fn shape_memory_is_bounded() {
        let mut state = TelemetryState::default();
        let value = compact_shape(&shape(&["a"], "tools"));
        for index in 0..PROMPT_CACHE_SHAPE_MEMORY_CAP + 8 {
            state.remember_and_take(&format!("session-{index}"), &value);
        }
        assert_eq!(state.shapes.len(), PROMPT_CACHE_SHAPE_MEMORY_CAP);
        assert_eq!(state.order.len(), PROMPT_CACHE_SHAPE_MEMORY_CAP);
    }

    /// 当日配额耗尽后只停写并告警一次，不阻塞主流程也不无限增长。
    #[test]
    fn daily_quota_is_respected() {
        let mut state = TelemetryState::default();
        for _ in 0..PROMPT_CACHE_MAX_SAMPLES_PER_DAY {
            assert_eq!(state.reserve("2026-11-24"), QuotaOutcome::Granted);
        }
        assert_eq!(
            state.reserve("2026-11-24"),
            QuotaOutcome::ExhaustedFirstTime
        );
        assert_eq!(state.reserve("2026-11-24"), QuotaOutcome::Exhausted);
        assert_eq!(
            state.reserve("2026-11-25"),
            QuotaOutcome::Granted,
            "跨日重置配额"
        );
    }

    /// 落盘路径按日期分片，避免单文件无限增长。
    #[test]
    fn samples_are_sharded_by_date() {
        let directory = PathBuf::from("data");
        assert_eq!(
            logical_path(&directory, "2026-11-24"),
            PathBuf::from("data").join("samples-2026-11-24.jsonl")
        );
    }

    /// 比较结果取首个差异原因，供日志直接引用。
    #[test]
    fn comparison_reason_is_projected_into_sample() {
        let previous = compact_shape(&shape(&["a"], "tools"));
        let current = compact_shape(&shape(&["a"], "tools-v2"));
        let comparison = current.compare(Some(&previous));
        assert_eq!(
            comparison,
            ProviderVisibleCacheShapeComparison {
                reason: ProviderVisibleCacheBreakReason::ToolsChanged,
                common_prefix_messages: 1,
                previous_message_count: 1,
                current_message_count: 1,
                conversation_rewritten: false,
                message_prefix_changed: false,
            }
        );
        assert_eq!(reason_label(&comparison.reason), "tools_changed");
    }
}
