//! 飞书回合投影的折叠态模型。
//!
//! 这一层只回答一个问题：一条已提交的 wire 事件落到投影的哪一格。它不做渲染、
//! 不碰网络、也不持有时间，因此可以整体被穷举测试。
//!
//! 归并规则（与桌面端 transcript 的 cell 语义同源）：
//! - 正文按 `part_id` 归段，`delta` 追加、整段提交时覆写；
//! - 工具按 `tool_call_id` 归行，结果只补状态与读数，**绝不复写目标行**；
//! - 思考按 `part_id` 归行，只留耗时，思考正文永不投影；
//! - 提示按 `key` 覆盖，瞬态状态（连接、生成中）不产生任何正文。

/// 过程行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowStatus {
    /// 已开始，尚未返回结果。
    Running,
    /// 已成功完成。
    Completed,
    /// 已失败或未完成。
    Failed,
}

/// 一条工具行：折叠态的全部信息都在这里，不再保留可展开的明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolRow {
    /// 工具调用 ID，行身份。
    pub(crate) call_id: String,
    /// 类别显示名，例如「执行」「读取」。
    pub(crate) label: String,
    /// 折叠态目标：模型自述目的或该工具的主参数。
    pub(crate) target: Option<String>,
    /// 当前状态。
    pub(crate) status: RowStatus,
    /// 行尾读数：`exit 1`、`1.2s`、`已写入`、`失败`；都没有时为 `None`。
    pub(crate) meta: Option<String>,
}

/// 一条思考行：只留耗时。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReasoningRow {
    /// 思考段身份（wire `part_id`）。
    pub(crate) part_id: String,
    /// 开始时刻；用于在结束后算出耗时。
    started_at_ms: u64,
    /// 已结束时的耗时；仍在思考时为 `None`。
    pub(crate) duration_ms: Option<u64>,
}

/// 一条提示行；同 `key` 覆盖，因此重试与切换供应商不会累积成墙。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoticeRow {
    /// 覆盖键。
    pub(crate) key: String,
    /// 提示正文。
    pub(crate) text: String,
}

/// 投影单元，按到达顺序排列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    /// 正文段。
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

/// 已提交 wire 事件归一出的投影事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectionEvent {
    /// 正文增量。
    AssistantDelta {
        /// transcript part 身份。
        part_id: String,
        /// 增量文本。
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
    /// 当前供应商路由；只供终态页脚使用，运行期不渲染。
    Route {
        /// `provider/model`。
        route: String,
    },
    /// 当前上下文占用；只供终态页脚使用。
    ContextTokens {
        /// token 数。
        tokens: u64,
    },
}

/// 一次回合的投影。
#[derive(Debug, Clone, Default)]
pub(crate) struct ProgressProjection {
    cells: Vec<Cell>,
    route: Option<String>,
    context_tokens: Option<u64>,
}

impl ProgressProjection {
    /// 应用一条投影事件。
    pub(crate) fn apply(&mut self, event: ProjectionEvent) {
        match event {
            ProjectionEvent::AssistantDelta { part_id, text } => self.push_body(&part_id, &text),
            ProjectionEvent::ToolStarted {
                call_id,
                label,
                target,
            } => self.start_tool(call_id, label, target),
            ProjectionEvent::ToolFinished {
                call_id,
                label,
                status,
                meta,
            } => self.finish_tool(&call_id, &label, status, meta),
            ProjectionEvent::ReasoningStarted { part_id, at_ms } => {
                self.start_reasoning(&part_id, at_ms)
            }
            ProjectionEvent::ReasoningFinished { part_id, at_ms } => {
                self.finish_reasoning(&part_id, at_ms);
            }
            ProjectionEvent::Notice { key, text } => self.push_notice(key, text),
            ProjectionEvent::Route { route } => self.route = Some(route),
            ProjectionEvent::ContextTokens { tokens } => self.context_tokens = Some(tokens),
        }
    }

    /// 投影单元。
    #[must_use]
    pub(crate) fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// 本回合的供应商路由。
    #[must_use]
    pub(crate) fn route(&self) -> Option<&str> {
        self.route.as_deref()
    }

    /// 最近一次上下文占用。
    #[must_use]
    pub(crate) fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
    }

    /// 尚无任何可渲染内容。
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// 正文原文：各段按到达顺序拼接，尚未剥离 `MEDIA:` 协议行、也未受预算约束。
    #[must_use]
    pub(crate) fn body_text(&self) -> String {
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
        self.cells
            .iter()
            .position(|cell| matches!(cell, Cell::Body { part_id: id, .. } if id == part_id))
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
        self.cells.push(Cell::Body {
            part_id: part_id.to_owned(),
            text: delta.to_owned(),
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
        self.cells.push(Cell::Tool(ToolRow {
            call_id: call_id.to_owned(),
            label: label.to_owned(),
            target: None,
            status,
            meta,
        }));
    }

    fn tool_index(&self, call_id: &str) -> Option<usize> {
        self.cells
            .iter()
            .position(|cell| matches!(cell, Cell::Tool(row) if row.call_id == call_id))
    }

    fn start_reasoning(&mut self, part_id: &str, at_ms: u64) {
        if self.reasoning_index(part_id).is_some() {
            return;
        }
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
        self.cells.push(Cell::Reasoning(ReasoningRow {
            part_id: part_id.to_owned(),
            started_at_ms: at_ms,
            duration_ms: None,
        }));
    }

    fn reasoning_index(&self, part_id: &str) -> Option<usize> {
        self.cells
            .iter()
            .position(|cell| matches!(cell, Cell::Reasoning(row) if row.part_id == part_id))
    }

    fn push_notice(&mut self, key: String, text: String) {
        if let Some(index) = self
            .cells
            .iter()
            .position(|cell| matches!(cell, Cell::Notice(row) if row.key == key))
        {
            if let Cell::Notice(row) = &mut self.cells[index] {
                row.text = text;
            }
            return;
        }
        self.cells.push(Cell::Notice(NoticeRow { key, text }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str) -> ProjectionEvent {
        ProjectionEvent::AssistantDelta {
            part_id: "part-1".to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn transient_status_never_becomes_body() {
        let mut projection = ProgressProjection::default();
        projection.apply(ProjectionEvent::Route {
            route: "cpa/gemini-3.8-flash-high".to_owned(),
        });
        projection.apply(ProjectionEvent::ContextTokens { tokens: 12_000 });
        projection.apply(delta("结论"));

        assert_eq!(projection.body_text(), "结论");
        assert_eq!(projection.cells().len(), 1, "状态只落在字段上，不占单元");
        assert_eq!(projection.route(), Some("cpa/gemini-3.8-flash-high"));
        assert_eq!(projection.context_tokens(), Some(12_000));
    }

    #[test]
    fn body_segments_merge_by_part_and_keep_order() {
        let mut projection = ProgressProjection::default();
        projection.apply(delta("第一段"));
        projection.apply(ProjectionEvent::ToolStarted {
            call_id: "call-1".to_owned(),
            label: "读取".to_owned(),
            target: Some("a.md".to_owned()),
        });
        projection.apply(ProjectionEvent::AssistantDelta {
            part_id: "part-2".to_owned(),
            text: "第二段".to_owned(),
        });
        projection.apply(ProjectionEvent::AssistantDelta {
            part_id: "part-1".to_owned(),
            text: "续写".to_owned(),
        });

        assert_eq!(projection.body_text(), "第一段续写\n\n第二段");
        assert_eq!(projection.cells().len(), 3, "同一 part 只占一格");
    }

    #[test]
    fn tool_result_patches_status_without_rewriting_target() {
        let mut projection = ProgressProjection::default();
        projection.apply(ProjectionEvent::ToolStarted {
            call_id: "call-1".to_owned(),
            label: "执行".to_owned(),
            target: Some("cargo test".to_owned()),
        });
        projection.apply(ProjectionEvent::ToolFinished {
            call_id: "call-1".to_owned(),
            label: "执行".to_owned(),
            status: RowStatus::Failed,
            meta: Some("exit 1".to_owned()),
        });

        let Cell::Tool(row) = &projection.cells()[0] else {
            panic!("必须是工具行");
        };
        assert_eq!(
            row.target.as_deref(),
            Some("cargo test"),
            "回执不得覆写目标行"
        );
        assert_eq!(row.status, RowStatus::Failed);
        assert_eq!(row.meta.as_deref(), Some("exit 1"));
        assert_eq!(projection.cells().len(), 1, "同一调用只占一行");
    }

    #[test]
    fn tool_result_without_call_still_lands_one_row() {
        let mut projection = ProgressProjection::default();
        projection.apply(ProjectionEvent::ToolFinished {
            call_id: "call-lost".to_owned(),
            label: "读取".to_owned(),
            status: RowStatus::Completed,
            meta: None,
        });
        assert_eq!(projection.cells().len(), 1, "调用事件丢了也不能丢过程");
    }

    #[test]
    fn reasoning_keeps_duration_only() {
        let mut projection = ProgressProjection::default();
        projection.apply(ProjectionEvent::ReasoningStarted {
            part_id: "reason-1".to_owned(),
            at_ms: 1_000,
        });
        projection.apply(ProjectionEvent::ReasoningFinished {
            part_id: "reason-1".to_owned(),
            at_ms: 4_500,
        });

        let Cell::Reasoning(row) = &projection.cells()[0] else {
            panic!("必须是思考行");
        };
        assert_eq!(row.duration_ms, Some(3_500));
        assert_eq!(projection.body_text(), "", "思考正文永不进正文");
    }

    #[test]
    fn notices_overwrite_by_key() {
        let mut projection = ProgressProjection::default();
        projection.apply(ProjectionEvent::Notice {
            key: "provider".to_owned(),
            text: "重试中".to_owned(),
        });
        projection.apply(ProjectionEvent::Notice {
            key: "provider".to_owned(),
            text: "切换供应商：K3/kimi-k3".to_owned(),
        });

        let Cell::Notice(row) = &projection.cells()[0] else {
            panic!("必须是提示行");
        };
        assert_eq!(row.text, "切换供应商：K3/kimi-k3", "同类提示只留最新一条");
        assert_eq!(projection.cells().len(), 1);
    }
}
