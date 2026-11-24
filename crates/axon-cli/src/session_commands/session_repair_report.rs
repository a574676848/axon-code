//! 存量修复报告：JSON 与文本两条通道读同一份事实。
//!
//! 两条子能力的载荷形状不同，因此报告在序列化时二选一；共同点是「逐会话给出结论 +
//! 顶部给出合计」，文本通道只做投影，不另算一遍。

use serde::Serialize;

/// 一次修复的报告：两条子能力二选一。
pub(super) enum RepairReport {
    /// 序号重排。
    Reorder(Box<ReorderReport>),
    /// 归属回填。
    Attribute(Box<AttributeReport>),
}

impl RepairReport {
    /// 是否出现会话级失败。
    pub(super) fn failed(&self) -> bool {
        match self {
            Self::Reorder(report) => report.sessions.iter().any(|item| item.error.is_some()),
            Self::Attribute(report) => report.sessions.iter().any(|item| item.error.is_some()),
        }
    }

    /// 序列化成一行 JSON。
    pub(super) fn to_json_line(&self) -> Result<String, String> {
        let serialized = match self {
            Self::Reorder(report) => serde_json::to_string(report),
            Self::Attribute(report) => serde_json::to_string(report),
        };
        serialized
            .map(|line| format!("{line}\n"))
            .map_err(|_| "修复报告序列化失败".to_owned())
    }

    /// 文本通道。
    pub(super) fn to_text(&self) -> String {
        match self {
            Self::Reorder(report) => report.to_text(),
            Self::Attribute(report) => report.to_text(),
        }
    }
}

/// 序号重排报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderReport {
    /// 结论：`repair_dry_run` / `repair_applied` / `repair_failed`。
    pub(super) status: &'static str,
    /// 子能力名。
    pub(super) mode: &'static str,
    /// 是否只出清单。
    pub(super) dry_run: bool,
    /// 事件目录。
    pub(super) events_root: String,
    /// 扫过的会话日志数。
    pub(super) scanned_sessions: usize,
    /// 存在逆序的会话数。
    pub(super) dirty_sessions: usize,
    /// 全部会话的逆序处合计。
    pub(super) inversion_count: usize,
    /// 逐会话结论。
    pub(super) sessions: Vec<ReorderSessionReport>,
    /// 扫描过程中的告警（读不出来的日志等）。
    pub(super) warnings: Vec<String>,
}

/// 单个会话的重排结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderSessionReport {
    /// 会话标识。
    pub(super) session_id: String,
    /// 事件日志路径：`--apply` 时这就是被替换的目标文件。
    pub(super) log_path: String,
    /// 备份文件路径；未执行修复时为空。
    pub(super) backup_path: Option<String>,
    /// 日志里的事件条数。
    pub(super) event_count: usize,
    /// 逆序处（清单可能被截断，计数始终是真实值）。
    pub(super) inversions: Vec<InversionEntry>,
    /// 逆序处是否被截断。
    pub(super) inversions_truncated: bool,
    /// 是否真的重写过。
    pub(super) repaired: bool,
    /// 本会话失败原因；成功或 dry-run 时为空。
    pub(super) error: Option<String>,
}

/// 一处序号逆序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InversionEntry {
    /// 读序里的位置（从 1 起）。
    pub(super) position: usize,
    /// 前一帧序号。
    pub(super) previous_sequence: u64,
    /// 当前帧序号。
    pub(super) current_sequence: u64,
    /// 前一帧的事件类型。
    pub(super) previous_event_type: String,
    /// 当前帧的事件类型。
    pub(super) current_event_type: String,
    /// 前一帧的写入时刻。
    pub(super) previous_emitted_at_ms: u64,
    /// 当前帧的写入时刻。
    pub(super) current_emitted_at_ms: u64,
    /// 是否与前一条同号（同号不属于可自动重排的形态）。
    pub(super) duplicate: bool,
}

/// 归属回填报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AttributeReport {
    /// 结论：`repair_dry_run` / `repair_applied` / `repair_failed`。
    pub(super) status: &'static str,
    /// 子能力名。
    pub(super) mode: &'static str,
    /// 是否只出清单。
    pub(super) dry_run: bool,
    /// 事件目录。
    pub(super) events_root: String,
    /// 扫过的会话日志数。
    pub(super) scanned_sessions: usize,
    /// 可唯一确定、能回填的帧总数。
    pub(super) backfillable_frames: usize,
    /// 无法确定归属的帧总数。
    pub(super) undetermined_frames: usize,
    /// 逐会话结论。
    pub(super) sessions: Vec<AttributeSessionReport>,
    /// 扫描过程中的告警。
    pub(super) warnings: Vec<String>,
}

/// 单个会话的归属结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AttributeSessionReport {
    /// 会话标识。
    pub(super) session_id: String,
    /// 事件日志路径。
    pub(super) log_path: String,
    /// 日志里的事件条数。
    pub(super) event_count: usize,
    /// 无归属的帧数。
    pub(super) unattributed_frames: usize,
    /// 判定：`backfillable` / `undetermined` / `clean`。
    pub(super) verdict: &'static str,
    /// 推导出的归属交互标识；无法确定时为空。
    pub(super) interaction_id: Option<String>,
    /// 无法确定的原因；可回填时为空。
    pub(super) reason: Option<String>,
    /// 备份文件路径；未执行修复时为空。
    pub(super) backup_path: Option<String>,
    /// 是否真的回写过。
    pub(super) repaired: bool,
    /// 本会话失败原因；成功或 dry-run 时为空。
    pub(super) error: Option<String>,
}

/// 报告结论：dry-run 只出清单，执行态按是否出现失败分档。
pub(super) fn report_status(dry_run: bool, failed: bool) -> &'static str {
    if failed {
        "repair_failed"
    } else if dry_run {
        "repair_dry_run"
    } else {
        "repair_applied"
    }
}

impl ReorderReport {
    /// 文本通道：清单按会话展开，目标路径逐条打印。
    fn to_text(&self) -> String {
        let mut output = format!(
            "Axon debug session repair --reorder: {}{}\n",
            self.status,
            if self.dry_run {
                "（dry-run，未写任何文件）"
            } else {
                "（已执行修复）"
            }
        );
        output.push_str(&format!("- events_root: {}\n", self.events_root));
        output.push_str(&format!(
            "- scanned_sessions: {} dirty_sessions: {} inversion_count: {}\n",
            self.scanned_sessions, self.dirty_sessions, self.inversion_count
        ));
        if self.sessions.is_empty() {
            output.push_str("- 没有可扫描的事件日志\n");
        }
        for session in &self.sessions {
            output.push_str(&format!(
                "- session {}: events={} inversions={}\n",
                session.session_id,
                session.event_count,
                session.inversions.len()
            ));
            output.push_str(&format!("  - 目标日志: {}\n", session.log_path));
            for entry in &session.inversions {
                output.push_str(&format!(
                    "  - #{} 序号 {} -> {}{}（{} -> {}，{}ms -> {}ms）\n",
                    entry.position,
                    entry.previous_sequence,
                    entry.current_sequence,
                    if entry.duplicate { "（同号）" } else { "" },
                    entry.previous_event_type,
                    entry.current_event_type,
                    entry.previous_emitted_at_ms,
                    entry.current_emitted_at_ms
                ));
            }
            if session.inversions_truncated {
                output.push_str(&format!(
                    "  - 逆序处超过 {} 条，清单已截断，计数仍为真实值\n",
                    super::session_repair::REPAIR_EVIDENCE_LIMIT
                ));
            }
            if let Some(backup) = &session.backup_path {
                output.push_str(&format!("  - 备份文件: {backup}\n"));
            }
            output.push_str(&format!(
                "  - repaired: {}\n",
                if session.repaired { "true" } else { "false" }
            ));
            if let Some(error) = &session.error {
                output.push_str(&format!("  - error: {error}\n"));
            }
        }
        for warning in &self.warnings {
            output.push_str(&format!("- warning: {warning}\n"));
        }
        output
    }
}

impl AttributeReport {
    /// 文本通道：结论、推导归属与无法确定的原因一起给出。
    fn to_text(&self) -> String {
        let mut output = format!(
            "Axon debug session repair --attribute: {}{}\n",
            self.status,
            if self.dry_run {
                "（dry-run，未写任何文件）"
            } else {
                "（已执行修复）"
            }
        );
        output.push_str(&format!("- events_root: {}\n", self.events_root));
        output.push_str(&format!(
            "- scanned_sessions: {} backfillable_frames: {} undetermined_frames: {}\n",
            self.scanned_sessions, self.backfillable_frames, self.undetermined_frames
        ));
        if self.sessions.is_empty() {
            output.push_str("- 没有需要回填的子代理会话\n");
        }
        for session in &self.sessions {
            output.push_str(&format!(
                "- session {}: events={} unattributed={} verdict={}\n",
                session.session_id,
                session.event_count,
                session.unattributed_frames,
                session.verdict
            ));
            output.push_str(&format!("  - 目标日志: {}\n", session.log_path));
            if let Some(interaction_id) = &session.interaction_id {
                output.push_str(&format!("  - 推导归属: {interaction_id}\n"));
            }
            if let Some(reason) = &session.reason {
                output.push_str(&format!("  - 无法确定: {reason}\n"));
            }
            if let Some(backup) = &session.backup_path {
                output.push_str(&format!("  - 备份文件: {backup}\n"));
            }
            output.push_str(&format!(
                "  - repaired: {}\n",
                if session.repaired { "true" } else { "false" }
            ));
            if let Some(error) = &session.error {
                output.push_str(&format!("  - error: {error}\n"));
            }
        }
        for warning in &self.warnings {
            output.push_str(&format!("- warning: {warning}\n"));
        }
        output
    }
}
