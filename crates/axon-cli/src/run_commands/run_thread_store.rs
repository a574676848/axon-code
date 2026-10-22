//! `axon run` 到统一 Thread 事件流的镜像。

use std::path::Path;

use axon_session::{
    AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand, FailTurnCommand,
    InterruptTurnCommand, JsonlThreadEventStore, StartEmptyThreadCommand, StartThreadCommand,
    StartTurnCommand, Thread, ThreadCompaction, ThreadDetail, ThreadService, ThreadSettings,
    ThreadSource, ThreadStatus, Turn, TurnStatus,
};
use serde::Serialize;

use super::{
    RunCommand,
    run_rollout_store::{
        RolloutHistoryWindow, install_rollout_compaction, load_rollout_history_window,
        load_rollout_history_window_from_root, restore_rollout_compaction,
    },
    run_session_store::{
        RunSessionIndexLookup, RunSessionRecord, load_indexed_run_session, resolved_axon_root,
    },
    run_thread_history::{history_items_from_record, record_has_unsettled_tool_call},
};
#[path = "run_thread_model_history.rs"]
mod model_history;
pub(super) use model_history::thread_model_history_json;

#[derive(Debug, Clone)]
pub(super) struct PreparedRunSession {
    pub(super) record: RunSessionRecord,
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

pub(super) fn install_compaction_projection(
    config_path: &Path,
    root: &Path,
    session_id: &str,
    expected_thread_event_sequence: u64,
    compaction: ThreadCompaction,
    previous_compaction: Option<&ThreadCompaction>,
) -> Result<(), String> {
    install_rollout_compaction(config_path, session_id, &compaction)?;
    if let Err(error) = ThreadService::new(JsonlThreadEventStore).compact_thread(
        root,
        CompactThreadCommand {
            thread_id: session_id.to_owned(),
            recorded_at_ms: compaction.recorded_at_ms,
            expected_sequence: expected_thread_event_sequence,
            compaction,
        },
    ) {
        let rollback = restore_rollout_compaction(config_path, session_id, previous_compaction);
        return Err(match rollback {
            Ok(()) => format!(
                "Thread 压缩投影失败，已回滚 rollout：session_id={session_id}，error={error}"
            ),
            Err(rollback_error) => format!(
                "Thread 压缩投影失败且 rollout 回滚失败：session_id={session_id}，projection={error}，rollback={rollback_error}"
            ),
        });
    }
    Ok(())
}

pub(super) fn install_active_compaction_projection(
    config_path: &Path,
    root: &Path,
    session_id: &str,
    assistant_part_id: &str,
    expected_sequence: u64,
    compaction: ThreadCompaction,
) -> Result<u64, String> {
    let (compaction, previous) = super::run_rollout_store::install_active_rollout_window(
        config_path,
        session_id,
        assistant_part_id,
        compaction,
    )?;
    let through_sequence = compaction.through_sequence;
    if let Err(error) = ThreadService::new(JsonlThreadEventStore).compact_thread(
        root,
        CompactThreadCommand {
            thread_id: session_id.to_owned(),
            recorded_at_ms: compaction.recorded_at_ms,
            expected_sequence,
            compaction,
        },
    ) {
        let rollback = restore_rollout_compaction(config_path, session_id, previous.as_ref());
        return Err(match rollback {
            Ok(()) => format!("活动窗口投影失败，rollout 已回滚：{error}"),
            Err(rollback_error) => format!(
                "活动窗口投影及 rollout 回滚失败：projection={error}，rollback={rollback_error}"
            ),
        });
    }
    Ok(through_sequence)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadTurnMetadata {
    usage: Option<axon_agent_runtime::TokenUsageTelemetry>,
    assistant_part_id: String,
    latest_failure: Option<String>,
    suggested_next_step: Option<String>,
    tool_receipts: Vec<serde_json::Value>,
    reasoning_message_count: usize,
    turn_event_count: usize,
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
        // 该 turn 已不可能再完成，必须先中断，否则同一会话的新 turn 会被永久拒绝。
        if let Some(stale) = activation.active_turn_id {
            axon_logger::warning(&format!(
                "Thread {} 存在未收口的 active turn {}，判定为进程中断遗留并自动中断",
                record.session_id, stale
            ));
            service.interrupt_turn(
                &root,
                InterruptTurnCommand {
                    thread_id: record.session_id.clone(),
                    turn_id: stale,
                    recorded_at_ms: record.created_at_ms,
                },
            )?;
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
    let Some(record) = load_indexed_run_session(config_path, &lookup)? else {
        return Ok(None);
    };
    let window = load_rollout_history_window(config_path, &record.session_id)?;
    let detail = thread_detail_from_history_window(&record, window)?;
    crate::turn_execution_ledger::restore_thread_history(&record.session_id, &detail);
    if detail.compacted_history.is_none() {
        let session_dir = resolved_axon_root(config_path)?
            .join("sessions")
            .join(&record.session_id);
        if axon_agent_runtime::SessionNotes::load_from_session(&session_dir)
            .map_err(|error| format!("读取窗口身份失败：{error:?}"))?
            .is_some_and(|notes| notes.window_number > 1)
        {
            return Err(
                "会话已换窗但活动窗口投影缺失，拒绝恢复旧聊天历史；请先修复窗口投影".to_owned(),
            );
        }
    }
    let model_history_json = thread_model_history_json(&detail)?;
    Ok(Some(PreparedRunSession {
        record,
        model_history_json,
    }))
}

pub(super) fn load_thread_detail(root: &Path, thread_id: &str) -> Result<ThreadDetail, String> {
    let window = load_rollout_history_window_from_root(root, thread_id)?;
    let latest = window
        .turns
        .last()
        .map(|(_, record)| record.clone())
        .or_else(|| {
            load_thread_session_from_root(root, thread_id)
                .ok()
                .flatten()
        })
        .ok_or_else(|| format!("未找到 SQLite thread：{thread_id}"))?;
    thread_detail_from_history_window(&latest, window)
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
                .map(|sequence| (sequence, record))
                .map_err(|_| "SQLite thread turn 数过大".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    thread_detail_from_history_window(
        fallback,
        RolloutHistoryWindow {
            compaction: None,
            turns,
        },
    )
}

fn thread_detail_from_history_window(
    fallback: &RunSessionRecord,
    window: RolloutHistoryWindow,
) -> Result<ThreadDetail, String> {
    let latest = window.turns.last().map_or(fallback, |(_, record)| record);
    let turns = window
        .turns
        .iter()
        .map(|(sequence, record)| turn_from_record(record, *sequence))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ThreadDetail {
        thread: Thread {
            thread_id: latest.session_id.clone(),
            session_root_id: fallback.session_id.clone(),
            parent_thread_id: latest.parent_session_id.clone(),
            forked_from_thread_id: None,
            name: None,
            status: ThreadStatus::Idle,
            source: ThreadSource::CliRun,
            ephemeral: false,
            settings: settings_from_record(latest),
            created_at_ms: fallback.created_at_ms,
            updated_at_ms: latest.created_at_ms,
            archived_at_ms: None,
        },
        turns,
        compacted_history: window.compaction,
    })
}

fn load_thread_session_from_root(
    root: &Path,
    thread_id: &str,
) -> Result<Option<RunSessionRecord>, String> {
    let path =
        axon_session::session_store_path(root).join(super::run_rollout_store::CLI_ROLLOUT_DB_FILE);
    super::run_rollout_store::RolloutSqliteStore::open(&path)?.load_by_id(thread_id)
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
    }
}

#[cfg(test)]
#[path = "run_thread_store_compaction_projection_tests.rs"]
mod compaction_projection_tests;
#[cfg(test)]
#[path = "run_thread_store_tests.rs"]
mod tests;
