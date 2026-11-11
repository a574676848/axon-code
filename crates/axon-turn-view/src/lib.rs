//! 回合投影的渠道中立模型：一条 wire 事实落到哪一格。
//!
//! 这一层只回答「归并」这一件事：正文按 `part_id` 归段、工具按 `tool_call_id` 归行、
//! 思考按 `part_id` 只留耗时、提示按 `key` 覆盖。它不做渲染、不碰网络、不持有时间，
//! 因此可以被整体穷举测试，也就能被多个渠道共用。
//!
//! 边界：
//! - 这里统一的是**事实**（哪些格子、什么状态、累积的正文），不是**呈现**。
//! - 工具行的显示名、目标与读数由调用方按自己的展示规则填进来；飞书的 8000 字预算、
//!   桌面的折叠与虚拟列表、TUI 的布局都留在各自渠道。
//! - 渠道只需要把 wire 事件折算成 [`TurnEvent`]，归并规则不再各写一遍。

use serde::Serialize;
use std::collections::HashMap;

/// 过程行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RowStatus {
    /// 已开始，尚未返回结果。
    Running,
    /// 已成功完成。
    Completed,
    /// 已失败或未完成。
    Failed,
}

/// 一条工具行：折叠态的全部信息都在这里，不再保留可展开的明细。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRow {
    /// 工具调用 ID，行身份。
    pub call_id: String,
    /// 类别显示名，例如「执行」「读取」。
    pub label: String,
    /// 折叠态目标：模型自述目的或该工具的主参数。
    pub target: Option<String>,
    /// 当前状态。
    pub status: RowStatus,
    /// 行尾读数：`exit 1`、`1.2s`、`已写入`、`失败`；都没有时为 `None`。
    pub meta: Option<String>,
}

/// 一条思考行：只留耗时。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningRow {
    /// 思考段身份（wire `part_id`）。
    pub part_id: String,
    /// 开始时刻；用于在结束后算出耗时。
    #[serde(skip)]
    started_at_ms: u64,
    /// 已结束时的耗时；仍在思考时为 `None`。
    pub duration_ms: Option<u64>,
}

/// 一条提示行；同 `key` 覆盖，因此重试与切换供应商不会累积成墙。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeRow {
    /// 覆盖键。
    pub key: String,
    /// 提示正文。
    pub text: String,
}

/// 投影单元，按到达顺序排列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Cell {
    /// 正文段。
    #[serde(rename_all = "camelCase")]
    Body {
        /// transcript part 身份。
        part_id: String,
        /// 已累积的正文。
        text: String,
    },
    /// 工具行。
    Tool(ToolRow),
    /// 思考行。
    Reasoning(ReasoningRow),
    /// 提示行。
    Notice(NoticeRow),
}

impl Cell {
    /// 该单元的 part 身份；工具行与提示行没有。
    #[must_use]
    pub fn part_id(&self) -> Option<&str> {
        match self {
            Self::Body { part_id, .. } => Some(part_id),
            Self::Reasoning(row) => Some(&row.part_id),
            Self::Tool(_) | Self::Notice(_) => None,
        }
    }
}

/// 渠道折算出来的一条投影事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    /// 正文增量：按 `part_id` 追加。
    AssistantDelta {
        /// transcript part 身份。
        part_id: String,
        /// 增量文本。
        text: String,
    },
    /// 正文整段：按 `part_id` 覆写，用于 semantic 终稿与边界快照。
    AssistantText {
        /// transcript part 身份。
        part_id: String,
        /// 整段正文。
        text: String,
    },
    /// 工具调用开始。
    ToolStarted {
        /// 工具调用 ID。
        call_id: String,
        /// 类别显示名。
        label: String,
        /// 折叠态目标；无信息量时为 `None`。
        target: Option<String>,
    },
    /// 工具调用完成。
    ToolFinished {
        /// 工具调用 ID。
        call_id: String,
        /// 类别显示名；调用事件丢失时用于补一整行。
        label: String,
        /// 结果状态。
        status: RowStatus,
        /// 行尾读数。
        meta: Option<String>,
    },
    /// 思考开始。
    ReasoningStarted {
        /// transcript part 身份。
        part_id: String,
        /// 事件时刻。
        at_ms: u64,
    },
    /// 思考结束。
    ReasoningFinished {
        /// transcript part 身份。
        part_id: String,
        /// 事件时刻。
        at_ms: u64,
    },
    /// 提示。
    Notice {
        /// 覆盖键。
        key: String,
        /// 提示正文。
        text: String,
    },
    /// 当前供应商路由；只供页脚使用，运行期不渲染。
    Route {
        /// `provider/model`。
        route: String,
    },
    /// 当前上下文占用；只供页脚使用。
    ContextTokens {
        /// token 数。
        tokens: u64,
    },
}

/// 一次回合的投影。
///
/// 单元格按到达顺序保存，同时用四张索引表把「按身份找格」压到 O(1)：一轮里正文段、
/// 工具行与提示行可能有几十到上百个，逐条线性扫描会让整个回合退化成 O(n²)。
/// 索引只在追加时写入、单元从不删除，因此位置永远有效。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnView {
    cells: Vec<Cell>,
    #[serde(skip)]
    body_positions: HashMap<String, usize>,
    #[serde(skip)]
    tool_positions: HashMap<String, usize>,
    #[serde(skip)]
    reasoning_positions: HashMap<String, usize>,
    #[serde(skip)]
    notice_positions: HashMap<String, usize>,
    route: Option<String>,
    context_tokens: Option<u64>,
}

impl TurnView {
    /// 应用一条投影事件。
    pub fn apply(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::AssistantDelta { part_id, text } => self.push_body(&part_id, &text),
            TurnEvent::AssistantText { part_id, text } => self.set_body(&part_id, &text),
            TurnEvent::ToolStarted {
                call_id,
                label,
                target,
            } => self.start_tool(call_id, label, target),
            TurnEvent::ToolFinished {
                call_id,
                label,
                status,
                meta,
            } => self.finish_tool(&call_id, &label, status, meta),
            TurnEvent::ReasoningStarted { part_id, at_ms } => self.start_reasoning(&part_id, at_ms),
            TurnEvent::ReasoningFinished { part_id, at_ms } => {
                self.finish_reasoning(&part_id, at_ms);
            }
            TurnEvent::Notice { key, text } => self.push_notice(key, text),
            TurnEvent::Route { route } => self.route = Some(route),
            TurnEvent::ContextTokens { tokens } => self.context_tokens = Some(tokens),
        }
    }

    /// 投影单元。
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// 本回合的供应商路由。
    #[must_use]
    pub fn route(&self) -> Option<&str> {
        self.route.as_deref()
    }

    /// 最近一次上下文占用。
    #[must_use]
    pub fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
    }

    /// 尚无任何可渲染内容。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// 正文原文：各段按到达顺序拼接，尚未剥离渠道协议行、也未受预算约束。
    #[must_use]
    pub fn body_text(&self) -> String {
        let mut body = String::new();
        for cell in &self.cells {
            let Cell::Body { text, .. } = cell else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(text);
        }
        body
    }

    fn body_index(&self, part_id: &str) -> Option<usize> {
        self.body_positions.get(part_id).copied()
    }

    fn push_body(&mut self, part_id: &str, delta: &str) {
        if delta.is_empty() {
            return;
        }
        if let Some(index) = self.body_index(part_id) {
            if let Cell::Body { text, .. } = &mut self.cells[index] {
                text.push_str(delta);
            }
            return;
        }
        self.body_positions
            .insert(part_id.to_owned(), self.cells.len());
        self.cells.push(Cell::Body {
            part_id: part_id.to_owned(),
            text: delta.to_owned(),
        });
    }

    fn set_body(&mut self, part_id: &str, text: &str) {
        if let Some(index) = self.body_index(part_id) {
            if let Cell::Body { text: existing, .. } = &mut self.cells[index] {
                *existing = text.to_owned();
            }
            return;
        }
        self.body_positions
            .insert(part_id.to_owned(), self.cells.len());
        self.cells.push(Cell::Body {
            part_id: part_id.to_owned(),
            text: text.to_owned(),
        });
    }

    fn start_tool(&mut self, call_id: String, label: String, target: Option<String>) {
        if let Some(index) = self.tool_index(&call_id) {
            // 同一调用的重复开始只补齐信息，不改已定的状态。
            if let Cell::Tool(row) = &mut self.cells[index] {
                if row.target.is_none() {
                    row.target = target;
                }
                if !label.is_empty() {
                    row.label = label;
                }
            }
            return;
        }
        self.tool_positions
            .insert(call_id.clone(), self.cells.len());
        self.cells.push(Cell::Tool(ToolRow {
            call_id,
            label,
            target,
            status: RowStatus::Running,
            meta: None,
        }));
    }

    fn finish_tool(&mut self, call_id: &str, label: &str, status: RowStatus, meta: Option<String>) {
        if let Some(index) = self.tool_index(call_id) {
            if let Cell::Tool(row) = &mut self.cells[index] {
                // 结果只补状态与读数：折叠行的目标在调用时就已定型。
                row.status = status;
                if meta.is_some() {
                    row.meta = meta;
                }
                if row.label.is_empty() && !label.is_empty() {
                    row.label = label.to_owned();
                }
            }
            return;
        }
        // 调用事件丢失（例如邮箱满载淘汰）：补一整行，保证过程不丢。
        self.tool_positions
            .insert(call_id.to_owned(), self.cells.len());
        self.cells.push(Cell::Tool(ToolRow {
            call_id: call_id.to_owned(),
            label: label.to_owned(),
            target: None,
            status,
            meta,
        }));
    }

    fn tool_index(&self, call_id: &str) -> Option<usize> {
        self.tool_positions.get(call_id).copied()
    }

    fn start_reasoning(&mut self, part_id: &str, at_ms: u64) {
        if self.reasoning_index(part_id).is_some() {
            return;
        }
        self.reasoning_positions
            .insert(part_id.to_owned(), self.cells.len());
        self.cells.push(Cell::Reasoning(ReasoningRow {
            part_id: part_id.to_owned(),
            started_at_ms: at_ms,
            duration_ms: None,
        }));
    }

    fn finish_reasoning(&mut self, part_id: &str, at_ms: u64) {
        if let Some(index) = self.reasoning_index(part_id) {
            if let Cell::Reasoning(row) = &mut self.cells[index] {
                row.duration_ms = Some(at_ms.saturating_sub(row.started_at_ms));
            }
            return;
        }
        // 开始事件丢失：仍落一行，不猜耗时。
        self.reasoning_positions
            .insert(part_id.to_owned(), self.cells.len());
        self.cells.push(Cell::Reasoning(ReasoningRow {
            part_id: part_id.to_owned(),
            started_at_ms: at_ms,
            duration_ms: None,
        }));
    }

    fn reasoning_index(&self, part_id: &str) -> Option<usize> {
        self.reasoning_positions.get(part_id).copied()
    }

    fn push_notice(&mut self, key: String, text: String) {
        if let Some(index) = self.notice_positions.get(&key).copied() {
            if let Cell::Notice(row) = &mut self.cells[index] {
                row.text = text;
            }
            return;
        }
        self.notice_positions.insert(key.clone(), self.cells.len());
        self.cells.push(Cell::Notice(NoticeRow { key, text }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str) -> TurnEvent {
        TurnEvent::AssistantDelta {
            part_id: "part-1".to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn transient_status_never_becomes_body() {
        let mut view = TurnView::default();
        view.apply(TurnEvent::Route {
            route: "cpa/gemini-3.8-flash-high".to_owned(),
        });
        view.apply(TurnEvent::ContextTokens { tokens: 12_000 });
        view.apply(delta("结论"));

        assert_eq!(view.body_text(), "结论");
        assert_eq!(view.cells().len(), 1, "状态只落在字段上，不占单元");
        assert_eq!(view.route(), Some("cpa/gemini-3.8-flash-high"));
        assert_eq!(view.context_tokens(), Some(12_000));
    }

    #[test]
    fn body_segments_merge_by_part_and_keep_order() {
        let mut view = TurnView::default();
        view.apply(delta("第一段"));
        view.apply(TurnEvent::ToolStarted {
            call_id: "call-1".to_owned(),
            label: "读取".to_owned(),
            target: Some("a.md".to_owned()),
        });
        view.apply(TurnEvent::AssistantDelta {
            part_id: "part-2".to_owned(),
            text: "第二段".to_owned(),
        });
        view.apply(TurnEvent::AssistantDelta {
            part_id: "part-1".to_owned(),
            text: "续写".to_owned(),
        });

        assert_eq!(view.body_text(), "第一段续写\n\n第二段");
        assert_eq!(view.cells().len(), 3, "同一 part 只占一格");
    }

    #[test]
    fn whole_text_replaces_the_same_part_instead_of_appending() {
        let mut view = TurnView::default();
        view.apply(delta("先写"));
        view.apply(TurnEvent::AssistantText {
            part_id: "part-1".to_owned(),
            text: "整段终稿".to_owned(),
        });

        assert_eq!(view.body_text(), "整段终稿", "整段提交必须覆写而不是叠加");
        assert_eq!(view.cells().len(), 1);
    }

    #[test]
    fn tool_result_patches_status_without_rewriting_target() {
        let mut view = TurnView::default();
        view.apply(TurnEvent::ToolStarted {
            call_id: "call-1".to_owned(),
            label: "执行".to_owned(),
            target: Some("cargo test".to_owned()),
        });
        view.apply(TurnEvent::ToolFinished {
            call_id: "call-1".to_owned(),
            label: "执行".to_owned(),
            status: RowStatus::Failed,
            meta: Some("exit 1".to_owned()),
        });

        let Cell::Tool(row) = &view.cells()[0] else {
            panic!("必须是工具行");
        };
        assert_eq!(
            row.target.as_deref(),
            Some("cargo test"),
            "回执不得覆写目标行"
        );
        assert_eq!(row.status, RowStatus::Failed);
        assert_eq!(row.meta.as_deref(), Some("exit 1"));
        assert_eq!(view.cells().len(), 1, "同一调用只占一行");
    }

    #[test]
    fn tool_result_without_call_still_lands_one_row() {
        let mut view = TurnView::default();
        view.apply(TurnEvent::ToolFinished {
            call_id: "call-lost".to_owned(),
            label: "读取".to_owned(),
            status: RowStatus::Completed,
            meta: None,
        });
        assert_eq!(view.cells().len(), 1, "调用事件丢了也不能丢过程");
    }

    #[test]
    fn reasoning_keeps_duration_only() {
        let mut view = TurnView::default();
        view.apply(TurnEvent::ReasoningStarted {
            part_id: "reason-1".to_owned(),
            at_ms: 1_000,
        });
        view.apply(TurnEvent::ReasoningFinished {
            part_id: "reason-1".to_owned(),
            at_ms: 4_500,
        });

        let Cell::Reasoning(row) = &view.cells()[0] else {
            panic!("必须是思考行");
        };
        assert_eq!(row.duration_ms, Some(3_500));
        assert_eq!(view.body_text(), "", "思考正文永不进正文");
    }

    #[test]
    fn notices_overwrite_by_key() {
        let mut view = TurnView::default();
        view.apply(TurnEvent::Notice {
            key: "provider".to_owned(),
            text: "重试中".to_owned(),
        });
        view.apply(TurnEvent::Notice {
            key: "provider".to_owned(),
            text: "切换供应商：K3/kimi-k3".to_owned(),
        });

        let Cell::Notice(row) = &view.cells()[0] else {
            panic!("必须是提示行");
        };
        assert_eq!(row.text, "切换供应商：K3/kimi-k3", "同类提示只留最新一条");
        assert_eq!(view.cells().len(), 1);
    }

    #[test]
    fn applying_one_hundred_thousand_events_stays_linear() {
        let started = std::time::Instant::now();
        let mut view = TurnView::default();
        for index in 0..100_000_usize {
            let call_id = index / 10;
            if index % 10 == 0 {
                view.apply(TurnEvent::ToolStarted {
                    call_id: format!("call-{call_id}"),
                    label: "执行".to_owned(),
                    target: Some("cargo test".to_owned()),
                });
            } else {
                view.apply(TurnEvent::ToolFinished {
                    call_id: format!("call-{call_id}"),
                    label: String::new(),
                    status: RowStatus::Completed,
                    meta: None,
                });
            }
        }

        assert_eq!(view.cells().len(), 10_000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "归并必须靠 O(1) 查表：逐单元格线性扫描会在这里退化到分钟级"
        );
    }
}
