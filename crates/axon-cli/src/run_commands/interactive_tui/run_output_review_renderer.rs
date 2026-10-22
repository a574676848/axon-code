//! Review wire 事实到终态行的投影。
//!
//! Review 终态属于 durable wire 流：同一 `part_id` 的多次快照按 sequence 收敛为最后一次，
//! 与实时 `ReviewStatus` 投影使用同一 part id 与同一状态映射。

use axon_protocol::wire::WireReviewStatus;
use axon_tui_runtime::AgentTuiReviewLine;

use super::run_output_tool_renderer::lifecycle_status_name;

/// 单帧 review 快照投影；durable 顺序由调用方保证。
pub(super) fn wire_review_line(review: &WireReviewStatus) -> AgentTuiReviewLine {
    let details = if review.details.is_empty() {
        review.detail.iter().cloned().collect()
    } else {
        review.details.clone()
    };
    AgentTuiReviewLine::with_id(
        review.part_id.clone(),
        lifecycle_status_name(review.status),
        review.subject.clone(),
        details,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_protocol::wire::WireLifecycleStatus;
    use axon_tui_runtime::AgentTuiReviewStatus;

    #[test]
    fn wire_review_line_prefers_detail_list_and_falls_back_to_single_detail() {
        let mut review = WireReviewStatus {
            part_id: "review:part:1".to_owned(),
            subject: "审阅".to_owned(),
            status: WireLifecycleStatus::Completed,
            detail: Some("单条详情".to_owned()),
            details: Vec::new(),
        };
        assert_eq!(wire_review_line(&review).details, ["单条详情"]);

        review.details = vec!["第一条".to_owned(), "第二条".to_owned()];
        assert_eq!(wire_review_line(&review).details, ["第一条", "第二条"]);
    }

    #[test]
    fn wire_review_line_maps_lifecycle_status_names() {
        let review = WireReviewStatus {
            part_id: "review:part:2".to_owned(),
            subject: "审阅".to_owned(),
            status: WireLifecycleStatus::Blocked,
            detail: None,
            details: Vec::new(),
        };

        let line = wire_review_line(&review);

        assert_eq!(line.id, "review:part:2");
        assert_eq!(line.status, AgentTuiReviewStatus::Blocked);
    }
}
