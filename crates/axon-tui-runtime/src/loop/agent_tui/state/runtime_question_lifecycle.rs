//! Runtime UserQuestion transcript 单元生命周期编排（UQ-205）。
//!
//! 实时提问事件与快照重放必须落到同一件事上：一条按 `part_id` 稳定的提问单元，
//! 以及「还能作答时打开底部面板」。这里只负责单元与面板，作答提交由
//! [`crate::r#loop::agent_tui::state::AgentTuiState::handle_bottom_view_key`] 收口。

use axon_protocol::wire::WireUserQuestion;

use crate::{
    r#loop::agent_tui::{
        bottom_view::{BottomView, QuestionView},
        state::AgentTuiState,
    },
    surface::{
        cell::{RenderCellData, RenderCellRecord},
        cell_question_data::AgentRenderQuestionCell,
    },
};

/// 还能作答的提问状态：面板必须处于打开状态。
const QUESTION_STATUS_PENDING: &str = "pending";
/// 截止前被延长的提问状态：同样还能作答。
const QUESTION_STATUS_EXTENDED: &str = "extended";

impl AgentTuiState {
    /// 把一条提问事件投影为提问单元，并同步面板开合。
    ///
    /// `lane_id` 取自事件信封的会话身份：Daemon 下它就是主 turn 的车道身份，
    /// 作答必须沿同一个身份回到统一答案入口。
    pub(in crate::r#loop::agent_tui) fn apply_user_question(
        &mut self,
        lane_id: &str,
        question: WireUserQuestion,
    ) {
        let answerable = is_answerable_status(&question.status);
        let part_id = question.part_id.clone();
        self.upsert_question_cell(&part_id, question_cell(&part_id, &question));
        if answerable {
            self.open_bottom_view(BottomView::Question(QuestionView::new(
                lane_id,
                question.call_id,
                question.questions,
                question.deadline_at_ms,
            )));
            return;
        }
        // 已结算 / 已挂起：面板必须收起，只留可回看的单元。
        self.close_question_view();
    }

    /// 按 `part_id` 原位替换提问单元；同一次提问的状态流转不能堆出多条单元。
    fn upsert_question_cell(&mut self, part_id: &str, cell: RenderCellRecord) {
        let existing = self.cells.transcript.iter().position(|line| {
            line.part_id() == part_id && matches!(line.data, Some(RenderCellData::Question(_)))
        });
        if let Some(index) = existing
            && self
                .cells
                .transcript
                .replace_cell(index, cell.clone())
                .is_ok()
        {
            return;
        }
        self.cells.transcript.push(cell);
    }
}

fn is_answerable_status(status: &str) -> bool {
    matches!(status, QUESTION_STATUS_PENDING | QUESTION_STATUS_EXTENDED)
}

fn question_cell(part_id: &str, question: &WireUserQuestion) -> RenderCellRecord {
    RenderCellRecord::question_with_data(AgentRenderQuestionCell::new(
        part_id,
        question.call_id.clone(),
        question.status.clone(),
        question.questions.clone(),
        question.answers.clone(),
        question.handoff.clone(),
        question.deadline_at_ms,
    ))
}
