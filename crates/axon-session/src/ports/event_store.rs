//! Thread 事件存储端口。

use std::path::Path;

use crate::domain::ThreadEventRecord;

/// Thread 启动链路使用的轻量激活快照。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadActivationSnapshot {
    /// 事件流中是否存在 thread 起始或 checkpoint。
    pub exists: bool,
    /// 下一个可写入事件序号。
    pub next_sequence: u64,
    /// 当前 active turn；正常聚合最多存在一个。
    pub active_turn_id: Option<String>,
}

impl ThreadActivationSnapshot {
    /// 从完整事件流推导激活状态；默认端口实现和兜底路径共用。
    pub fn from_records(records: &[ThreadEventRecord]) -> Self {
        let latest_sequence = records.iter().map(|record| record.sequence).max();
        let mut active_turn_ids = Vec::new();
        for record in records {
            apply_activation_event(&mut active_turn_ids, record);
        }
        Self {
            exists: latest_sequence.is_some(),
            next_sequence: latest_sequence.map_or(0, |sequence| sequence + 1),
            active_turn_id: active_turn_ids.last().cloned(),
        }
    }
}

fn apply_activation_event(active_turn_ids: &mut Vec<String>, record: &ThreadEventRecord) {
    use crate::domain::{ThreadEvent, TurnStatus};

    if record.thread_id.is_empty() {
        return;
    }
    match &record.event {
        ThreadEvent::ThreadStarted { .. } => {
            active_turn_ids.clear();
        }
        ThreadEvent::ThreadCheckpoint { detail } => {
            *active_turn_ids = detail
                .turns
                .iter()
                .filter(|turn| turn.status == TurnStatus::Active)
                .map(|turn| turn.turn_id.clone())
                .collect();
        }
        ThreadEvent::TurnStarted { turn } => {
            active_turn_ids.retain(|active| active != &turn.turn_id);
            active_turn_ids.push(turn.turn_id.clone());
        }
        ThreadEvent::TurnItemsAppended { .. } => {}
        ThreadEvent::TurnCompleted { turn } => {
            if turn.status == TurnStatus::Active {
                if !active_turn_ids.contains(&turn.turn_id) {
                    active_turn_ids.push(turn.turn_id.clone());
                }
            } else {
                active_turn_ids.retain(|active| active != &turn.turn_id);
            }
        }
        ThreadEvent::TurnInterrupted { turn_id } | ThreadEvent::TurnFailed { turn_id, .. } => {
            active_turn_ids.retain(|active| active != turn_id);
        }
        ThreadEvent::ThreadResumed
        | ThreadEvent::ThreadCompacted { .. }
        | ThreadEvent::ThreadForked { .. }
        | ThreadEvent::ThreadNamed { .. }
        | ThreadEvent::ThreadArchived
        | ThreadEvent::ThreadUnarchived
        | ThreadEvent::ThreadClosed => {}
    }
}

/// Thread 事件存储。
pub trait ThreadEventStore {
    /// 追加一条事件。
    fn append(&self, root: &Path, record: &ThreadEventRecord) -> Result<(), String>;

    /// 用完整 checkpoint 替换指定 thread 的历史事件。
    fn replace_thread_with_checkpoint(
        &self,
        root: &Path,
        checkpoint: &ThreadEventRecord,
        expected_sequence: u64,
    ) -> Result<(), String> {
        let current_sequence = self
            .read_thread(root, &checkpoint.thread_id)?
            .last()
            .map_or(0, |record| record.sequence);
        if current_sequence != expected_sequence {
            return Err(format!(
                "thread history version conflict: expected={} actual={}",
                expected_sequence, current_sequence
            ));
        }
        self.append(root, checkpoint)
    }

    /// 读取全部事件。
    fn read_all(&self, root: &Path) -> Result<Vec<ThreadEventRecord>, String>;

    /// 读取指定 thread 的事件；旧实现默认从全量事件中过滤。
    fn read_thread(&self, root: &Path, thread_id: &str) -> Result<Vec<ThreadEventRecord>, String> {
        Ok(self
            .read_all(root)?
            .into_iter()
            .filter(|record| record.thread_id == thread_id)
            .collect())
    }

    /// 读取启动链路所需的最小激活状态。
    fn read_thread_activation(
        &self,
        root: &Path,
        thread_id: &str,
    ) -> Result<ThreadActivationSnapshot, String> {
        Ok(ThreadActivationSnapshot::from_records(
            &self.read_thread(root, thread_id)?,
        ))
    }

    /// 获取指定 thread 的下一个事件序列号。
    fn next_sequence(&self, root: &Path, thread_id: &str) -> Result<u64, String> {
        Ok(self.read_thread_activation(root, thread_id)?.next_sequence)
    }
}
