use super::*;

#[test]
fn feishu_processing_plan_start_adds_typing_reaction() {
    let plan = RemoteFeishuProcessingRuntime::plan_start("om_message").expect("plan start");

    assert_eq!(plan.actions.len(), 1);
    assert_eq!(
        plan.actions[0].kind,
        RemoteFeishuProcessingActionKind::AddReaction
    );
    assert_eq!(
        plan.actions[0].reaction_type,
        REMOTE_FEISHU_PROCESSING_REACTION_IN_PROGRESS
    );
}

#[test]
fn feishu_processing_plan_complete_success_only_removes_typing() {
    let plan = RemoteFeishuProcessingRuntime::plan_complete(
        "om_message",
        RemoteFeishuProcessingOutcome::Success,
    )
    .expect("plan complete");

    assert_eq!(plan.actions.len(), 1);
    assert_eq!(
        plan.actions[0].kind,
        RemoteFeishuProcessingActionKind::RemoveReaction
    );
}

#[test]
fn feishu_processing_plan_complete_failure_adds_crossmark() {
    let plan = RemoteFeishuProcessingRuntime::plan_complete(
        "om_message",
        RemoteFeishuProcessingOutcome::Failure,
    )
    .expect("plan complete");

    assert_eq!(plan.actions.len(), 2);
    assert_eq!(
        plan.actions[1].kind,
        RemoteFeishuProcessingActionKind::AddReaction
    );
    assert_eq!(
        plan.actions[1].reaction_type,
        REMOTE_FEISHU_PROCESSING_REACTION_FAILURE
    );
}

#[test]
fn feishu_processing_execute_start_and_complete_failure() {
    let start = RemoteFeishuProcessingRuntime::execute_start_with_http(
        "https://open.feishu.cn",
        "tenant-token",
        "om_message",
        100,
        |_request| {
            Ok(crate::RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"data":{"reaction_id":"reaction-1"}}"#.to_owned(),
            })
        },
    )
    .expect("start");

    assert_eq!(start.reaction_id, "reaction-1");
    let completed = RemoteFeishuProcessingRuntime::execute_complete_with_http(
        "https://open.feishu.cn",
        "tenant-token",
        "om_message",
        RemoteFeishuProcessingOutcome::Failure,
        Some(&start),
        200,
        |request| {
            if request.method == "DELETE" {
                Ok(crate::RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0}"#.to_owned(),
                })
            } else {
                Ok(crate::RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"reaction_id":"reaction-2"}}"#.to_owned(),
                })
            }
        },
    )
    .expect("complete");

    assert_eq!(completed.len(), 2);
    assert!(!completed[0].active);
    assert_eq!(
        completed[1].reaction_type,
        REMOTE_FEISHU_PROCESSING_REACTION_FAILURE
    );
}
