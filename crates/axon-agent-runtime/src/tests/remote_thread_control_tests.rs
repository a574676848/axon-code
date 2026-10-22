use axon_domain::{AxonError, RemoteAction, RemoteCapabilityPolicy, RemoteDenialReason, SessionId};

use super::{
    REMOTE_THREAD_LIST_LIMIT, REMOTE_THREAD_REASON_LOCAL_EXECUTION_REQUIRED,
    RemoteThreadControlAction, RemoteThreadControlRequest, RemoteThreadControlRuntime,
    RemoteThreadState, RemoteThreadSummary,
};

#[test]
fn remote_thread_control_plans_start_as_local_execution_without_backend_switch() {
    let report = runtime()
        .plan(RemoteThreadControlRequest {
            action: RemoteThreadControlAction::StartNew {
                bounded_instruction: "启动一个代码审查线程".to_owned(),
                title: Some("代码审查".to_owned()),
            },
            known_threads: threads(),
        })
        .unwrap();

    assert!(report.command.local_execution_required);
    assert!(!report.command.remote_backend_switch_allowed);
    assert!(report.threads.is_empty());
    assert!(
        report
            .reasons
            .contains(&REMOTE_THREAD_REASON_LOCAL_EXECUTION_REQUIRED.to_owned())
    );
}

#[test]
fn remote_thread_control_continues_known_busy_thread_as_queued_command() {
    let report = runtime()
        .plan(RemoteThreadControlRequest {
            action: RemoteThreadControlAction::ContinueExisting {
                session_id: SessionId(2),
                bounded_instruction: "补充：优先跑测试".to_owned(),
                queued: true,
            },
            known_threads: threads(),
        })
        .unwrap();

    assert_eq!(
        report.command.action,
        RemoteThreadControlAction::ContinueExisting {
            session_id: SessionId(2),
            bounded_instruction: "补充：优先跑测试".to_owned(),
            queued: true,
        }
    );
    assert!(report.command.local_execution_required);
}

#[test]
fn remote_thread_control_lists_and_queries_bounded_threads() {
    let list_report = runtime()
        .plan(RemoteThreadControlRequest {
            action: RemoteThreadControlAction::ListThreads { limit: 1 },
            known_threads: threads(),
        })
        .unwrap();
    let status_report = runtime()
        .plan(RemoteThreadControlRequest {
            action: RemoteThreadControlAction::QueryStatus {
                session_id: SessionId(2),
            },
            known_threads: threads(),
        })
        .unwrap();

    assert_eq!(list_report.threads.len(), 1);
    assert_eq!(list_report.threads[0].session_id, SessionId(1));
    assert_eq!(status_report.threads.len(), 1);
    assert_eq!(status_report.threads[0].state, RemoteThreadState::Busy);
}

#[test]
fn remote_thread_control_fails_closed_for_unknown_thread_policy_and_limit() {
    let denied_policy = RemoteCapabilityPolicy {
        allow_bounded_summary: true,
        allow_attachment_upload: true,
        allow_output_ref_grant: true,
        allow_thread_control: false,
    };
    let denied = RemoteThreadControlRuntime::new(denied_policy).plan(RemoteThreadControlRequest {
        action: RemoteThreadControlAction::ListThreads { limit: 1 },
        known_threads: threads(),
    });
    let unknown = runtime().plan(RemoteThreadControlRequest {
        action: RemoteThreadControlAction::QueryStatus {
            session_id: SessionId(404),
        },
        known_threads: threads(),
    });
    let invalid_limit = runtime().plan(RemoteThreadControlRequest {
        action: RemoteThreadControlAction::ListThreads {
            limit: REMOTE_THREAD_LIST_LIMIT + 1,
        },
        known_threads: threads(),
    });

    assert_eq!(
        denied,
        Err(AxonError::RemoteDenied {
            action: RemoteAction::ControlThread,
            reason: RemoteDenialReason::NotGrantedByPolicy,
        })
    );
    assert_eq!(unknown, Err(AxonError::runtime_failure("远控线程不存在")));
    assert_eq!(
        invalid_limit,
        Err(AxonError::runtime_failure("远控线程列表 limit 超出范围"))
    );
}

fn runtime() -> RemoteThreadControlRuntime {
    RemoteThreadControlRuntime::new(RemoteCapabilityPolicy::default_sidecar())
}

fn threads() -> Vec<RemoteThreadSummary> {
    vec![
        RemoteThreadSummary {
            session_id: SessionId(1),
            title: "空闲线程".to_owned(),
            state: RemoteThreadState::Idle,
            queued_message_count: 0,
        },
        RemoteThreadSummary {
            session_id: SessionId(2),
            title: "运行中线程".to_owned(),
            state: RemoteThreadState::Busy,
            queued_message_count: 1,
        },
    ]
}
