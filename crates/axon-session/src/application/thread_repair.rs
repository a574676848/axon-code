//! Thread 侧未收口轮次的修复入口。
//!
//! 与用户主动中断分开：中断只写终态，修复会先把崩溃时半截的工具调用补成「结果未知」
//! 的回执，再与终态放进同一条事件里原子落地，历史因此重新平衡。
//!
//! 独立成文是因为 `thread_service` 已经贴着单文件行数上限，而这部分与主服务只共享
//! `append_at_sequence` 一个内部原语。

use std::path::Path;

use super::THREAD_EVENT_SCHEMA_VERSION;
use super::thread_service::{InterruptTurnCommand, ThreadService};
use crate::domain::{ThreadEvent, ThreadEventRecord};
use crate::ports::ThreadEventStore;

/// 修复未收口轮次的命令。
#[derive(Debug, Clone)]
pub struct RepairOpenTurnsCommand {
    /// Thread ID。
    pub thread_id: String,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 调用方已经读到的下一个可用序号。
    ///
    /// 给出时省掉一次反向扫日志（真实数据上均值数 MB）：`mirror_session_started` 这类
    /// 调用方本来就持有激活快照，再读一遍纯属浪费。留空则自行读取。
    pub known_next_sequence: Option<u64>,
}

/// 修复结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnRepairOutcome {
    /// 历史已经平衡，本次没有写入。
    Balanced,
    /// 已收口，并补齐了合成的工具结果。
    Repaired {
        /// 被收口的轮次，按发现顺序排列。
        turn_ids: Vec<String>,
        /// 补合成的工具结果条数。
        synthetic_results: usize,
    },
}

impl<S> ThreadService<S>
where
    S: ThreadEventStore,
{
    /// 带序号校验的追加：仅在当前末尾序号正好是 `sequence - 1` 时写入。
    ///
    /// 修复类低频路径用它避免「先读末尾再写」之间被并发写插队，把两条记录落成同一
    /// 序号。热路径仍走 `append_at_sequence`，不为多数情况付全量读的代价。
    pub(super) fn append_with_expected_sequence(
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
        self.store
            .append_at_expected_sequence(root, &record, sequence.saturating_sub(1))
    }

    /// 将 active turn 标记为中断，并补齐崩溃遗留的半截工具调用。
    ///
    /// 与 `repair_open_turns` 的区别：这条是「有人主动中断」的语义，没有 active turn
    /// 时报错是正确的；修复入口则把「本来就干净」当作成功。
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
        // 中断时把半截的工具调用一并补成「结果未知」：否则这条调用永远没有结果，
        // 下一轮装配仍会把悬空的 tool_call 发给 provider。
        let synthetic = self
            .read_thread(root, &command.thread_id)
            .ok()
            .and_then(|detail| {
                detail
                    .turns
                    .iter()
                    .find(|turn| turn.turn_id == command.turn_id)
                    .map(crate::domain::turn_repair::synthetic_tool_results)
            })
            .unwrap_or_default();
        self.append_at_sequence(
            root,
            &command.thread_id,
            activation.next_sequence,
            command.recorded_at_ms,
            ThreadEvent::TurnInterrupted {
                turn_id: command.turn_id,
                appended_items: synthetic,
            },
        )
    }

    /// 修复全部未收口轮次。
    ///
    /// 判据来自日志自身的结构：仍是 `Active`，就说明写它的进程没有走到收口。
    /// 收口**全部**残留而不是只收尾部：此前只修尾部一条，历史上更早留下的 Active 永远
    /// 清不掉，`start_turn` 随后一直拒绝，会话表现为「重发也起不来」。
    /// 已经平衡时返回 `Balanced` 且零写入，因此可重复调用。
    pub fn repair_open_turns(
        &self,
        root: &Path,
        command: RepairOpenTurnsCommand,
    ) -> Result<TurnRepairOutcome, String> {
        // 先看激活快照：没有 active 就是平衡，不必付一次全量读的代价。
        let activation = self
            .store
            .read_thread_activation(root, &command.thread_id)?;
        if activation.active_turn_id.is_none() {
            return Ok(TurnRepairOutcome::Balanced);
        }
        let detail = self.read_thread(root, &command.thread_id)?;
        let open = crate::domain::turn_repair::open_turns(&detail.turns);
        if open.is_empty() {
            return Ok(TurnRepairOutcome::Balanced);
        }
        let mut next_sequence = command
            .known_next_sequence
            .unwrap_or(activation.next_sequence);
        let mut turn_ids = Vec::with_capacity(open.len());
        let mut synthetic_results = 0_usize;
        for turn in open {
            let synthetic = crate::domain::turn_repair::synthetic_tool_results(turn);
            synthetic_results += synthetic.len();
            // 走带序号校验的追加：收口与「用户重新开始一轮」可能并发，普通追加会让两条
            // 记录落成同一序号，投影时同时生效，反而制造出两个 active 轮。
            self.append_with_expected_sequence(
                root,
                &command.thread_id,
                next_sequence,
                command.recorded_at_ms,
                ThreadEvent::TurnInterrupted {
                    turn_id: turn.turn_id.clone(),
                    appended_items: synthetic,
                },
            )?;
            next_sequence = next_sequence.saturating_add(1);
            turn_ids.push(turn.turn_id.clone());
        }
        Ok(TurnRepairOutcome::Repaired {
            turn_ids,
            synthetic_results,
        })
    }
}
