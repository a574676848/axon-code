use axon_domain::{PaneId, PaneSecurityBadge, PaneSecurityBadgeLevel, PermissionSnapshotId};

use super::*;

#[test]
fn command_event_card_projects_failed_recent_command() {
    let projection = AgentPaneProjection::from_input(AgentPaneProjectionInput {
        pane_id: PaneId(7),
        cwd: "/workspace/project".to_owned(),
        shell_kind: "bash".to_owned(),
        git_state: None,
        recent_command: Some(failed_recent_command()),
        permission_snapshot_id: PermissionSnapshotId(3),
        backend_badge: PaneSecurityBadge {
            level: PaneSecurityBadgeLevel::Normal,
            label: "normal".to_owned(),
            reasons: Vec::new(),
        },
        runtime_cell: None,
        response: None,
        apply_execution: None,
    });

    let Some(AgentPaneCard::CommandEvent {
        command,
        exit_code,
        summary,
        source_ref,
        remote_full_content_allowed,
    }) = projection
        .cards
        .iter()
        .find(|card| matches!(card, AgentPaneCard::CommandEvent { .. }))
    else {
        panic!("失败命令必须投影为 CommandEvent 卡片");
    };

    assert_eq!(command, "cargo check");
    assert_eq!(*exit_code, Some(101));
    assert!(summary.contains("耗时=1.2s"));
    assert!(!summary.contains("duration_ms="));
    assert!(summary.contains("mismatched types"));
    assert!(summary.contains("src/layout.rs:142:9"));
    assert_eq!(source_ref, "command-event://pane/7");
    assert!(!remote_full_content_allowed);
    assert!(projection.explain().contains("命令事件"));
}

#[test]
fn command_event_card_redacts_sensitive_summary() {
    let mut command = failed_recent_command();
    command.command = "curl --header Authorization:sk-ant-test-fixture-invalid-0001".to_owned();
    command.output_tail = "request failed api_key=secret-value".to_owned();
    command.detected_errors[0].message = "failed token=ghp_test_fixture_invalid_0001".to_owned();
    let projection = AgentPaneProjection::from_input(AgentPaneProjectionInput {
        pane_id: PaneId(7),
        cwd: "/workspace/project".to_owned(),
        shell_kind: "bash".to_owned(),
        git_state: None,
        recent_command: Some(command),
        permission_snapshot_id: PermissionSnapshotId(3),
        backend_badge: PaneSecurityBadge {
            level: PaneSecurityBadgeLevel::Normal,
            label: "normal".to_owned(),
            reasons: Vec::new(),
        },
        runtime_cell: None,
        response: None,
        apply_execution: None,
    });

    let Some(AgentPaneCard::CommandEvent {
        command, summary, ..
    }) = projection
        .cards
        .iter()
        .find(|card| matches!(card, AgentPaneCard::CommandEvent { .. }))
    else {
        panic!("失败命令必须投影为 CommandEvent 卡片");
    };

    assert!(command.contains("[REDACTED]"));
    assert!(summary.contains("[REDACTED]"));
    assert!(!command.contains("sk-ant-test-fixture-invalid-0001"));
    assert!(!summary.contains("sk-ant-test-fixture-invalid-0001"));
    assert!(!summary.contains("secret-value"));
    assert!(!summary.contains("ghp_test_fixture_invalid_0001"));
}

fn failed_recent_command() -> RecentCommandState {
    RecentCommandState {
        command: "cargo check".to_owned(),
        exit_code: Some(101),
        output_tail: "error[E0308]: mismatched types".to_owned(),
        duration_ms: Some(1_200),
        detected_errors: vec![CommandErrorSpan {
            kind: CommandErrorKind::Rustc,
            message: "mismatched types".to_owned(),
            file: Some("src/layout.rs".to_owned()),
            line: Some(142),
            column: Some(9),
        }],
    }
}
