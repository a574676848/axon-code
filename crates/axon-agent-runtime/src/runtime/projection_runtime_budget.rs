//! 交互投影的主机级资源预算。
//!
//! TUI 与远控使用同一执行流，差异仅在投影协议。预算在进程内解析一次，避免每个
//! 端侧以孤立常量决定积压、合并与单条展示数据的内存占用。

use std::sync::OnceLock;

const MIN_LOGICAL_CORES: usize = 4;
const MAX_LOGICAL_CORES: usize = 16;
const MAILBOX_SLOTS_PER_CORE: usize = 8;
const MIN_SEMANTIC_MAILBOX_SLOTS: usize = 64;
const MAX_SEMANTIC_MAILBOX_SLOTS: usize = 128;
const TEXT_BYTES_PER_CORE: usize = 8 * 1024;
const MIN_COALESCED_TEXT_BYTES: usize = 32 * 1024;
const MAX_COALESCED_TEXT_BYTES: usize = 128 * 1024;
const DELTA_CHUNK_TARGET_BYTES: usize = 2 * 1024;
const MIN_DELTA_CHUNKS: usize = 16;
const MAX_DELTA_CHUNKS: usize = 64;
const CRITICAL_RESERVE_DIVISOR: usize = 16;
const MIN_CRITICAL_MAILBOX_RESERVE: usize = 2;
const MAX_CRITICAL_MAILBOX_RESERVE: usize = 8;
const STEER_ACTION_DIVISOR: usize = 16;
const MIN_STEER_ACTION_SLOTS: usize = 2;
const MAX_STEER_ACTION_SLOTS: usize = 8;
const REMOTE_MAILBOX_MULTIPLIER: usize = 4;
const MAX_REMOTE_MAILBOX_SLOTS: usize = 512;

/// 已解析的交互投影资源预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionRuntimeBudget {
    /// 本地交互端的语义 mailbox 槽位数。
    pub semantic_mailbox_slots: usize,
    /// 单个可合并展示事件的最大 UTF-8 字节数。
    pub coalesced_text_bytes: usize,
    /// 单个 TUI 批次允许保留的 delta 数量。
    pub delta_chunk_count: usize,
    /// 本地邮箱预留给审批和终态等不可降级事件的槽位数。
    pub critical_mailbox_reserve: usize,
    /// TUI 转向回执的独立有界槽位数。
    pub steer_action_slots: usize,
    /// 远端投影的语义 mailbox 槽位数。
    pub remote_mailbox_slots: usize,
}

impl ProjectionRuntimeBudget {
    /// 返回当前进程唯一的投影预算。
    #[must_use]
    pub fn current() -> Self {
        static BUDGET: OnceLock<ProjectionRuntimeBudget> = OnceLock::new();
        *BUDGET.get_or_init(|| Self::for_logical_cores(logical_cores()))
    }

    /// 用于测试和受控运行环境的预算推导。
    #[must_use]
    pub fn for_logical_cores(logical_cores: usize) -> Self {
        let cores = logical_cores.clamp(MIN_LOGICAL_CORES, MAX_LOGICAL_CORES);
        let semantic_mailbox_slots = cores
            .saturating_mul(MAILBOX_SLOTS_PER_CORE)
            .clamp(MIN_SEMANTIC_MAILBOX_SLOTS, MAX_SEMANTIC_MAILBOX_SLOTS);
        let coalesced_text_bytes = cores
            .saturating_mul(TEXT_BYTES_PER_CORE)
            .clamp(MIN_COALESCED_TEXT_BYTES, MAX_COALESCED_TEXT_BYTES);
        let delta_chunk_count = coalesced_text_bytes
            .saturating_div(DELTA_CHUNK_TARGET_BYTES)
            .clamp(MIN_DELTA_CHUNKS, MAX_DELTA_CHUNKS);
        let critical_mailbox_reserve = semantic_mailbox_slots
            .saturating_div(CRITICAL_RESERVE_DIVISOR)
            .clamp(MIN_CRITICAL_MAILBOX_RESERVE, MAX_CRITICAL_MAILBOX_RESERVE);
        let steer_action_slots = semantic_mailbox_slots
            .saturating_div(STEER_ACTION_DIVISOR)
            .clamp(MIN_STEER_ACTION_SLOTS, MAX_STEER_ACTION_SLOTS);
        let remote_mailbox_slots = semantic_mailbox_slots
            .saturating_mul(REMOTE_MAILBOX_MULTIPLIER)
            .min(MAX_REMOTE_MAILBOX_SLOTS);
        Self {
            semantic_mailbox_slots,
            coalesced_text_bytes,
            delta_chunk_count,
            critical_mailbox_reserve,
            steer_action_slots,
            remote_mailbox_slots,
        }
    }

    /// 从开头切出一个不拆分 UTF-8 字符的展示增量。
    #[must_use]
    pub fn split_text_chunk<'a>(&self, text: &'a str) -> (&'a str, &'a str) {
        if text.len() <= self.coalesced_text_bytes {
            return (text, "");
        }
        let mut boundary = self.coalesced_text_bytes;
        while !text.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }
        text.split_at(boundary)
    }
}

fn logical_cores() -> usize {
    std::thread::available_parallelism().map_or(MIN_LOGICAL_CORES, usize::from)
}

#[cfg(test)]
mod tests {
    use super::ProjectionRuntimeBudget;

    #[test]
    fn resource_budget_scales_with_host_parallelism_inside_global_bounds() {
        let small = ProjectionRuntimeBudget::for_logical_cores(1);
        let large = ProjectionRuntimeBudget::for_logical_cores(64);

        assert!(small.semantic_mailbox_slots <= large.semantic_mailbox_slots);
        assert!(small.coalesced_text_bytes <= large.coalesced_text_bytes);
        assert!(small.remote_mailbox_slots <= large.remote_mailbox_slots);
    }

    #[test]
    fn text_chunk_boundary_preserves_utf8() {
        let budget = ProjectionRuntimeBudget::for_logical_cores(4);
        let text = format!("{}中", "x".repeat(budget.coalesced_text_bytes));
        let (head, tail) = budget.split_text_chunk(&text);

        assert!(head.is_char_boundary(head.len()));
        assert_eq!(format!("{head}{tail}"), text);
    }
}
