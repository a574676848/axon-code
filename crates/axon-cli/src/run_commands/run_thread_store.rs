//! `axon run` 到统一 Thread 事件流的镜像。

use std::path::Path;

use axon_session::RuntimeStore as _;
#[cfg(test)]
use axon_session::Thread;
use axon_session::{
    AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand, FailTurnCommand,
    InterruptTurnCommand, JsonlThreadEventStore, RepairOpenTurnCommand, StartEmptyThreadCommand,
    StartThreadCommand, StartTurnCommand, ThreadCompaction, ThreadDetail, ThreadService,
    ThreadSettings, ThreadSource, ThreadStatus, Turn, TurnRepairOutcome, TurnStatus,
};
use serde::Serialize;

use super::{
    RunCommand,
    run_session_store::{
        RunSessionIndexLookup, RunSessionRecord, load_indexed_run_session, resolved_axon_root,
    },
    run_thread_history::{history_items_from_record, record_has_unsettled_tool_call},
};
#[path = "run_thread_model_history.rs"]
mod model_history;
pub(super) use model_history::thread_model_history_json;

/// 恢复会话所需的事实：全部来自 thread 文件的 settings 与最近一轮 turn metadata。
///
/// 目标态下模型可见历史只有一份：`data/sessions/threads/*.jsonl`。恢复时先定位
/// thread，再从它的 settings 与最近一轮 metadata 还原 provider、模型、cwd 与父会话，
/// 不再从第二个数据库副本反推。
#[derive(Debug, Clone)]
pub(super) struct ResumedSessionFacts {
    pub(super) session_id: String,
    pub(super) parent_session_id: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) model_name: Option<String>,
}

impl ResumedSessionFacts {
    fn from_detail(detail: &ThreadDetail) -> Self {
        let settings = &detail.thread.settings;
        let model_name = detail.turns.iter().rev().find_map(|turn| {
            turn_metadata(&turn.token_usage_json).and_then(|metadata| metadata.model_name)
        });
        Self {
            session_id: detail.thread.thread_id.clone(),
            parent_session_id: detail.thread.parent_thread_id.clone(),
            cwd: Some(settings.cwd.clone()).filter(|cwd| !cwd.trim().is_empty()),
            provider: settings.provider.clone(),
            model: settings.model.clone(),
            model_name,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PreparedRunSession {
    pub(super) facts: ResumedSessionFacts,
    pub(super) model_history_json: String,
}

pub(super) fn thread_event_sequence_snapshot(root: &Path, thread_id: &str) -> Result<u64, String> {
    let activation = axon_session::ThreadEventStore::read_thread_activation(
        &JsonlThreadEventStore,
        root,
        thread_id,
    )?;
    Ok(activation.next_sequence.saturating_sub(1))
}

pub(super) fn install_active_compaction_projection(
    root: &Path,
    session_id: &str,
    assistant_part_id: &str,
    expected_sequence: u64,
    mut compaction: ThreadCompaction,
) -> Result<u64, String> {
    let service = ThreadService::new(JsonlThreadEventStore);
    // `through_sequence` 取 thread 侧当前轮的序号：turn_id 由
    // `{session_id}-turn-{assistant_part_id}` 组成，因此按后缀精确命中，找不到再退回活动轮。
    let detail = service.read_thread(root, session_id)?;
    let suffix = format!("-turn-{assistant_part_id}");
    let current = detail
        .turns
        .iter()
        .rev()
        .find(|turn| turn.turn_id.ends_with(&suffix))
        .or_else(|| {
            detail
                .turns
                .iter()
                .rev()
                .find(|turn| turn.status == TurnStatus::Active)
        })
        .ok_or_else(|| "活动窗口缺少当前运行轮次".to_owned())?;
    compaction.through_sequence = current.sequence;
    let through_sequence = compaction.through_sequence;
    service.compact_thread(
        root,
        CompactThreadCommand {
            thread_id: session_id.to_owned(),
            recorded_at_ms: compaction.recorded_at_ms,
            expected_sequence,
            compaction,
        },
    )?;
    Ok(through_sequence)
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadTurnMetadata {
    usage: Option<axon_agent_runtime::TokenUsageTelemetry>,
    assistant_part_id: String,
    latest_failure: Option<String>,
    suggested_next_step: Option<String>,
    tool_receipts: Vec<serde_json::Value>,
    reasoning_message_count: usize,
    turn_event_count: usize,
    /// 用户当初选择的模型别名；恢复会话时用它回到同一别名。
    #[serde(default)]
    model_name: Option<String>,
    /// 上一轮记忆反思提示；下一轮开头展示，随轮次落盘而不是另存一份。
    #[serde(default)]
    reflection_notice: Option<String>,
    /// 本轮使用的推理强度；恢复与只读读数都用它，不再回查第二个库。
    #[serde(default)]
    reasoning_effort: Option<String>,
}

/// 车道当前模型的权威事实：全部来自 thread settings 与最近一轮 metadata。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionModelFacts {
    pub(crate) provider: String,
    pub(crate) model: String,
    /// 用户当时选择的别名；旧轮次没有该字段时为 `None`。
    pub(crate) model_name: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
}

pub(crate) use super::run_rollout_store::SessionUsageTotals;

/// 读某条会话的累计用量；没有投影时读作全零（新会话本来就是这个状态）。
pub(crate) fn session_usage_totals_at_root(
    root: &Path,
    session_id: &str,
) -> Result<SessionUsageTotals, String> {
    let store =
        axon_session::SqliteRuntimeStore::from_root(root.join(axon_config::AXON_RUNTIME_DATA_DIR));
    store.initialize().map_err(|error| error.to_string())?;
    let session = axon_domain::DaemonSessionId::new(session_id.to_owned())
        .map_err(|_| format!("会话 ID 非法：{session_id}"))?;
    let Some(metrics) = store
        .session_metrics(&session)
        .map_err(|error| error.to_string())?
    else {
        return Ok(SessionUsageTotals::default());
    };
    Ok(SessionUsageTotals {
        total_tokens: metrics
            .total_input_tokens
            .saturating_add(metrics.total_output_tokens),
        input_tokens: metrics.total_input_tokens,
        cache_read_tokens: metrics.total_cache_read_tokens,
        cache_write_tokens: metrics.total_cache_write_tokens,
        output_tokens: metrics.total_output_tokens,
        reasoning_tokens: metrics.total_reasoning_tokens,
    })
}
/// 读某条 thread 的当前模型事实；没有该 thread 或还没有任何一轮时读作 `None`。
pub(crate) fn session_model_at_root(
    root: &Path,
    session_id: &str,
) -> Result<Option<SessionModelFacts>, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_owned());
    }
    let Ok(detail) = ThreadService::new(JsonlThreadEventStore).read_thread(root, session_id) else {
        return Ok(None);
    };
    let settings = &detail.thread.settings;
    let metadata = detail
        .turns
        .iter()
        .rev()
        .find_map(|turn| turn_metadata(&turn.token_usage_json));
    Ok(Some(SessionModelFacts {
        provider: settings.provider.clone(),
        model: settings.model.clone(),
        model_name: metadata.as_ref().and_then(|value| value.model_name.clone()),
        reasoning_effort: metadata.and_then(|value| value.reasoning_effort),
    }))
}

/// Thread mirror 中单个工具回执摘要的字符上限。
const THREAD_TOOL_RECEIPT_SUMMARY_CHARS: usize = 512;

fn bounded_mirror_text(value: &str) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = single_line.trim();
    if trimmed.chars().count() <= THREAD_TOOL_RECEIPT_SUMMARY_CHARS {
        return trimmed.to_owned();
    }
    let mut summary = trimmed
        .chars()
        .take(THREAD_TOOL_RECEIPT_SUMMARY_CHARS)
        .collect::<String>();
    summary.push_str("...");
    summary
}

fn tool_receipt_mirror(record_receipt: &super::run_execution::RunToolReceipt) -> serde_json::Value {
    let archive_ref = record_receipt
        .output_ref
        .as_ref()
        .and_then(|output_ref| output_ref.archive_ref.as_ref())
        .map(|archive_ref| archive_ref.relative_path.clone());
    serde_json::json!({
        "callId": record_receipt.tool_call_id,
        "providerCallId": record_receipt.provider_call_id,
        "toolName": record_receipt.tool_name,
        "status": record_receipt.status,
        "summary": bounded_mirror_text(&record_receipt.summary),
        "archiveRef": archive_ref,
    })
}

/// 将一次完成的 run 写入统一 Thread 事件流。
pub(super) fn mirror_session_to_thread(
    config_path: &Path,
    record: &RunSessionRecord,
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let turn = turn_from_record(record, 0)?;
    match service.read_thread(&root, &record.session_id) {
        Ok(detail) => {
            mirror_existing_turn(&service, &root, &record.session_id, &detail, turn, record)
        }
        Err(_) if turn.status == TurnStatus::Active => {
            service.start_empty_thread(
                &root,
                StartEmptyThreadCommand {
                    thread_id: record.session_id.clone(),
                    session_root_id: record.session_id.clone(),
                    parent_thread_id: record.parent_session_id.clone(),
                    forked_from_thread_id: None,
                    source: ThreadSource::CliRun,
                    ephemeral: false,
                    settings: settings_from_record(record),
                    created_at_ms: record.created_at_ms,
                },
            )?;
            service.start_turn(
                &root,
                StartTurnCommand {
                    thread_id: record.session_id.clone(),
                    recorded_at_ms: record.created_at_ms,
                    turn,
                },
            )
        }
        Err(_) => service.start_thread(
            &root,
            StartThreadCommand {
                thread_id: record.session_id.clone(),
                session_root_id: record.session_id.clone(),
                parent_thread_id: record.parent_session_id.clone(),
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings_from_record(record),
                created_at_ms: record.created_at_ms,
                turn,
            },
        ),
    }
}

/// 在 provider child turn 发起前创建真实 active Thread/Turn。
pub(super) fn mirror_session_started(
    config_path: &Path,
    record: &RunSessionRecord,
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut turn = turn_from_record(record, 0)?;
    turn.status = TurnStatus::Active;
    turn.completed_at_ms = None;
    turn.interrupted_at_ms = None;

    let activation = service.read_thread_activation(&root, &record.session_id)?;
    if activation.exists {
        if activation.active_turn_id.as_deref() == Some(turn.turn_id.as_str()) {
            return Ok(());
        }
        // 崩溃恢复：上一进程若在 turn 中途被强杀，Thread 会遗留永不收口的 active turn。
        // 该 turn 已不可能再完成，必须先收口，否则同一会话的新 turn 会被永久拒绝。
        //
        // 收口走 repair_open_turn 而不是 interrupt_turn：前者会把崩溃时半截的工具调用
        // 一并补成「结果未知」的回执，历史因此平衡；后者只写终态，工具调用仍然悬空。
        // 收口失败只告警：修不好历史不该顺带炸掉这一轮。
        if let Some(stale) = activation.active_turn_id {
            match service.repair_open_turn(
                &root,
                RepairOpenTurnCommand {
                    thread_id: record.session_id.clone(),
                    turn_id: stale,
                    recorded_at_ms: record.created_at_ms,
                    // 激活快照刚刚读过，直接复用它的序号，省掉一次反向扫日志。
                    known_next_sequence: Some(activation.next_sequence),
                },
            ) {
                Ok(TurnRepairOutcome::Repaired {
                    turn_id,
                    synthetic_results,
                }) => axon_logger::warning(&format!(
                    "Thread {} 遗留轮次 {turn_id} 已收口，补合成工具结果 {synthetic_results} 条",
                    record.session_id
                )),
                Ok(TurnRepairOutcome::Balanced) => {}
                Err(error) => axon_logger::warning(&format!(
                    "Thread {} 遗留轮次收口失败，仍尝试开始新轮：{error}",
                    record.session_id
                )),
            }
        }
        service.start_turn(
            &root,
            StartTurnCommand {
                thread_id: record.session_id.clone(),
                recorded_at_ms: record.created_at_ms,
                turn,
            },
        )
    } else {
        service.start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: record.session_id.clone(),
                session_root_id: record.session_id.clone(),
                parent_thread_id: record.parent_session_id.clone(),
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings_from_record(record),
                created_at_ms: record.created_at_ms,
            },
        )?;
        service.start_turn(
            &root,
            StartTurnCommand {
                thread_id: record.session_id.clone(),
                recorded_at_ms: record.created_at_ms,
                turn,
            },
        )
    }
}

/// 将 provider child turn 收口到 Thread 终态。
pub(super) fn mirror_session_terminal(
    config_path: &Path,
    session_id: &str,
    status: &str,
    failure: Option<&axon_domain::RuntimeFailure>,
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let detail = service.read_thread(&root, session_id)?;
    let Some(turn) = detail
        .turns
        .iter()
        .rev()
        .find(|turn| turn.status == TurnStatus::Active)
    else {
        return Ok(());
    };
    if status == "interrupted" {
        service.interrupt_turn(
            &root,
            InterruptTurnCommand {
                thread_id: session_id.to_owned(),
                turn_id: turn.turn_id.clone(),
                recorded_at_ms: super::run_execution::now_ms(),
            },
        )
    } else {
        let failure = failure.ok_or_else(|| "失败终态缺少 RuntimeFailure".to_owned())?;
        service.fail_turn(
            &root,
            FailTurnCommand {
                thread_id: session_id.to_owned(),
                turn_id: turn.turn_id.clone(),
                failure: failure.clone(),
                recorded_at_ms: failure.timestamp,
            },
        )
    }
}

fn mirror_existing_turn(
    service: &ThreadService<JsonlThreadEventStore>,
    root: &Path,
    thread_id: &str,
    detail: &ThreadDetail,
    turn: Turn,
    record: &RunSessionRecord,
) -> Result<(), String> {
    if turn.status == TurnStatus::Active {
        if let Some(active) = detail
            .turns
            .iter()
            .find(|value| value.status == TurnStatus::Active)
        {
            if active.turn_id != turn.turn_id {
                return Err("thread 已存在不同 active turn".to_owned());
            }
            let start = active.history_items.len().min(turn.history_items.len());
            let items = turn.history_items[start..].to_vec();
            if items.is_empty() {
                return Ok(());
            }
            return service.append_turn_items(
                root,
                AppendTurnItemsCommand {
                    thread_id: thread_id.to_owned(),
                    turn_id: turn.turn_id,
                    items,
                    recorded_at_ms: record.created_at_ms,
                },
            );
        }
        return service.start_turn(
            root,
            StartTurnCommand {
                thread_id: thread_id.to_owned(),
                recorded_at_ms: record.created_at_ms,
                turn,
            },
        );
    }
    if let Some(active) = detail
        .turns
        .iter()
        .find(|value| value.status == TurnStatus::Active)
    {
        return service.append_turn(
            root,
            AppendTurnCommand {
                thread_id: thread_id.to_owned(),
                recorded_at_ms: record.created_at_ms,
                turn: Turn {
                    turn_id: active.turn_id.clone(),
                    sequence: active.sequence,
                    ..turn
                },
            },
        );
    }
    let sequence =
        u64::try_from(detail.turns.len()).map_err(|_| "thread turn 数过大".to_owned())?;
    service.append_turn(
        root,
        AppendTurnCommand {
            thread_id: thread_id.to_owned(),
            recorded_at_ms: record.created_at_ms,
            turn: Turn { sequence, ..turn },
        },
    )
}

/// 从 SQLite SSOT 恢复 run 上下文。
pub(crate) fn load_thread_session(
    config_path: &Path,
    session_id: Option<&str>,
) -> Result<Option<RunSessionRecord>, String> {
    let lookup = session_id.map_or(
        RunSessionIndexLookup::Latest,
        RunSessionIndexLookup::SessionId,
    );
    load_indexed_run_session(config_path, &lookup)
}

/// 一次完成恢复目标定位、SQLite turn 读取与模型历史装配。
pub(super) fn prepare_run_session(
    config_path: &Path,
    command: &RunCommand,
    implicit_session_id: Option<&str>,
) -> Result<Option<PreparedRunSession>, String> {
    if command.continue_last {
        return prepare_session_from_lookup(config_path, RunSessionIndexLookup::Latest);
    }
    if let Some(session_id) = command.session.as_deref() {
        return prepare_session_from_lookup(
            config_path,
            RunSessionIndexLookup::SessionId(session_id),
        );
    }
    // 未显式指定 Session 时加载当前复合车道运行窗口的既有 rollout；
    // 没有车道（子代理等隔离入口）时不恢复任何历史，由调用方自行建档。
    let Some(session_id) = implicit_session_id else {
        return Ok(None);
    };
    prepare_session_from_lookup(config_path, RunSessionIndexLookup::SessionId(session_id))
}

fn prepare_session_from_lookup(
    config_path: &Path,
    lookup: RunSessionIndexLookup<'_>,
) -> Result<Option<PreparedRunSession>, String> {
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let thread_id = match lookup {
        RunSessionIndexLookup::Latest => latest_thread_id(&service, &root)?,
        RunSessionIndexLookup::SessionId(session_id) => Some(session_id.to_owned()),
    };
    let Some(thread_id) = thread_id else {
        return Ok(None);
    };
    // 目标 thread 不存在时如实返回「没有可恢复的会话」，不隐式新建。
    let Ok(detail) = service.read_thread(&root, &thread_id) else {
        return Ok(None);
    };
    if detail.compacted_history.is_none() && window_number_exceeds_first(config_path, &thread_id)? {
        return Err(
            "会话已换窗但活动窗口投影缺失，拒绝恢复旧聊天历史；请先修复窗口投影".to_owned(),
        );
    }
    let facts = ResumedSessionFacts::from_detail(&detail);
    let model_history_json = thread_model_history_json(&detail)?;
    crate::turn_execution_ledger::restore_thread_history(&facts.session_id, &detail);
    Ok(Some(PreparedRunSession {
        facts,
        model_history_json,
    }))
}

/// 最近一个未归档 thread：`--continue` 没有指针时的兜底候选。
fn latest_thread_id(
    service: &ThreadService<JsonlThreadEventStore>,
    root: &Path,
) -> Result<Option<String>, String> {
    let summaries = service.list_threads(root, 20)?;
    Ok(summaries
        .into_iter()
        .filter(|summary| summary.status != ThreadStatus::Archived)
        .max_by_key(|summary| summary.updated_at_ms)
        .map(|summary| summary.thread_id))
}

/// 换窗后的 thread 必须带活动窗口投影，否则恢复出来的历史会跨窗。
fn window_number_exceeds_first(config_path: &Path, thread_id: &str) -> Result<bool, String> {
    let session_dir = resolved_axon_root(config_path)?
        .join("sessions")
        .join(thread_id);
    Ok(
        axon_agent_runtime::SessionNotes::load_from_session(&session_dir)
            .map_err(|error| format!("读取窗口身份失败：{error:?}"))?
            .is_some_and(|notes| notes.window_number > 1),
    )
}

/// thread 侧记录的最近一轮 metadata。
fn turn_metadata(token_usage_json: &Option<String>) -> Option<ThreadTurnMetadata> {
    let raw = token_usage_json.as_deref()?;
    serde_json::from_str::<ThreadTurnMetadata>(raw).ok()
}

#[cfg(test)]
fn thread_detail_from_records(records: &[RunSessionRecord]) -> Result<ThreadDetail, String> {
    let fallback = records
        .last()
        .ok_or_else(|| "SQLite thread turns 不能为空".to_owned())?;
    let turns = records
        .iter()
        .cloned()
        .enumerate()
        .map(|(sequence, record)| {
            u64::try_from(sequence)
                .map_err(|_| "SQLite thread turn 数过大".to_owned())
                .and_then(|sequence| turn_from_record(&record, sequence))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ThreadDetail {
        thread: Thread {
            thread_id: fallback.session_id.clone(),
            session_root_id: fallback.session_id.clone(),
            parent_thread_id: fallback.parent_session_id.clone(),
            forked_from_thread_id: None,
            name: None,
            status: ThreadStatus::Idle,
            source: ThreadSource::CliRun,
            ephemeral: false,
            settings: settings_from_record(fallback),
            created_at_ms: fallback.created_at_ms,
            updated_at_ms: fallback.created_at_ms,
            archived_at_ms: None,
        },
        turns,
        compacted_history: None,
    })
}

fn settings_from_record(record: &RunSessionRecord) -> ThreadSettings {
    ThreadSettings {
        cwd: record.cwd.clone().unwrap_or_default(),
        provider: record.provider.clone(),
        model: record.model.clone(),
        selected_skill_name: record.selected_skill_name.clone(),
    }
}

pub(super) fn mirror_failed_session_to_thread(
    config_path: &Path,
    record: &RunSessionRecord,
    failure: &axon_domain::RuntimeFailure,
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let detail = service.read_thread(&root, &record.session_id)?;
    let active = detail
        .turns
        .iter()
        .rev()
        .find(|turn| turn.status == TurnStatus::Active)
        .ok_or_else(|| "thread 没有可标记失败的 active turn".to_owned())?;
    let failed_turn = turn_from_record(record, active.sequence)?;
    let start = active
        .history_items
        .len()
        .min(failed_turn.history_items.len());
    let items = failed_turn.history_items[start..].to_vec();
    if !items.is_empty() {
        service.append_turn_items(
            &root,
            AppendTurnItemsCommand {
                thread_id: record.session_id.clone(),
                turn_id: active.turn_id.clone(),
                items,
                recorded_at_ms: failure.timestamp,
            },
        )?;
    }
    service.fail_turn(
        &root,
        FailTurnCommand {
            thread_id: record.session_id.clone(),
            turn_id: active.turn_id.clone(),
            failure: failure.clone(),
            recorded_at_ms: failure.timestamp,
        },
    )
}

fn turn_from_record(record: &RunSessionRecord, sequence: u64) -> Result<Turn, String> {
    let metadata = thread_turn_metadata_from_record(record);
    let token_usage_json = Some(
        serde_json::to_string(&metadata)
            .map_err(|_| "thread turn metadata 序列化失败".to_owned())?,
    );
    let active = record.failure.is_none() && record_has_unsettled_tool_call(record);
    let status = if record.failure.is_some() {
        TurnStatus::Failed
    } else if active {
        TurnStatus::Active
    } else {
        TurnStatus::Completed
    };
    Ok(Turn {
        turn_id: format!("{}-turn-{}", record.session_id, record.assistant_part_id),
        sequence,
        status,
        history_items: history_items_from_record(record, sequence),
        token_usage_json,
        started_at_ms: record.created_at_ms,
        completed_at_ms: record
            .failure
            .as_ref()
            .map(|failure| failure.timestamp)
            .or_else(|| (!active).then_some(record.created_at_ms)),
        interrupted_at_ms: None,
        failure: record.failure.clone(),
    })
}

fn thread_turn_metadata_from_record(record: &RunSessionRecord) -> ThreadTurnMetadata {
    let context = record.session_context.as_ref();
    ThreadTurnMetadata {
        usage: record.usage,
        assistant_part_id: record.assistant_part_id.clone(),
        latest_failure: context.and_then(|value| value.latest_failure.clone()),
        suggested_next_step: context.map(|value| value.suggested_next_step.clone()),
        tool_receipts: record
            .tool_receipts
            .iter()
            .map(tool_receipt_mirror)
            .collect(),
        reasoning_message_count: record.reasoning_messages.len(),
        turn_event_count: record.turn_events.len(),
        model_name: record.model_name.clone(),
        reflection_notice: record.reflection_notice.clone(),
        reasoning_effort: record.reasoning_effort.clone(),
    }
}

/// 上一轮的记忆反思提示：取当前轮之前最近一条带提示的轮次。
pub(super) fn previous_reflection_notice(
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

/// 只读接口用的入口：调用方手里是 Axon 根目录，不是配置文件路径。
pub(crate) fn latest_reflection_notice_at_root(root: &Path, session_id: &str) -> Option<String> {
    let detail = ThreadService::new(JsonlThreadEventStore)
        .read_thread(root, session_id)
        .ok()?;
    detail.turns.iter().rev().find_map(|turn| {
        turn_metadata(&turn.token_usage_json).and_then(|value| value.reflection_notice)
    })
}

/// 读取 thread 投影；读不到时读作「没有」，只读接口不因此失败。
fn read_thread_detail(config_path: &Path, session_id: &str) -> Option<ThreadDetail> {
    let root = resolved_axon_root(config_path).ok()?;
    ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, session_id)
        .ok()
}

#[cfg(test)]
#[path = "run_thread_store_tests.rs"]
mod tests;
