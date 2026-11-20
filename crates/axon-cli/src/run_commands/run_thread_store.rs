//! `axon run` 到统一 Thread 事件流的镜像。

use std::path::Path;
use std::time::Instant;

#[cfg(test)]
use axon_session::Thread;
use axon_session::{
    AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand, FailTurnCommand,
    InterruptTurnCommand, JsonlThreadEventStore, RepairOpenTurnCommand, StartEmptyThreadCommand,
    StartThreadCommand, StartTurnCommand, ThreadCompaction, ThreadDetail, ThreadService,
    ThreadSettings, ThreadSource, Turn, TurnRepairOutcome, TurnStatus,
};

use super::{
    run_session_store::{RunSessionRecord, resolved_axon_root},
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
    /// 这段对话所属的运行窗口（Thread 线程标识）。
    pub(super) thread_id: String,
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
            thread_id: detail.thread.thread_id.clone(),
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
    thread_id: &str,
    assistant_part_id: &str,
    expected_sequence: u64,
    mut compaction: ThreadCompaction,
) -> Result<u64, String> {
    let service = ThreadService::new(JsonlThreadEventStore);
    // 整个函数是一次换窗的 Thread I/O 总成本：定位当前轮（读整份 thread）加装投影
    // （整份快照重写）。采样覆盖两段，才能看出长会话上换窗究竟慢在哪里。
    let started = Instant::now();
    // `through_sequence` 取 thread 侧当前轮的序号：turn_id 由
    // `{thread_id}-turn-{assistant_part_id}` 组成，因此按后缀精确命中，找不到再退回活动轮。
    //
    // 入参必须是 Thread 事件流的键（`sessions.current_run_id`），不是会话 ULID：
    // 两者是不同的身份空间，拿会话 ULID 来读 Thread 只会得到「未找到 thread」。
    let detail = service.read_thread(root, thread_id)?;
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
    // 复用刚读到的 detail 安装投影：`compact_thread` 会再读一遍整份 thread，
    // 对 60 MiB 级的会话等于把同一份语料解析两次。
    let result = service.compact_thread_from_detail(
        root,
        CompactThreadCommand {
            thread_id: thread_id.to_owned(),
            recorded_at_ms: compaction.recorded_at_ms,
            expected_sequence,
            compaction,
        },
        detail,
    );
    crate::runtime_thread_io_metrics::record(
        root,
        crate::runtime_thread_io_metrics::OPERATION_COMPACT_THREAD,
        thread_id,
        started,
        result.is_ok(),
    );
    result?;
    Ok(through_sequence)
}

#[path = "run_thread_queries.rs"]
mod queries;
pub(crate) use queries::{
    LatestTurnFacts, RecentTurnFact, ThreadTurnMetadata, latest_turn_facts_at_root,
    latest_turn_usage_tokens_at_root, patch_turn_artifacts, previous_reflection_notice,
    recent_turn_facts, thread_turn_metadata_from_record, turn_record, turn_record_by_sequence,
    turn_sequence,
};

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

pub(super) fn tool_receipt_mirror(
    record_receipt: &super::run_execution::RunToolReceipt,
) -> serde_json::Value {
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
///
/// 采样覆盖整次收口（读整份 thread + 增量落盘）：这是每轮固定发生的 O(语料) 成本，
/// 长会话是否越用越慢就看这条曲线。
pub(super) fn mirror_session_to_thread(
    config_path: &Path,
    record: &RunSessionRecord,
) -> Result<(), String> {
    let started = Instant::now();
    let thread_id = record.thread_id.clone();
    let result = mirror_session_to_thread_inner(config_path, record);
    // 侧车与 thread 记录同一次收口落地：读侧因此不必为了三个标量再读大记录。
    if result.is_ok() {
        super::run_thread_facts::record_turn_facts(config_path, record);
    }
    if let Ok(root) = resolved_axon_root(config_path) {
        crate::runtime_thread_io_metrics::record(
            &root,
            crate::runtime_thread_io_metrics::OPERATION_MIRROR_TERMINAL,
            &thread_id,
            started,
            result.is_ok(),
        );
    }
    result
}

fn mirror_session_to_thread_inner(
    config_path: &Path,
    record: &RunSessionRecord,
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    let turn = turn_from_record(record, 0)?;
    match service.read_thread(&root, &record.thread_id) {
        Ok(detail) => {
            mirror_existing_turn(&service, &root, &record.thread_id, &detail, turn, record)
        }
        Err(_) if turn.status == TurnStatus::Active => {
            service.start_empty_thread(
                &root,
                StartEmptyThreadCommand {
                    thread_id: record.thread_id.clone(),
                    session_root_id: record.thread_id.clone(),
                    parent_thread_id: record.parent_thread_id.clone(),
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
                    thread_id: record.thread_id.clone(),
                    recorded_at_ms: record.created_at_ms,
                    turn,
                },
            )
        }
        Err(_) => service.start_thread(
            &root,
            StartThreadCommand {
                thread_id: record.thread_id.clone(),
                session_root_id: record.thread_id.clone(),
                parent_thread_id: record.parent_thread_id.clone(),
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

    let activation = service.read_thread_activation(&root, &record.thread_id)?;
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
                    thread_id: record.thread_id.clone(),
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
                    record.thread_id
                )),
                Ok(TurnRepairOutcome::Balanced) => {}
                Err(error) => axon_logger::warning(&format!(
                    "Thread {} 遗留轮次收口失败，仍尝试开始新轮：{error}",
                    record.thread_id
                )),
            }
        }
        service.start_turn(
            &root,
            StartTurnCommand {
                thread_id: record.thread_id.clone(),
                recorded_at_ms: record.created_at_ms,
                turn,
            },
        )
    } else {
        service.start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: record.thread_id.clone(),
                session_root_id: record.thread_id.clone(),
                parent_thread_id: record.parent_thread_id.clone(),
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
                thread_id: record.thread_id.clone(),
                recorded_at_ms: record.created_at_ms,
                turn,
            },
        )
    }
}

/// 计算仍需要落盘的历史 item：身份是 `(类别, part_id)`，与投影侧 upsert 口径一致。
///
/// 增量落盘不能按位置切片：item 的生成顺序不是「只追加尾部」（工具回执插在调用之间、
/// reasoning 排在最后），切片会把回执错插到历史里并丢掉已有调用的结果。
pub(super) fn pending_turn_items(
    existing: &[axon_session::ThreadHistoryItem],
    projected: &[axon_session::ThreadHistoryItem],
) -> Vec<axon_session::ThreadHistoryItem> {
    projected
        .iter()
        .filter(|item| {
            !existing
                .iter()
                .any(|current| current.identity() == item.identity() && current == *item)
        })
        .cloned()
        .collect()
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

pub(super) fn mirror_existing_turn(
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
            let items = pending_turn_items(&active.history_items, &turn.history_items);
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
        // detail 已确认过这一轮：追加不再把整条 thread 读第二遍。
        return service.append_turn_at_tail(
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
    /*
     * 下一轮的序号取「当前最大序号 + 1」，不是「轮数」。
     *
     * thread 被治理重写之后，文件里只剩快照之后的窗口，序号从中间开始（例如 5..24），
     * 轮数却是 20；按轮数取号会与保留下来的 `sequence == 20` 那一轮撞号，进而让
     * `trace:<session>:<sequence>` 的标识、按序号的轮次定位与 affect 序列的先后判定
     * 全部指向错的轮次。
     */
    let sequence = detail
        .turns
        .iter()
        .map(|turn| turn.sequence)
        .max()
        .map_or(0, |last| last.saturating_add(1));
    service.append_turn_at_tail(
        root,
        AppendTurnCommand {
            thread_id: thread_id.to_owned(),
            recorded_at_ms: record.created_at_ms,
            turn: Turn { sequence, ..turn },
        },
    )
}

/// 从 SQLite SSOT 恢复一段运行窗口的上下文。
/// 一次完成恢复目标定位、SQLite turn 读取与模型历史装配。
///
/// 入参是本轮运行窗口（`sessions.current_run_id`）：窗口与会话的对应关系由标识解析给出，
/// 这里只按窗口读 Thread 事件流。
pub(super) fn prepare_run_session(
    config_path: &Path,
    identities: &super::RunIdentities,
) -> Result<Option<PreparedRunSession>, String> {
    let started = Instant::now();
    let thread_id = identities.run_session_id.clone();
    let result = prepare_run_session_inner(config_path, identities);
    if let Ok(root) = resolved_axon_root(config_path) {
        crate::runtime_thread_io_metrics::record(
            &root,
            crate::runtime_thread_io_metrics::OPERATION_PREPARE_RUN,
            &thread_id,
            started,
            result.is_ok(),
        );
    }
    result
}

fn prepare_run_session_inner(
    config_path: &Path,
    identities: &super::RunIdentities,
) -> Result<Option<PreparedRunSession>, String> {
    let run_session_id = identities.run_session_id.as_str();
    let root = resolved_axon_root(config_path)?;
    let service = ThreadService::new(JsonlThreadEventStore);
    // 「不存在」与「读不出来」必须分开：前者如实返回没有可恢复会话，后者是数据损坏，
    // 静默当成空会话会让用户先看到失忆、再在起跑时才看到报错。
    if JsonlThreadEventStore::thread_shard_paths(&root, run_session_id)?.is_empty() {
        return Ok(None);
    }
    // 崩溃遗留的 active turn 必须在装配模型历史之前收口：否则第一次 provider 请求会带着
    // 「有 tool_calls 却没有 tool 结果」的悬空历史，必然被拒一次。
    repair_open_turn_before_restore(&service, &root, run_session_id)?;
    let detail = service
        .read_thread(&root, run_session_id)
        .map_err(|error| format!("会话历史读取失败：{run_session_id}，{error}"))?;
    if detail.compacted_history.is_none()
        && window_number_exceeds_first(config_path, run_session_id)?
    {
        return Err(
            "会话已换窗但活动窗口投影缺失，拒绝恢复旧聊天历史；请先修复窗口投影".to_owned(),
        );
    }
    let facts = ResumedSessionFacts::from_detail(&detail);
    let model_history_json = thread_model_history_json(&detail)?;
    crate::turn_execution_ledger::restore_thread_history(&identities.session_id, &detail);
    Ok(Some(PreparedRunSession {
        facts,
        model_history_json,
    }))
}

/// 恢复前先把崩溃遗留的 active turn 收口：补合成工具结果并写终态。
///
/// 收口在装配历史之前执行，保证发往 provider 的第一条请求就已经是平衡历史；
/// 收口失败只告警不阻断（历史照旧装配，用户至少能看见会话并自行处理）。
fn repair_open_turn_before_restore(
    service: &ThreadService<JsonlThreadEventStore>,
    root: &Path,
    thread_id: &str,
) -> Result<(), String> {
    let activation = service.read_thread_activation(root, thread_id)?;
    let Some(turn_id) = activation.active_turn_id else {
        return Ok(());
    };
    match service.repair_open_turn(
        root,
        RepairOpenTurnCommand {
            thread_id: thread_id.to_owned(),
            turn_id,
            recorded_at_ms: super::run_execution::now_ms(),
            known_next_sequence: Some(activation.next_sequence),
        },
    ) {
        Ok(TurnRepairOutcome::Repaired {
            turn_id,
            synthetic_results,
        }) => axon_logger::warning(&format!(
            "Thread {thread_id} 崩溃遗留轮次 {turn_id} 已在恢复前收口，补合成工具结果 {synthetic_results} 条"
        )),
        Ok(TurnRepairOutcome::Balanced) => {}
        Err(error) => axon_logger::warning(&format!(
            "Thread {thread_id} 崩溃遗留轮次收口失败，按现有历史继续恢复：{error}"
        )),
    }
    Ok(())
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
pub(super) fn turn_metadata(token_usage_json: &Option<String>) -> Option<ThreadTurnMetadata> {
    let raw = token_usage_json.as_deref()?;
    serde_json::from_str::<ThreadTurnMetadata>(raw).ok()
}

#[cfg(test)]
use axon_session::ThreadStatus;

/// 测试夹具独立成文：主文件贴 700 行硬限，夹具不该占正文的行数。
#[cfg(test)]
#[path = "run_thread_store_test_support.rs"]
mod test_support;
#[cfg(test)]
pub(super) use test_support::thread_detail_from_records;

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
    let detail = service.read_thread(&root, &record.thread_id)?;
    let active = detail
        .turns
        .iter()
        .rev()
        .find(|turn| turn.status == TurnStatus::Active)
        .ok_or_else(|| "thread 没有可标记失败的 active turn".to_owned())?;
    let failed_turn = turn_from_record(record, active.sequence)?;
    // 与中途落盘同一口径：按身份 upsert 只补差量，绝不按位置切片。
    let items = pending_turn_items(&active.history_items, &failed_turn.history_items);
    if !items.is_empty() {
        service.append_turn_items(
            &root,
            AppendTurnItemsCommand {
                thread_id: record.thread_id.clone(),
                turn_id: active.turn_id.clone(),
                items,
                recorded_at_ms: failure.timestamp,
            },
        )?;
    }
    service.fail_turn(
        &root,
        FailTurnCommand {
            thread_id: record.thread_id.clone(),
            turn_id: active.turn_id.clone(),
            failure: failure.clone(),
            recorded_at_ms: failure.timestamp,
        },
    )
}

pub(super) fn turn_from_record(record: &RunSessionRecord, sequence: u64) -> Result<Turn, String> {
    let metadata = queries::thread_turn_metadata_from_record(record);
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
        turn_id: format!("{}-turn-{}", record.thread_id, record.assistant_part_id),
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

fn read_thread_detail(config_path: &Path, session_id: &str) -> Option<ThreadDetail> {
    let root = resolved_axon_root(config_path).ok()?;
    ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, session_id)
        .ok()
}

#[cfg(test)]
#[path = "run_thread_store_tests.rs"]
mod tests;
