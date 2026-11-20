//! Thread 事件追加：序号分配与「不做前置校验」的低成本追加入口。
//!
//! 从 `thread_service` 拆出：那一层负责语义（开始/收口/中断/压缩），本层只管把一条
//! 事件按下一个序号落盘。收口与治理回写在热路径上，因此这里刻意不读整条 thread。

use std::path::Path;

use crate::application::THREAD_EVENT_SCHEMA_VERSION;
use crate::domain::{ThreadEvent, ThreadEventRecord, TurnStatus};

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

impl ThreadService<crate::JsonlThreadEventStore> {
    /// 把 thread 重写为「会话头 + 最近一次压缩快照 + 最近 `keep_turns` 轮」。
    ///
    /// 为什么需要它：thread 是 append-only JSONL，一轮一条记录，长会话会无限增长，
    /// 尾窗读、备份与启动恢复都跟着变慢。这里把已被压缩快照覆盖的旧轮次从文件里真正
    /// 去掉，模型可见历史由快照承担，因此**不丢上下文**；没有压缩快照时一律不动，
    /// 因为删掉旧轮次就等于删掉模型历史。
    ///
    /// 窗口下界只由模型可见性决定：**只删 `sequence < through_sequence` 的旧轮次**，
    /// 也就是已经并入压缩快照的那一段。`keep_turns` 只当触发阈值与轮次上限目标用：
    /// 快照之后仍有超过 `keep_turns` 轮可见时，多出来的必须留下，否则删掉的是模型
    /// 还看得见的历史。
    ///
    /// 返回是否真的重写：轮数未超阈值、或还没有压缩快照时返回 `false`。
    pub fn rewrite_to_window(
        &self,
        root: &Path,
        thread_id: &str,
        keep_turns: usize,
    ) -> Result<bool, String> {
        let Some((records, _dropped, expected_sequence)) =
            self.window_records(root, thread_id, keep_turns)?
        else {
            return Ok(false);
        };
        self.store
            .rewrite_thread_with_records(root, thread_id, expected_sequence, &records)?;
        Ok(true)
    }

    /// 只算账不落盘：返回将被移出文件的轮次数；不需要重写时返回 `None`。
    pub fn plan_thread_window(
        &self,
        root: &Path,
        thread_id: &str,
        keep_turns: usize,
    ) -> Result<Option<usize>, String> {
        Ok(self
            .window_records(root, thread_id, keep_turns)?
            .map(|(_, dropped, _)| dropped))
    }

    /// 构造窗口重写后的完整记录集；不满足重写条件时返回 `None`。
    ///
    /// 返回的第三项是读快照时的末条事件序号：写路径拿它做乐观校验，期间有人追加就
    /// 撤销本次重写（读-改-写不允许覆盖别人的写入）。
    fn window_records(
        &self,
        root: &Path,
        thread_id: &str,
        keep_turns: usize,
    ) -> Result<Option<(Vec<ThreadEventRecord>, usize, u64)>, String> {
        let (detail, expected_sequence) = self.project_thread_with_tail(root, thread_id)?;
        if detail.turns.len() <= keep_turns {
            return Ok(None);
        }
        // 有在跑的一轮时一律不动：重写只重发终态，正在跑的那一轮会被写成终态。
        if detail
            .turns
            .iter()
            .any(|turn| turn.status == TurnStatus::Active)
        {
            return Ok(None);
        }
        let Some(compaction) = detail.compacted_history.clone() else {
            return Ok(None);
        };
        let visible_from = detail
            .turns
            .iter()
            .position(|turn| turn.sequence >= compaction.through_sequence)
            .unwrap_or(detail.turns.len());
        // 只删快照已覆盖的那一段：索引 >= visible_from 的轮次都还在模型窗口里。
        let keep_from = visible_from;
        if keep_from == 0 {
            return Ok(None);
        }
        let thread = &detail.thread;
        let mut events: Vec<(u64, ThreadEvent)> = Vec::with_capacity(keep_turns.saturating_add(6));
        events.push((
            thread.created_at_ms,
            ThreadEvent::ThreadStarted {
                session_root_id: thread.session_root_id.clone(),
                parent_thread_id: thread.parent_thread_id.clone(),
                forked_from_thread_id: thread.forked_from_thread_id.clone(),
                source: thread.source,
                ephemeral: thread.ephemeral,
                settings: thread.settings.clone(),
            },
        ));
        events.push((
            compaction.recorded_at_ms,
            ThreadEvent::ThreadCompacted { compaction },
        ));
        /*
         * 会话级事实要原样重发。
         *
         * 名称、子会话来源与归档状态都不在 `ThreadStarted` 里，而是各自的独立事件：
         * 只重发轮次会让这些字段在重写后凭空消失，而派生索引里还留着旧值，事实源与
         * 缓存就此分叉（重建索引会把它们永久抹掉）。
         */
        if let Some(name) = thread.name.clone() {
            events.push((
                thread.updated_at_ms,
                ThreadEvent::ThreadNamed { name: Some(name) },
            ));
        }
        if let Some(child_origin) = thread.child_origin.clone() {
            events.push((
                thread.updated_at_ms,
                ThreadEvent::ThreadAnnotated {
                    child_origin: Some(child_origin),
                },
            ));
        }
        if thread.status == crate::domain::ThreadStatus::Archived {
            events.push((thread.updated_at_ms, ThreadEvent::ThreadArchived));
        }
        for turn in &detail.turns[keep_from..] {
            events.push((
                turn.completed_at_ms
                    .or(turn.interrupted_at_ms)
                    .unwrap_or(turn.started_at_ms),
                ThreadEvent::TurnCompleted { turn: turn.clone() },
            ));
        }
        let records = events
            .into_iter()
            .enumerate()
            .map(|(offset, (recorded_at_ms, event))| ThreadEventRecord {
                schema_version: THREAD_EVENT_SCHEMA_VERSION,
                thread_id: thread_id.to_owned(),
                sequence: u64::try_from(offset).unwrap_or(0),
                recorded_at_ms,
                event,
            })
            .collect();
        Ok(Some((records, keep_from, expected_sequence)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JsonlThreadEventStore;
    use crate::application::{AppendTurnCommand, CompactThreadCommand, StartEmptyThreadCommand};
    use crate::domain::{ThreadCompaction, ThreadSettings, ThreadSource, Turn, TurnStatus};

    fn settings() -> ThreadSettings {
        ThreadSettings {
            cwd: "/w".to_owned(),
            provider: "provider-a".to_owned(),
            model: "model-a".to_owned(),
            selected_skill_name: None,
        }
    }

    fn turn(sequence: u64) -> Turn {
        Turn {
            turn_id: format!("turn-{sequence}"),
            sequence,
            status: TurnStatus::Completed,
            history_items: Vec::new(),
            token_usage_json: None,
            started_at_ms: 1_000 + sequence,
            completed_at_ms: Some(2_000 + sequence),
            interrupted_at_ms: None,
            failure: None,
        }
    }

    fn seed(root: &Path, turns: u64) -> ThreadService<JsonlThreadEventStore> {
        let service = ThreadService::new(JsonlThreadEventStore);
        service
            .start_empty_thread(
                root,
                StartEmptyThreadCommand {
                    thread_id: "thread-1".to_owned(),
                    session_root_id: "thread-1".to_owned(),
                    parent_thread_id: None,
                    forked_from_thread_id: None,
                    source: ThreadSource::CliRun,
                    ephemeral: false,
                    settings: settings(),
                    created_at_ms: 10,
                },
            )
            .expect("建 thread");
        for sequence in 0..turns {
            service
                .append_turn(
                    root,
                    AppendTurnCommand {
                        thread_id: "thread-1".to_owned(),
                        recorded_at_ms: 100 + sequence,
                        turn: turn(sequence),
                    },
                )
                .expect("追加轮次");
        }
        service
    }

    fn compaction() -> ThreadCompaction {
        ThreadCompaction {
            history_json: "[]".to_owned(),
            through_sequence: 0,
            checkpoint_text: "已压缩".to_owned(),
            before_tokens: 1,
            after_tokens: 1,
            recorded_at_ms: 20,
        }
    }
    #[test]
    fn rewrite_keeps_recent_turns_and_the_compaction_snapshot() {
        let directory = axon_test_support::test_temp_path("thread-window-rewrite");
        let root = directory.as_path();
        let service = seed(root, 4);
        // 快照覆盖前两轮：窗口重写只允许删这一段，留下的正好是最近两轮。
        let mut snapshot = compaction();
        snapshot.through_sequence = 2;
        service
            .compact_thread(
                root,
                CompactThreadCommand {
                    thread_id: "thread-1".to_owned(),
                    recorded_at_ms: 30,
                    expected_sequence: 4,
                    compaction: snapshot,
                },
            )
            .expect("写压缩快照");

        assert!(
            service
                .rewrite_to_window(root, "thread-1", 2)
                .expect("重写"),
            "有压缩快照且超阈值时必须真的重写"
        );
        let detail = ThreadService::new(JsonlThreadEventStore)
            .read_thread(root, "thread-1")
            .expect("读回");
        assert_eq!(detail.turns.len(), 2, "只保留最近两轮");
        assert!(detail.compacted_history.is_some(), "压缩快照必须留下");
        assert_eq!(
            detail.thread.settings.provider, "provider-a",
            "会话头原样保留"
        );
    }

    #[test]
    fn rewrite_is_a_no_op_without_compaction_or_below_threshold() {
        let directory = axon_test_support::test_temp_path("thread-window-noop");
        let root = directory.as_path();
        let service = seed(root, 2);
        assert!(
            !service
                .rewrite_to_window(root, "thread-1", 4)
                .expect("轮数未超阈值"),
            "轮数未超阈值时不重写"
        );
        assert!(
            !service
                .rewrite_to_window(root, "thread-1", 1)
                .expect("缺压缩快照"),
            "没有压缩快照时宁可不动，删旧轮次等于删模型历史"
        );
    }

    #[test]
    fn rewrite_keeps_every_turn_the_snapshot_still_leaves_visible() {
        let directory = axon_test_support::test_temp_path("thread-window-snapshot-window");
        let root = directory.as_path();
        let service = seed(root, 30);
        let mut snapshot = compaction();
        // 快照只覆盖前 5 轮：其后 25 轮仍在模型窗口里，比 keep_turns=20 更多，必须全留。
        snapshot.through_sequence = 5;
        service
            .compact_thread(
                root,
                CompactThreadCommand {
                    thread_id: "thread-1".to_owned(),
                    recorded_at_ms: 30,
                    expected_sequence: 30,
                    compaction: snapshot,
                },
            )
            .expect("写压缩快照");

        assert!(
            service
                .rewrite_to_window(root, "thread-1", 20)
                .expect("重写"),
            "有可丢弃的旧轮次时应重写"
        );
        let detail = ThreadService::new(JsonlThreadEventStore)
            .read_thread(root, "thread-1")
            .expect("读回");
        assert_eq!(detail.turns.len(), 25, "窗口之外仍可见的轮次必须全部留下");
        assert_eq!(detail.turns[0].sequence, 5, "只删快照覆盖的前五轮");
        assert_eq!(
            detail
                .compacted_history
                .expect("快照必须留下")
                .through_sequence,
            5,
            "快照边界不动，恢复侧仍然保留 >= through_sequence 的轮次"
        );
    }

    #[test]
    fn rewrite_never_cuts_turns_still_visible_to_the_model() {
        let directory = axon_test_support::test_temp_path("thread-window-visible");
        let root = directory.as_path();
        let service = seed(root, 5);
        let mut snapshot = compaction();
        // 两个数字都卡在边界上：快照只覆盖到第 4 轮，而 keep_turns 比「快照之后的轮数」更大。
        snapshot.through_sequence = 4;
        service
            .compact_thread(
                root,
                CompactThreadCommand {
                    thread_id: "thread-1".to_owned(),
                    recorded_at_ms: 30,
                    expected_sequence: 5,
                    compaction: snapshot,
                },
            )
            .expect("写压缩快照");

        assert_eq!(
            service
                .plan_thread_window(root, "thread-1", 2)
                .expect("算账"),
            Some(4),
            "dry-run 与执行必须给出同一个丢弃轮数"
        );
        assert!(
            service
                .rewrite_to_window(root, "thread-1", 2)
                .expect("重写"),
            "有可丢弃的旧轮次时必须真的重写"
        );
        let detail = ThreadService::new(JsonlThreadEventStore)
            .read_thread(root, "thread-1")
            .expect("读回");
        assert_eq!(detail.turns.len(), 1, "只保留快照之后仍可见的一轮");
        assert_eq!(detail.turns[0].sequence, 4, "模型可见轮次不得被切掉");
    }
}
