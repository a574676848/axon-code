use axon_domain::{
    ByteChunkId, ModelProfileId, OutputDrainState, PaneId, PermissionSnapshotId, RuntimeCellState,
    ToolCallId,
};

use super::{
    LongRunningCellRuntime, LongRunningJobEvent, LongRunningJobStatus, MemoryAdmissionRuntime,
    RuntimeCellRuntime,
};

#[test]
fn long_running_cell_pause_resume_cancel_controls_background_job() {
    let cell_runtime = RuntimeCellRuntime::default();
    let mut job_runtime = LongRunningCellRuntime::new();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    cell_runtime.start_cell(&mut cell).unwrap();
    cell_runtime
        .start_tool_call(&mut cell, ToolCallId(1))
        .unwrap();
    let job_id = job_runtime
        .attach_running_cell(&cell, 1_000, "索引代码")
        .unwrap();
    job_runtime
        .update_progress(job_id, 40, "已完成扫描")
        .unwrap();
    job_runtime.heartbeat(job_id, 1_500).unwrap();
    job_runtime.pause(job_id, &mut cell).unwrap();

    assert!(job_runtime.heartbeat(job_id, 1_600).is_err());
    assert!(job_runtime.update_progress(job_id, 50, "不应更新").is_err());

    job_runtime.resume(job_id, &mut cell).unwrap();
    job_runtime.update_progress(job_id, 60, "继续执行").unwrap();
    job_runtime.cancel(job_id, &mut cell, "用户取消").unwrap();

    let job = job_runtime.job(job_id).unwrap();
    assert_eq!(job.status, LongRunningJobStatus::Cancelled);
    assert!(matches!(cell.state, RuntimeCellState::Cancelled(_)));
    assert!(job_runtime.heartbeat(job_id, 2_000).is_err());
    assert!(matches!(
        job_runtime.events(),
        [
            LongRunningJobEvent::Attached { .. },
            LongRunningJobEvent::ProgressUpdated { .. },
            LongRunningJobEvent::Heartbeat { .. },
            LongRunningJobEvent::Paused { .. },
            LongRunningJobEvent::Resumed { .. },
            LongRunningJobEvent::ProgressUpdated { .. },
            LongRunningJobEvent::Cancelled { .. },
        ]
    ));
}

#[test]
fn runtime_cell_cannot_complete_while_waiting_tool() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    runtime.start_cell(&mut cell).unwrap();
    runtime.start_tool_call(&mut cell, ToolCallId(1)).unwrap();

    assert!(runtime.close_cell(&mut cell, None).is_err());
    assert!(runtime.complete_cell(&mut cell, "不应完成").is_err());
}

#[test]
fn runtime_cell_records_tool_delta_and_completion() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    runtime.start_cell(&mut cell).unwrap();
    runtime.start_tool_call(&mut cell, ToolCallId(1)).unwrap();
    runtime
        .record_tool_delta(&mut cell, ToolCallId(1), "读取中")
        .unwrap();
    runtime.complete_tool_call(&mut cell).unwrap();

    assert!(matches!(cell.state, RuntimeCellState::Running));
    assert_eq!(cell.events.len(), 5);
}

#[test]
fn runtime_cell_records_tool_failure() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    runtime.start_cell(&mut cell).unwrap();
    runtime.start_tool_call(&mut cell, ToolCallId(1)).unwrap();
    runtime
        .fail_tool_call(&mut cell, ToolCallId(1), "工具失败")
        .unwrap();

    assert!(matches!(cell.state, RuntimeCellState::Failed(_)));
    assert_eq!(cell.events.len(), 5);
}

#[test]
fn runtime_cell_records_completion_events() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    runtime.start_cell(&mut cell).unwrap();
    runtime.close_cell(&mut cell, None).unwrap();
    runtime.complete_cell(&mut cell, "完成").unwrap();

    assert!(matches!(cell.state, RuntimeCellState::Completed(_)));
    assert_eq!(cell.events.len(), 4);
}

#[test]
fn runtime_cell_state_machine_rejects_non_linear_transitions() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    assert!(runtime.start_tool_call(&mut cell, ToolCallId(1)).is_err());
    runtime.start_cell(&mut cell).unwrap();
    runtime.start_tool_call(&mut cell, ToolCallId(1)).unwrap();

    assert!(runtime.start_tool_call(&mut cell, ToolCallId(2)).is_err());

    runtime.complete_tool_call(&mut cell).unwrap();
    runtime.close_cell(&mut cell, None).unwrap();
    runtime.complete_cell(&mut cell, "完成").unwrap();

    assert!(runtime.start_cell(&mut cell).is_err());
    assert!(runtime.start_tool_call(&mut cell, ToolCallId(3)).is_err());
}

#[test]
fn runtime_cell_pause_resume_restores_waiting_tool_and_is_queryable() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    runtime.start_cell(&mut cell).unwrap();
    runtime.start_tool_call(&mut cell, ToolCallId(9)).unwrap();
    runtime.pause_cell(&mut cell).unwrap();
    let paused_snapshot = cell.snapshot();
    runtime.resume_cell(&mut cell).unwrap();

    assert!(matches!(
        paused_snapshot.state,
        RuntimeCellState::Paused(axon_domain::PausedCellState::WaitingTool(ToolCallId(9)))
    ));
    assert!(matches!(
        cell.state,
        RuntimeCellState::WaitingTool(ToolCallId(9))
    ));
    assert_eq!(cell.snapshot().event_count, 5);
    assert!(runtime.pause_cell(&mut cell).is_ok());
    assert!(runtime.pause_cell(&mut cell).is_err());
}

#[test]
fn runtime_cell_pause_rejects_draft_and_final_states() {
    let runtime = RuntimeCellRuntime::default();
    let mut draft = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );
    let mut final_cell = RuntimeCellRuntime::default().create_cell(
        PaneId(2),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    assert!(runtime.pause_cell(&mut draft).is_err());
    runtime.start_cell(&mut final_cell).unwrap();
    runtime.close_cell(&mut final_cell, None).unwrap();
    runtime.complete_cell(&mut final_cell, "完成").unwrap();
    assert!(runtime.pause_cell(&mut final_cell).is_err());
}

#[test]
fn runtime_cell_waits_for_output_drain_before_completion() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    runtime.start_cell(&mut cell).unwrap();
    runtime
        .mark_output_pending(&mut cell, ByteChunkId(1))
        .unwrap();

    assert!(runtime.complete_cell(&mut cell, "不应完成").is_err());

    runtime
        .mark_output_drained(&mut cell, ByteChunkId(1))
        .unwrap();
    runtime.close_cell(&mut cell, Some(ByteChunkId(1))).unwrap();
    runtime.complete_cell(&mut cell, "完成").unwrap();

    assert!(matches!(cell.state, RuntimeCellState::Completed(_)));
    assert!(matches!(cell.output_drain, OutputDrainState::Drained));
}

#[test]
fn memory_admission_requires_closed_completed_cell() {
    let runtime = RuntimeCellRuntime::default();
    let mut cell = RuntimeCellRuntime::default().create_cell(
        PaneId(1),
        "E:\\zbg\\axon-code",
        ModelProfileId(1),
        PermissionSnapshotId(1),
    );

    runtime.start_cell(&mut cell).unwrap();
    assert!(MemoryAdmissionRuntime::ensure_cell_extractable(&cell).is_err());

    runtime.close_cell(&mut cell, None).unwrap();
    assert!(MemoryAdmissionRuntime::ensure_cell_extractable(&cell).is_err());

    runtime.complete_cell(&mut cell, "完成").unwrap();
    assert!(MemoryAdmissionRuntime::ensure_cell_extractable(&cell).is_ok());
}
