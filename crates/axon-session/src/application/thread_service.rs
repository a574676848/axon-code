//! Thread 应用服务。

use std::path::Path;

use crate::{
    application::{
        THREAD_EVENT_SCHEMA_VERSION,
        projection::{project_threads, to_summary},
    },
    domain::{
        ThreadCompaction, ThreadDetail, ThreadEvent, ThreadEventRecord, ThreadId, ThreadListQuery,
        ThreadSettings, ThreadSource, ThreadSummary, ThreadSummaryPage, Turn,
    },
    ports::ThreadEventStore,
};

/// Thread service。
#[derive(Debug, Clone)]
pub struct ThreadService<S> {
    store: S,
}

impl<S> ThreadService<S>
where
    S: ThreadEventStore,
{
    /// 创建 service。
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    /// 创建空 thread。
    pub fn start_empty_thread(
        &self,
        root: &Path,
        command: StartEmptyThreadCommand,
    ) -> Result<(), String> {
        if command.ephemeral {
            return Ok(());
        }
        let activation = self
            .store
            .read_thread_activation(root, &command.thread_id)?;
        if activation.exists {
            return Ok(());
        }
        let sequence = activation.next_sequence;
        let start = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: command.thread_id.clone(),
            sequence,
            recorded_at_ms: command.created_at_ms,
            event: ThreadEvent::ThreadStarted {
                session_root_id: command.session_root_id,
                parent_thread_id: command.parent_thread_id,
                forked_from_thread_id: command.forked_from_thread_id,
                source: command.source,
                ephemeral: command.ephemeral,
                settings: command.settings,
            },
        };
        self.store.append(root, &start)
    }

    /// 创建 thread 并写入首个 turn。
    pub fn start_thread(&self, root: &Path, command: StartThreadCommand) -> Result<(), String> {
        if command.ephemeral {
            return Ok(());
        }
        let sequence = self.next_sequence(root, &command.thread_id)?;
        let start = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: command.thread_id.clone(),
            sequence,
            recorded_at_ms: command.created_at_ms,
            event: ThreadEvent::ThreadStarted {
                session_root_id: command.session_root_id,
                parent_thread_id: command.parent_thread_id,
                forked_from_thread_id: command.forked_from_thread_id,
                source: command.source,
                ephemeral: command.ephemeral,
                settings: command.settings,
            },
        };
        self.store.append(root, &start)?;
        let turn = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: command.thread_id,
            sequence: sequence + 1,
            recorded_at_ms: command
                .turn
                .completed_at_ms
                .unwrap_or(command.created_at_ms),
            event: ThreadEvent::TurnCompleted { turn: command.turn },
        };
        self.store.append(root, &turn)
    }

    /// 读取 thread。
    pub fn read_thread(&self, root: &Path, thread_id: &str) -> Result<ThreadDetail, String> {
        self.project_thread(root, thread_id)
    }

    /// 读取启动链路使用的轻量激活状态。
    pub fn read_thread_activation(
        &self,
        root: &Path,
        thread_id: &str,
    ) -> Result<crate::ports::ThreadActivationSnapshot, String> {
        self.store.read_thread_activation(root, thread_id)
    }

    /// 追加一个完成 turn。
    pub fn append_turn(&self, root: &Path, command: AppendTurnCommand) -> Result<(), String> {
        self.read_thread(root, &command.thread_id)?;
        self.append_simple(
            root,
            &command.thread_id,
            command
                .turn
                .completed_at_ms
                .unwrap_or(command.recorded_at_ms),
            ThreadEvent::TurnCompleted { turn: command.turn },
        )
    }

    /// 命名 thread。
    pub fn name_thread(&self, root: &Path, command: NameThreadCommand) -> Result<(), String> {
        self.read_thread(root, &command.thread_id)?;
        self.append_simple(
            root,
            &command.thread_id,
            command.recorded_at_ms,
            ThreadEvent::ThreadNamed { name: command.name },
        )
    }

    /// 标记一个 active turn。
    pub fn start_turn(&self, root: &Path, command: StartTurnCommand) -> Result<(), String> {
        let activation = self
            .store
            .read_thread_activation(root, &command.thread_id)?;
        if activation.active_turn_id.is_some() {
            return Err("thread 已有 active turn，不能重复开始".to_owned());
        }
        self.append_at_sequence(
            root,
            &command.thread_id,
            activation.next_sequence,
            command.recorded_at_ms,
            ThreadEvent::TurnStarted { turn: command.turn },
        )
    }

    /// 追加 active turn 的增量历史 item。
    pub fn append_turn_items(
        &self,
        root: &Path,
        command: AppendTurnItemsCommand,
    ) -> Result<(), String> {
        let detail = self.read_thread(root, &command.thread_id)?;
        let active =
            active_turn(&detail).ok_or_else(|| "thread 没有可追加的 active turn".to_owned())?;
        if active.turn_id != command.turn_id {
            return Err("追加历史的 turn_id 与 active turn 不匹配".to_owned());
        }
        self.append_simple(
            root,
            &command.thread_id,
            command.recorded_at_ms,
            ThreadEvent::TurnItemsAppended {
                turn_id: command.turn_id,
                items: command.items,
            },
        )
    }

    /// 将 active turn 标记为失败。
    pub fn fail_turn(&self, root: &Path, command: FailTurnCommand) -> Result<(), String> {
        let detail = self.read_thread(root, &command.thread_id)?;
        let active =
            active_turn(&detail).ok_or_else(|| "thread 没有可标记失败的 active turn".to_owned())?;
        if active.turn_id != command.turn_id {
            return Err("失败 turn_id 与 active turn 不匹配".to_owned());
        }
        self.append_simple(
            root,
            &command.thread_id,
            command.recorded_at_ms,
            ThreadEvent::TurnFailed {
                turn_id: command.turn_id,
                failure: command.failure,
            },
        )
    }

    /// 将 active turn 标记为中断。
    pub fn interrupt_turn(&self, root: &Path, command: InterruptTurnCommand) -> Result<(), String> {
        let activation = self
            .store
            .read_thread_activation(root, &command.thread_id)?;
        let Some(active_turn_id) = activation.active_turn_id else {
            return Err("thread 没有可中断的 active turn".to_owned());
        };
        if active_turn_id != command.turn_id {
            return Err("中断 turn_id 与 active turn 不匹配".to_owned());
        }
        self.append_at_sequence(
            root,
            &command.thread_id,
            activation.next_sequence,
            command.recorded_at_ms,
            ThreadEvent::TurnInterrupted {
                turn_id: command.turn_id,
            },
        )
    }

    /// 列出 thread 摘要。
    pub fn list_threads(&self, root: &Path, limit: usize) -> Result<Vec<ThreadSummary>, String> {
        Ok(self
            .list_thread_page(
                root,
                &ThreadListQuery {
                    limit,
                    ..ThreadListQuery::default()
                },
            )?
            .threads)
    }

    /// 分页列出 thread 摘要。
    pub fn list_thread_page(
        &self,
        root: &Path,
        query: &ThreadListQuery,
    ) -> Result<ThreadSummaryPage, String> {
        let records = self.store.read_all(root)?;
        self.list_thread_page_from_records(&records, query)
    }

    /// 基于已读取的事件分页投影 thread 摘要。
    pub fn list_thread_page_from_records(
        &self,
        records: &[ThreadEventRecord],
        query: &ThreadListQuery,
    ) -> Result<ThreadSummaryPage, String> {
        if query.limit == 0 {
            return Err("thread list limit 必须大于 0".to_owned());
        }
        let mut summaries = project_threads(records)
            .iter()
            .map(to_summary)
            .filter(|summary| thread_matches_query(summary, query))
            .collect::<Vec<_>>();
        sort_summaries(&mut summaries);
        let total_records = summaries.len();
        let after_cursor = query
            .cursor
            .as_deref()
            .map(parse_thread_cursor)
            .transpose()?;
        let start_index = after_cursor
            .and_then(|cursor| {
                summaries
                    .iter()
                    .position(|summary| summary_cursor(summary) == cursor)
                    .map(|index| index + 1)
            })
            .unwrap_or(0);
        let mut threads = summaries
            .into_iter()
            .skip(start_index)
            .take(query.limit.saturating_add(1))
            .collect::<Vec<_>>();
        let next_cursor = if threads.len() > query.limit {
            let _ = threads.pop();
            threads.last().map(encode_thread_cursor)
        } else {
            None
        };
        Ok(ThreadSummaryPage {
            total_records,
            next_cursor,
            threads,
        })
    }

    /// 恢复 thread。
    pub fn resume_thread(&self, root: &Path, command: &ResumeThreadCommand) -> Result<(), String> {
        let records = self.store.read_thread(root, &command.thread_id)?;
        project_thread_records(&records, &command.thread_id)?;
        let record = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: command.thread_id.clone(),
            sequence: next_sequence_from_records(&records),
            recorded_at_ms: command.recorded_at_ms,
            event: ThreadEvent::ThreadResumed,
        };
        self.store.append(root, &record)
    }

    /// 安装压缩后的上下文历史。
    pub fn compact_thread(&self, root: &Path, command: CompactThreadCommand) -> Result<(), String> {
        let mut detail = self.read_thread(root, &command.thread_id)?;
        detail.compacted_history = Some(command.compaction);
        detail.thread.updated_at_ms = command.recorded_at_ms;
        let checkpoint = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: command.thread_id.clone(),
            sequence: self.next_sequence(root, &command.thread_id)?,
            recorded_at_ms: command.recorded_at_ms,
            event: ThreadEvent::ThreadCheckpoint { detail },
        };
        self.store
            .replace_thread_with_checkpoint(root, &checkpoint, command.expected_sequence)
    }

    /// fork thread。
    pub fn fork_thread(&self, root: &Path, command: ForkThreadCommand) -> Result<ThreadId, String> {
        let mut source = self.read_thread(root, &command.source_thread_id)?;
        if let Some(turn) = active_turn(&source) {
            self.append_simple(
                root,
                &command.source_thread_id,
                command.recorded_at_ms,
                ThreadEvent::TurnInterrupted {
                    turn_id: turn.turn_id.clone(),
                },
            )?;
            source = self.read_thread(root, &command.source_thread_id)?;
        }
        if command.ephemeral {
            return Ok(command.new_thread_id);
        }
        let settings = command
            .settings
            .unwrap_or_else(|| source.thread.settings.clone());
        let start_sequence = self.next_sequence(root, &command.new_thread_id)?;
        let start = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: command.new_thread_id.clone(),
            sequence: start_sequence,
            recorded_at_ms: command.recorded_at_ms,
            event: ThreadEvent::ThreadStarted {
                session_root_id: source.thread.session_root_id.clone(),
                parent_thread_id: Some(source.thread.thread_id.clone()),
                forked_from_thread_id: Some(source.thread.thread_id.clone()),
                source: source.thread.source,
                ephemeral: command.ephemeral,
                settings,
            },
        };
        self.store.append(root, &start)?;
        self.append_simple(
            root,
            &command.new_thread_id,
            command.recorded_at_ms,
            ThreadEvent::ThreadForked {
                source_thread_id: command.source_thread_id,
            },
        )?;
        for (index, source_turn) in source.turns.iter().enumerate() {
            let mut turn = source_turn.clone();
            turn.sequence = u64::try_from(index).map_err(|_| "fork turn 数过大".to_owned())?;
            turn.turn_id = format!("{}-fork-turn-{}", command.new_thread_id, turn.sequence);
            self.append_simple(
                root,
                &command.new_thread_id,
                command
                    .recorded_at_ms
                    .saturating_add(turn.sequence)
                    .saturating_add(1),
                ThreadEvent::TurnCompleted { turn },
            )?;
        }
        Ok(command.new_thread_id)
    }

    fn project_thread(&self, root: &Path, thread_id: &str) -> Result<ThreadDetail, String> {
        let events = self.store.read_thread(root, thread_id)?;
        project_thread_records(&events, thread_id)
    }

    fn append_simple(
        &self,
        root: &Path,
        thread_id: &str,
        recorded_at_ms: u64,
        event: ThreadEvent,
    ) -> Result<(), String> {
        let record = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: thread_id.to_owned(),
            sequence: self.next_sequence(root, thread_id)?,
            recorded_at_ms,
            event,
        };
        self.store.append(root, &record)
    }

    fn append_at_sequence(
        &self,
        root: &Path,
        thread_id: &str,
        sequence: u64,
        recorded_at_ms: u64,
        event: ThreadEvent,
    ) -> Result<(), String> {
        let record = ThreadEventRecord {
            schema_version: THREAD_EVENT_SCHEMA_VERSION,
            thread_id: thread_id.to_owned(),
            sequence,
            recorded_at_ms,
            event,
        };
        self.store.append(root, &record)
    }

    fn next_sequence(&self, root: &Path, thread_id: &str) -> Result<u64, String> {
        self.store.next_sequence(root, thread_id)
    }
}

fn project_thread_records(
    records: &[ThreadEventRecord],
    thread_id: &str,
) -> Result<ThreadDetail, String> {
    project_threads(records)
        .into_iter()
        .find(|detail| detail.thread.thread_id == thread_id)
        .ok_or_else(|| format!("未找到 thread：{thread_id}"))
}

fn next_sequence_from_records(records: &[ThreadEventRecord]) -> u64 {
    records
        .iter()
        .map(|record| record.sequence)
        .max()
        .map_or(0, |sequence| sequence + 1)
}

fn sort_summaries(summaries: &mut [ThreadSummary]) {
    summaries.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.thread_id.cmp(&right.thread_id))
    });
}

fn thread_matches_query(summary: &ThreadSummary, query: &ThreadListQuery) -> bool {
    if let Some(cwd) = query.cwd.as_deref()
        && !cwd_matches(&summary.cwd, cwd)
    {
        return false;
    }
    if let Some(archived) = query.archived {
        return (summary.status == crate::domain::ThreadStatus::Archived) == archived;
    }
    true
}

fn cwd_matches(stored: &str, query: &str) -> bool {
    #[cfg(windows)]
    {
        normalize_windows_cwd(stored) == normalize_windows_cwd(query)
    }
    #[cfg(not(windows))]
    {
        stored == query
    }
}

#[cfg(windows)]
fn normalize_windows_cwd(value: &str) -> String {
    const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";
    const VERBATIM_PREFIX: &str = r"\\?\";
    const UNC_PREFIX: &str = r"\\";

    let normalized = value.replace('/', r"\");
    let without_verbatim = normalized.strip_prefix(VERBATIM_UNC_PREFIX).map_or_else(
        || {
            normalized
                .strip_prefix(VERBATIM_PREFIX)
                .unwrap_or(&normalized)
                .to_owned()
        },
        |remainder| format!("{UNC_PREFIX}{remainder}"),
    );
    without_verbatim.trim_end_matches('\\').to_lowercase()
}

fn encode_thread_cursor(summary: &ThreadSummary) -> String {
    format!("{}:{}", summary.updated_at_ms, summary.thread_id)
}

fn parse_thread_cursor(cursor: &str) -> Result<(u64, String), String> {
    let Some((updated_at_ms, thread_id)) = cursor.split_once(':') else {
        return Err("thread cursor 格式无效".to_owned());
    };
    let updated_at_ms = updated_at_ms
        .parse::<u64>()
        .map_err(|_| "thread cursor 时间无效".to_owned())?;
    if thread_id.trim().is_empty() {
        return Err("thread cursor 缺少 thread_id".to_owned());
    }
    Ok((updated_at_ms, thread_id.to_owned()))
}

fn summary_cursor(summary: &ThreadSummary) -> (u64, String) {
    (summary.updated_at_ms, summary.thread_id.clone())
}

fn active_turn(detail: &ThreadDetail) -> Option<&Turn> {
    detail
        .turns
        .iter()
        .rev()
        .find(|turn| turn.status == crate::domain::TurnStatus::Active)
}

/// 创建空 thread 命令。
#[derive(Debug, Clone)]
pub struct StartEmptyThreadCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 会话树根 ID。
    pub session_root_id: ThreadId,
    /// 父 thread ID。
    pub parent_thread_id: Option<ThreadId>,
    /// fork 来源 ID。
    pub forked_from_thread_id: Option<ThreadId>,
    /// 来源。
    pub source: ThreadSource,
    /// 是否仅内存态。
    pub ephemeral: bool,
    /// 设置。
    pub settings: ThreadSettings,
    /// 创建时间。
    pub created_at_ms: u64,
}

/// 创建 thread 命令。
#[derive(Debug, Clone)]
pub struct StartThreadCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 会话树根 ID。
    pub session_root_id: ThreadId,
    /// 父 thread ID。
    pub parent_thread_id: Option<ThreadId>,
    /// fork 来源 ID。
    pub forked_from_thread_id: Option<ThreadId>,
    /// 来源。
    pub source: ThreadSource,
    /// 是否仅内存态。
    pub ephemeral: bool,
    /// 设置。
    pub settings: ThreadSettings,
    /// 创建时间。
    pub created_at_ms: u64,
    /// 首个 turn。
    pub turn: Turn,
}

/// 追加 turn 命令。
#[derive(Debug, Clone)]
pub struct AppendTurnCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// Turn 数据。
    pub turn: Turn,
}

/// 命名 thread 命令。
#[derive(Debug, Clone)]
pub struct NameThreadCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// Thread 名称。
    pub name: Option<String>,
}

/// 开始 turn 命令。
#[derive(Debug, Clone)]
pub struct StartTurnCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// Turn 数据。
    pub turn: Turn,
}

/// 追加 active turn 历史 item 的命令。
#[derive(Debug, Clone)]
pub struct AppendTurnItemsCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// Active turn ID。
    pub turn_id: String,
    /// 新增历史 item。
    pub items: Vec<crate::domain::ThreadHistoryItem>,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// Turn 失败命令。
#[derive(Debug, Clone)]
pub struct FailTurnCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// Turn ID。
    pub turn_id: String,
    /// 结构化失败事实。
    pub failure: axon_domain::RuntimeFailure,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// Turn 中断命令。
#[derive(Debug, Clone)]
pub struct InterruptTurnCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// Turn ID。
    pub turn_id: String,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// 恢复命令。
#[derive(Debug, Clone)]
pub struct ResumeThreadCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// 压缩 thread 命令。
#[derive(Debug, Clone)]
pub struct CompactThreadCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 压缩开始时观察到的最新历史序列号。
    pub expected_sequence: u64,
    /// 压缩快照。
    pub compaction: ThreadCompaction,
}

/// Fork 命令。
#[derive(Debug, Clone)]
pub struct ForkThreadCommand {
    /// 来源 thread。
    pub source_thread_id: ThreadId,
    /// 新 thread。
    pub new_thread_id: ThreadId,
    /// 覆盖设置。
    pub settings: Option<ThreadSettings>,
    /// 是否仅内存态。
    pub ephemeral: bool,
    /// 记录时间。
    pub recorded_at_ms: u64,
}
