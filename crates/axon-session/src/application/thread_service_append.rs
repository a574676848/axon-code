//! Thread 事件追加：序号分配与「不做前置校验」的低成本追加入口。
//!
//! 从 `thread_service` 拆出：那一层负责语义（开始/收口/中断/压缩），本层只管把一条
//! 事件按下一个序号落盘。收口与治理回写在热路径上，因此这里刻意不读整条 thread。

use std::path::Path;

use crate::application::THREAD_EVENT_SCHEMA_VERSION;
use crate::domain::{ThreadEvent, ThreadEventRecord};

use crate::ports::ThreadEventStore;

use super::thread_service::ThreadService;

impl<S> ThreadService<S>
where
    S: ThreadEventStore,
{
    /// 追加一个完成 turn，不做整条读回的前置校验。
    ///
    /// 只给「已经从尾窗确认过该轮存在」的调用方用（治理回写）：`append_turn` 的前置
    /// `read_thread` 与历史长度成正比，实测单会话 80 MiB，一条回写要付两次全量反序列化。
    pub fn append_turn_at_tail(
        &self,
        root: &Path,
        command: crate::application::AppendTurnCommand,
    ) -> Result<(), String> {
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

    /// 追加一条轮次 metadata 增量。
    ///
    /// 治理回写只补两项事实（反思提示、affect 序列），不复制整份轮次记录：实测一份轮次
    /// 记录 676 KB 到数 MB，追加增量把每次收口的写放大从「整条记录」压到「一行小事件」。
    pub fn append_turn_metadata_patch(
        &self,
        root: &Path,
        thread_id: &str,
        recorded_at_ms: u64,
        assistant_part_id: &str,
        notice: Option<&str>,
        affect_sequence: &[String],
    ) -> Result<(), String> {
        self.append_simple(
            root,
            thread_id,
            recorded_at_ms,
            ThreadEvent::TurnMetadataPatched {
                assistant_part_id: assistant_part_id.to_owned(),
                notice: notice.map(str::to_owned),
                affect_sequence: affect_sequence.to_vec(),
            },
        )
    }

    pub(super) fn append_simple(
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

    pub(super) fn append_at_sequence(
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

    pub(super) fn next_sequence(&self, root: &Path, thread_id: &str) -> Result<u64, String> {
        self.store.next_sequence(root, thread_id)
    }
}
