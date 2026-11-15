//! Thread 侧未收口轮次的修复入口。
//!
//! 与用户主动中断分开：中断只写终态，修复会先把崩溃时半截的工具调用补成「结果未知」
//! 的回执，再与终态放进同一条事件里原子落地，历史因此重新平衡。
//!
//! 独立成文是因为 `thread_service` 已经贴着单文件行数上限，而这部分与主服务只共享
//! `append_at_sequence` 一个内部原语。

use std::path::Path;

use super::thread_service::{RepairOpenTurnCommand, ThreadService, TurnRepairOutcome};
use crate::domain::ThreadEvent;
use crate::ports::ThreadEventStore;

impl<S> ThreadService<S>
where
    S: ThreadEventStore,
{
    /// 修复一个未收口的尾部轮次。
    ///
    /// 判据来自日志自身的结构：尾部轮次仍是 `Active`，说明写它的进程没有走到收口。
    /// 已经平衡时返回 `Balanced` 且零写入，因此可重复调用；`turn_id` 不匹配则报错，
    /// 避免修复期间新开的健康轮次被误伤。
    pub fn repair_open_turn(
        &self,
        root: &Path,
        command: RepairOpenTurnCommand,
    ) -> Result<TurnRepairOutcome, String> {
        let activation = self.read_thread_activation(root, &command.thread_id)?;
        let Some(active_turn_id) = activation.active_turn_id else {
            return Ok(TurnRepairOutcome::Balanced);
        };
        if active_turn_id != command.turn_id {
            return Err("修复 turn_id 与当前 active turn 不匹配".to_owned());
        }
        let detail = self.read_thread(root, &command.thread_id)?;
        let Some(turn) = crate::domain::turn_repair::open_tail_turn(&detail.turns) else {
            return Ok(TurnRepairOutcome::Balanced);
        };
        let synthetic = crate::domain::turn_repair::synthetic_tool_results(turn);
        let synthetic_results = synthetic.len();
        self.append_at_sequence(
            root,
            &command.thread_id,
            activation.next_sequence,
            command.recorded_at_ms,
            ThreadEvent::TurnInterrupted {
                turn_id: command.turn_id.clone(),
                appended_items: synthetic,
            },
        )?;
        Ok(TurnRepairOutcome::Repaired {
            turn_id: command.turn_id,
            synthetic_results,
        })
    }
}
