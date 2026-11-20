//! Thread 只读投影：轮次事实、反思提示、模型与用量读数、run 记录反向重建。
//!
//! 这些入口都只读 `data/sessions/threads/*.jsonl` 或控制面投影，不回查任何第二个
//! 数据库副本：thread 文件是会话事实的唯一来源。

use std::path::Path;

use axon_session::{
    JsonlThreadEventStore, ThreadDetail, ThreadEvent, ThreadEventRecord, ThreadService, Turn,
};
use serde::Serialize;

use super::super::run_session_store::{RunSessionRecord, resolved_axon_root};
use super::{read_thread_detail, turn_metadata};

/// 一轮的零模型事实摘要，供人格规则层与信号采集使用。
///
/// 只保留阈值判定需要的量：轮序、时间、输入长度、成败与既有状态序列。
/// 正文与工具输出一律不进来，避免把历史整条搬进收口路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentTurnFact {
    /// 会话内轮序。
    pub(crate) sequence: u64,
    /// 落盘时间，Unix 毫秒。
    pub(crate) recorded_at_ms: u64,
    /// 该轮用户输入的字符数。
    pub(crate) prompt_chars: u32,
    /// 该轮是否以失败收场。
    pub(crate) failed: bool,
    /// 该轮轨迹里记录的用户状态序列；未采集过时为空。
    pub(crate) affect_sequence: Vec<String>,
}

/// 车道当前模型的权威事实：全部来自 thread settings 与最近一轮 metadata。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadTurnMetadata {
    pub(crate) usage: Option<axon_agent_runtime::TokenUsageTelemetry>,
    pub(crate) assistant_part_id: String,
    pub(crate) latest_failure: Option<String>,
    pub(crate) suggested_next_step: Option<String>,
    pub(crate) tool_receipts: Vec<serde_json::Value>,
    pub(crate) reasoning_message_count: usize,
    pub(crate) turn_event_count: usize,
    /// 用户当初选择的模型别名；恢复会话时用它回到同一别名。
    #[serde(default)]
    pub(crate) model_name: Option<String>,
    /// 上一轮记忆反思提示；下一轮开头展示，随轮次落盘而不是另存一份。
    #[serde(default)]
    pub(crate) reflection_notice: Option<String>,
    /// 本轮使用的推理强度；恢复与只读读数都用它，不再回查第二个库。
    #[serde(default)]
    pub(crate) reasoning_effort: Option<String>,
    /// 该轮用户输入的字符数；显式信号采集只需要这个量。
    #[serde(default)]
    pub(crate) prompt_chars: u32,
    /// 该轮是否以失败收场。
    #[serde(default)]
    pub(crate) failed: bool,
    /// 该轮轨迹里记录的用户状态序列；未采集过时为空。
    #[serde(default)]
    pub(crate) affect_sequence: Vec<String>,
    /// 上游实际回包模型；失败重试可能落到别的模型。
    #[serde(default)]
    pub(crate) upstream_model: Option<String>,
    /// 本轮请求路径（provider 名称）。
    #[serde(default)]
    pub(crate) request_path: Option<String>,
    /// 速度档位；未设置时为空。
    #[serde(default)]
    pub(crate) speed_mode: Option<String>,
    /// 首 token 时延；未测量时为空。
    #[serde(default)]
    pub(crate) ttft_ms: Option<u64>,
    /// 生成速度（token/s）；未测量时为空。
    #[serde(default)]
    pub(crate) generation_speed_tps: Option<f64>,
    /// 本轮入口渠道（`tui` / `cli` / `desktop` / `webhook` / `cron` / `goal`）。
    ///
    /// 用量统计按它归类来源；存量轮次没有这一列，读侧按主会话处理。
    #[serde(default)]
    pub(crate) origin_channel: Option<String>,
    /// 本轮 provider 调用次数；用量投影需要，不落原始事件。
    #[serde(default)]
    pub(crate) provider_call_count: u64,
    /// 本轮 provider 调用耗时合计，用于用量投影的延迟口径。
    #[serde(default)]
    pub(crate) provider_duration_ms: u64,
}

pub(crate) fn thread_turn_metadata_from_record(record: &RunSessionRecord) -> ThreadTurnMetadata {
    let context = record.session_context.as_ref();
    ThreadTurnMetadata {
        usage: record.usage,
        assistant_part_id: record.assistant_part_id.clone(),
        latest_failure: context.and_then(|value| value.latest_failure.clone()),
        suggested_next_step: context.map(|value| value.suggested_next_step.clone()),
        tool_receipts: record
            .tool_receipts
            .iter()
            .map(super::tool_receipt_mirror)
            .collect(),
        reasoning_message_count: record.reasoning_messages.len(),
        turn_event_count: record.turn_events.len(),
        model_name: record.model_name.clone(),
        reflection_notice: record.reflection_notice.clone(),
        reasoning_effort: record.reasoning_effort.clone(),
        prompt_chars: u32::try_from(record.prompt.chars().count()).unwrap_or(u32::MAX),
        failed: record.failure.is_some(),
        affect_sequence: record
            .turn_trace
            .as_ref()
            .map(|trace| trace.signals.affect_sequence.clone())
            .unwrap_or_default(),
        upstream_model: record.upstream_model.clone(),
        request_path: record.request_path.clone(),
        speed_mode: record.speed_mode.clone(),
        ttft_ms: record.ttft_ms,
        generation_speed_tps: record.generation_speed_tps,
        origin_channel: record.origin_channel.clone(),
        provider_call_count: record
            .turn_events
            .iter()
            .filter(|event| event.kind == axon_agent_runtime::AgentTurnEventKind::ProviderCompleted)
            .count()
            .try_into()
            .unwrap_or(u64::MAX),
        provider_duration_ms: record
            .turn_events
            .iter()
            .filter(|event| event.kind == axon_agent_runtime::AgentTurnEventKind::ProviderCompleted)
            .filter_map(|event| event.duration_ms)
            .fold(0_u64, u64::saturating_add),
    }
}

/// 最新一轮 metadata 里只有 metadata 才有的那几项事实。
///
/// 读口径只有一条：从 thread 尾部窗口里取**最新一轮**记录的 metadata；读不到就当作
/// 没有（`None`），既不回退整条 thread、也不另找第二个事实源。provider、model 与
/// 别名不在这里：它们是会话行上的现成字段，同一请求已经读过，不必为它们再读 thread。
pub(crate) struct LatestTurnFacts {
    /// 用户当时选择的别名；旧轮次没有该字段时为 `None`。
    pub(crate) model_name: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    /// 最近一次记忆反思提示；尾部窗口读不到时为空。
    pub(crate) reflection_notice: Option<String>,
}

/// 尾部预算：最新一轮 metadata 落在末尾记录里，实测单条记录在百 KB 到数 MB 量级。
///
/// 取 4 MiB 是为了覆盖「最新一轮就是最大一条」的常见情形：超长的末轮记录如果被截成
/// 半行，就只能退回整条 thread 读，那一次是按 80 MiB 计的。
const THREAD_TAIL_BUDGET_BYTES: u64 = 4 * 1024 * 1024;

/// 轻读最新一轮的 metadata；thread 不存在或尾部读不到时返回 `None`。
pub(crate) fn latest_turn_facts_at_root(root: &Path, thread_id: &str) -> Option<LatestTurnFacts> {
    let tail = JsonlThreadEventStore
        .read_thread_tail(root, thread_id, THREAD_TAIL_BUDGET_BYTES)
        .ok()?;
    let (assistant_part_id, mut metadata) = latest_turn_metadata(&tail.records)?;
    apply_metadata_patches(&tail.records, &assistant_part_id, &mut metadata);
    Some(LatestTurnFacts {
        model_name: metadata.model_name,
        reasoning_effort: metadata.reasoning_effort,
        reflection_notice: metadata.reflection_notice,
    })
}

/// 把尾窗里的治理增量套到该轮 metadata 上。
///
/// 增量事件可能有多条（反思提示与 affect 序列分开补写），按记录顺序叠加即最终值。
fn apply_metadata_patches(
    records: &[ThreadEventRecord],
    assistant_part_id: &str,
    metadata: &mut ThreadTurnMetadata,
) {
    for record in records {
        let ThreadEvent::TurnMetadataPatched {
            assistant_part_id: part,
            notice,
            affect_sequence,
        } = &record.event
        else {
            continue;
        };
        if part != assistant_part_id {
            continue;
        }
        if let Some(notice) = notice {
            metadata.reflection_notice = Some(notice.clone());
        }
        if !affect_sequence.is_empty() {
            metadata.affect_sequence = affect_sequence.clone();
        }
    }
}

/// 尾部记录里的最新一轮 metadata，附上它的 assistant part 标识。
///
/// 判据是**轮序最大**，而不是「最后一条轮次记录」：治理回写是另一条记录、可能迟到，
/// 按位置取会拿到更早一轮的模型别名与推理强度。
fn latest_turn_metadata(records: &[ThreadEventRecord]) -> Option<(String, ThreadTurnMetadata)> {
    let mut newest: Option<u64> = None;
    let mut found: Option<(String, ThreadTurnMetadata)> = None;
    for record in records {
        let Some(turn) = thread_record_turn(record) else {
            continue;
        };
        if newest.is_some_and(|known| turn.sequence < known) {
            continue;
        }
        let parsed = turn_metadata(&turn.token_usage_json);
        if newest == Some(turn.sequence) {
            // 同一轮的后写覆盖前写；只有真的解析出 metadata 才覆盖。
            if let Some(metadata) = parsed {
                found = Some((metadata.assistant_part_id.clone(), metadata));
            }
            continue;
        }
        newest = Some(turn.sequence);
        found = parsed.map(|metadata| (metadata.assistant_part_id.clone(), metadata));
    }
    found
}

/// 一条 thread 记录的轮次；不是轮次记录时为空。
fn thread_record_turn(record: &ThreadEventRecord) -> Option<&Turn> {
    match &record.event {
        ThreadEvent::TurnCompleted { turn } | ThreadEvent::TurnStarted { turn } => Some(turn),
        _ => None,
    }
}

/// 最近若干轮的事实摘要，供显式信号采集；顺序为由旧到新。
pub(crate) fn recent_turn_facts(
    config_path: &Path,
    session_id: &str,
    limit: u32,
) -> Result<Vec<RecentTurnFact>, String> {
    let root = resolved_axon_root(config_path)?;
    let Ok(detail) = ThreadService::new(JsonlThreadEventStore).read_thread(&root, session_id)
    else {
        return Ok(Vec::new());
    };
    let want = usize::try_from(limit).unwrap_or(usize::MAX);
    let start = detail.turns.len().saturating_sub(want);
    Ok(detail.turns[start..]
        .iter()
        .map(|turn| {
            let metadata = turn_metadata(&turn.token_usage_json);
            RecentTurnFact {
                sequence: turn.sequence,
                recorded_at_ms: turn.started_at_ms,
                prompt_chars: metadata.as_ref().map_or(0, |value| value.prompt_chars),
                failed: metadata.as_ref().is_some_and(|value| value.failed)
                    || turn.status == axon_session::TurnStatus::Failed,
                affect_sequence: metadata
                    .map(|value| value.affect_sequence)
                    .unwrap_or_default(),
            }
        })
        .collect())
}
/// 某轮在会话内的轮序：turn_id 由 `{session_id}-turn-{assistant_part_id}` 组成，按后缀命中。
pub(crate) fn turn_sequence(
    config_path: &Path,
    session_id: &str,
    assistant_part_id: &str,
) -> Result<Option<u64>, String> {
    let root = resolved_axon_root(config_path)?;
    let Ok(detail) = ThreadService::new(JsonlThreadEventStore).read_thread(&root, session_id)
    else {
        return Ok(None);
    };
    let suffix = format!("-turn-{assistant_part_id}");
    Ok(detail
        .turns
        .iter()
        .find(|turn| turn.turn_id.ends_with(&suffix))
        .map(|turn| turn.sequence))
}

/// 某条 thread 最近一轮观测到的 token 用量合计。
///
/// 只读轮次 metadata：无用量或缺省时读作 `None`，不拿 0 冒充「没花过 token」。
pub(crate) fn latest_turn_usage_tokens_at_root(
    config_path: &Path,
    session_id: &str,
) -> Result<Option<u64>, String> {
    let Some(detail) = read_thread_detail(config_path, session_id) else {
        return Ok(None);
    };
    Ok(detail
        .turns
        .iter()
        .rev()
        .find_map(|turn| turn_metadata(&turn.token_usage_json).and_then(|metadata| metadata.usage))
        .map(|usage| usage.total_tokens()))
}

/// 上一轮的记忆反思提示：取当前轮之前最近一条带提示的轮次。
pub(crate) fn previous_reflection_notice(
    config_path: &Path,
    session_id: &str,
    current_part_id: &str,
) -> Option<String> {
    let detail = read_thread_detail(config_path, session_id)?;
    let mut notices = detail
        .turns
        .iter()
        .map(|turn| {
            let metadata = turn_metadata(&turn.token_usage_json);
            (
                metadata
                    .as_ref()
                    .map(|value| value.assistant_part_id.clone()),
                metadata.and_then(|value| value.reflection_notice),
            )
        })
        .collect::<Vec<_>>();
    let position = notices
        .iter()
        .position(|(part_id, _)| part_id.as_deref() == Some(current_part_id))?;
    notices
        .drain(..position)
        .filter_map(|(_, notice)| notice)
        .next_back()
}

/// 治理收口产物回写：只补写本轮 metadata 中由 post-run 产生的两项事实。
///
/// 整条 `RunSessionRecord` 重写会把 thread 投影里不再收录的字段（reasoning 条数、
/// turn event 条数等）抹成零，因此这里按 `(thread, turn)` 点对点补写 metadata，
/// 其余字段原样保留；`None` 提示表示「本轮没有新提示」，不覆盖已有值，也不触发写。
/// 治理收口产物回写：只补写本轮 metadata 中由 post-run 产生的两项事实。
///
/// 写的是**增量事件**（`TurnMetadataPatched`），不再复制整份轮次记录：轮次记录本身
/// 会随历史 items 长到数 MB，复制一次等于把它再写一遍。
/// `None` 提示表示「本轮没有新提示」，不覆盖已有值，也不触发写。
pub(crate) fn patch_turn_artifacts(
    config_path: &Path,
    session_id: &str,
    assistant_part_id: &str,
    notice: Option<&str>,
    affect_sequence: &[String],
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    // 没有要补写的事实就到此为止：留痕由反射回执承担，thread 不再陪写一条空事件。
    if notice.is_none() && affect_sequence.is_empty() {
        return Ok(());
    }
    /*
     * 定位只在尾部窗口里做：治理回写紧跟在该轮收口之后，最新一轮就在尾部。尾部找不到
     * （异常顺序、单条记录超过窗口）才退回整条读，宁可慢一次也不把产物写到别的轮次上。
     */
    if tail_turn_by_part(&root, session_id, assistant_part_id).is_none()
        && !ThreadService::new(JsonlThreadEventStore)
            .read_thread(&root, session_id)?
            .turns
            .iter()
            .any(|turn| {
                turn_metadata(&turn.token_usage_json)
                    .is_some_and(|metadata| metadata.assistant_part_id == assistant_part_id)
            })
    {
        return Err(format!("thread {session_id} 缺少轮次 {assistant_part_id}"));
    }
    let appended = ThreadService::new(JsonlThreadEventStore).append_turn_metadata_patch(
        &root,
        session_id,
        crate::run_commands::run_execution::now_ms(),
        assistant_part_id,
        notice,
        affect_sequence,
    );
    // 侧车同步补写提示：读侧一次主键查询即可拿到，不再读 thread。
    if appended.is_ok()
        && let Some(notice) = notice
    {
        crate::run_commands::run_thread_facts::record_turn_notice(
            config_path,
            session_id,
            assistant_part_id,
            notice,
        );
    }
    appended
}

/// 尾部窗口里按 `assistant_part_id` 找轮次；找不到时为空。
fn tail_turn_by_part(root: &Path, thread_id: &str, assistant_part_id: &str) -> Option<Turn> {
    let tail = JsonlThreadEventStore
        .read_thread_tail(root, thread_id, THREAD_TAIL_BUDGET_BYTES)
        .ok()?;
    tail.records
        .iter()
        .rev()
        .filter_map(thread_record_turn)
        .find(|turn| {
            turn_metadata(&turn.token_usage_json)
                .is_some_and(|metadata| metadata.assistant_part_id == assistant_part_id)
        })
        .cloned()
}

/// 由 thread 轮次反向重建整条 run 记录。
///
/// 治理收口与轨迹读取共用这一份事实：迁移后不再依赖单一轮次账本。
/// 轮次 metadata 给出用量、模型别名、推理强度与失败标记，历史 item 给出输入、
/// 响应、工具调用与回执；thread 本就不收录的字段（启动时间线、turn event 明细）
/// 留空，由消费方按缺省处理。
pub(crate) fn turn_record(
    config_path: &Path,
    identities: &crate::run_commands::RunIdentities,
    assistant_part_id: &str,
) -> Result<Option<RunSessionRecord>, String> {
    let Some(detail) = read_thread_detail(config_path, &identities.run_session_id) else {
        return Ok(None);
    };
    let found = detail.turns.iter().find_map(|turn| {
        let metadata = turn_metadata(&turn.token_usage_json)?;
        (metadata.assistant_part_id == assistant_part_id).then_some((turn, metadata))
    });
    let Some((turn, metadata)) = found else {
        return Ok(None);
    };
    Ok(Some(turn_record_from_part(
        &detail, turn, &metadata, identities,
    )))
}

/// 按轮序或最新一轮取回 thread 轮次记录，并带回解出的轮序。
pub(crate) fn turn_record_by_sequence(
    config_path: &Path,
    identities: &crate::run_commands::RunIdentities,
    sequence: Option<u64>,
) -> Result<Option<(RunSessionRecord, u64)>, String> {
    let Some(detail) = read_thread_detail(config_path, &identities.run_session_id) else {
        return Ok(None);
    };
    let found = match sequence {
        Some(wanted) => detail
            .turns
            .iter()
            .find(|turn| turn.sequence == wanted)
            .map(|turn| (turn, wanted)),
        None => detail.turns.last().map(|turn| (turn, turn.sequence)),
    };
    let Some((turn, sequence)) = found else {
        return Ok(None);
    };
    let Some(metadata) = turn_metadata(&turn.token_usage_json) else {
        return Ok(None);
    };
    Ok(Some((
        turn_record_from_part(&detail, turn, &metadata, identities),
        sequence,
    )))
}

fn turn_record_from_part(
    detail: &ThreadDetail,
    turn: &Turn,
    metadata: &ThreadTurnMetadata,
    identities: &crate::run_commands::RunIdentities,
) -> RunSessionRecord {
    let projection = super::super::run_thread_history::turn_item_projection(&turn.history_items);
    RunSessionRecord {
        session_id: identities.session_id.clone(),
        thread_id: detail.thread.thread_id.clone(),
        // 父会话身份不在 Thread 里：投影只还原本轮内容，身份由调用方补齐。
        parent_session_id: None,
        parent_thread_id: detail.thread.parent_thread_id.clone(),
        interaction_id: String::new(),
        created_at_ms: turn.started_at_ms,
        startup_timeline: super::super::run_startup_snapshot::RunStartupTimeline {
            total_ms: 0,
            stages: Vec::new(),
            mcp_schema_cache: Vec::new(),
            skill_catalog_revision: String::new(),
        },
        cwd: Some(detail.thread.settings.cwd.clone()).filter(|cwd| !cwd.trim().is_empty()),
        provider: detail.thread.settings.provider.clone(),
        model: detail.thread.settings.model.clone(),
        model_name: metadata.model_name.clone(),
        selected_skill_name: detail.thread.settings.selected_skill_name.clone(),
        repo_context: None,
        session_context: None,
        durable_memory: None,
        prompt: projection.prompt,
        local_images: projection.local_images,
        local_files: projection.local_files,
        response: projection.response,
        assistant_part_id: metadata.assistant_part_id.clone(),
        usage: metadata.usage,
        tool_receipts: projection.tool_receipts,
        tool_calls: projection.tool_calls,
        reasoning_messages: projection.reasoning_messages,
        injected_context_messages: projection.injected_context_messages,
        turn_events: Vec::new(),
        failure: None,
        upstream_model: None,
        reasoning_effort: metadata.reasoning_effort.clone(),
        request_path: None,
        speed_mode: None,
        ttft_ms: None,
        generation_speed_tps: None,
        reflection_notice: metadata.reflection_notice.clone(),
        turn_trace: None,
        origin_channel: metadata.origin_channel.clone(),
    }
}
