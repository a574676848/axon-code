use super::*;
use axon_domain::AxonError;

fn spawn_request(parent_path: AgentPath, task_name: &str) -> AgentSpawnRequest {
    AgentSpawnRequest {
        parent_path,
        task_name: task_name.to_owned(),
        message: format!("处理 {task_name}"),
        nickname: None,
        role: None,
    }
}

#[test]
fn agent_path_resolves_canonical_and_relative_references() {
    let root = AgentPath::root();
    let review = root.join("review").unwrap();
    let refactor_auth = root.join("refactor-auth").unwrap();
    let security = review.resolve("security").unwrap();

    assert_eq!(review.as_str(), "/root/review");
    assert_eq!(refactor_auth.as_str(), "/root/refactor-auth");
    assert_eq!(security.as_str(), "/root/review/security");
    assert_eq!(review.resolve("/root/review/security").unwrap(), security);
    assert!(AgentPath::parse("review").is_err());
    assert!(review.resolve("Security").is_err());
}

#[test]
fn control_runtime_spawns_lists_and_closes_agent_tree() {
    let mut runtime = AgentControlRuntime::default();
    let review = runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();
    runtime
        .update_status(&review.path, AgentStatus::Running)
        .unwrap();
    let security = runtime
        .spawn(spawn_request(review.path.clone(), "security"))
        .unwrap();

    let listed = runtime
        .list_agents(&AgentPath::root(), Some("review"))
        .unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|agent| agent.agent_name.as_str())
            .collect::<Vec<_>>(),
        ["/root/review", "/root/review/security"]
    );
    assert_eq!(
        runtime
            .resolve_agent(&review.path, "security")
            .unwrap()
            .thread_id,
        security.thread_id
    );

    let close = runtime.close(&review.path).unwrap();
    assert_eq!(close.previous_status, AgentStatus::Running);
    assert_eq!(close.closed_count, 2);
    assert_eq!(runtime.active_agent_count(), 0);
    assert!(runtime.open_descendant_paths(&AgentPath::root()).is_empty());
}

#[test]
fn control_runtime_rejects_duplicate_path_and_limit_overflow() {
    let mut runtime = AgentControlRuntime::new(AgentControlConfig {
        max_agents: 1,
        max_active_turns: 1,
        max_depth: 1,
    });
    runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();

    assert!(matches!(
        runtime.spawn(spawn_request(AgentPath::root(), "review")),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_PATH_EXISTS
    ));
    assert!(matches!(
        runtime.spawn(spawn_request(AgentPath::root(), "build")),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_LIMIT_REACHED
    ));
}

#[test]
fn control_runtime_spawn_reservation_releases_slot_path_and_nickname_on_drop() {
    let mut runtime = AgentControlRuntime::new(AgentControlConfig {
        max_agents: 2,
        max_active_turns: 2,
        max_depth: 1,
    });
    let request = AgentSpawnRequest {
        nickname: Some("worker".to_owned()),
        ..spawn_request(AgentPath::root(), "review")
    };
    let reservation = runtime.reserve_spawn(&request).unwrap();

    assert_eq!(runtime.occupied_agent_slots(), 1);
    assert!(matches!(
        runtime.reserve_spawn(&request),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_PATH_EXISTS
    ));
    assert!(matches!(
        runtime.reserve_spawn(&AgentSpawnRequest {
            nickname: Some("worker".to_owned()),
            ..spawn_request(AgentPath::root(), "build")
        }),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_NICKNAME_EXISTS
    ));
    drop(reservation);

    assert_eq!(runtime.occupied_agent_slots(), 0);
    let spawned = runtime.spawn(request).unwrap();
    assert_eq!(spawned.path.as_str(), "/root/review");
    assert_eq!(spawned.nickname.as_deref(), Some("worker"));
}

#[test]
fn control_runtime_releases_nickname_after_close() {
    let mut runtime = AgentControlRuntime::default();
    let request = AgentSpawnRequest {
        nickname: Some("worker".to_owned()),
        ..spawn_request(AgentPath::root(), "review")
    };
    let spawned = runtime.spawn(request).unwrap();
    assert!(matches!(
        runtime.spawn(AgentSpawnRequest {
            nickname: Some("worker".to_owned()),
            ..spawn_request(AgentPath::root(), "build")
        }),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_NICKNAME_EXISTS
    ));

    runtime.close(&spawned.path).unwrap();
    let respawned = runtime
        .spawn(AgentSpawnRequest {
            nickname: Some("worker".to_owned()),
            ..spawn_request(AgentPath::root(), "build")
        })
        .unwrap();
    assert_eq!(respawned.nickname, Some("worker".to_owned()));
}

#[test]
fn control_runtime_allows_nested_spawn_until_configured_depth() {
    let mut runtime = AgentControlRuntime::new(AgentControlConfig {
        max_agents: 4,
        max_active_turns: 4,
        max_depth: 2,
    });
    let first = runtime
        .spawn(spawn_request(AgentPath::root(), "first"))
        .unwrap();
    let second = runtime.spawn(spawn_request(first.path, "second")).unwrap();

    assert_eq!(second.path.as_str(), "/root/first/second");
    assert!(matches!(
        runtime.spawn(spawn_request(second.path, "third")),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_DEPTH_LIMIT_REACHED
    ));
}

#[test]
fn control_runtime_reopens_closed_edge_without_duplicate_tree_entries() {
    let mut runtime = AgentControlRuntime::default();
    let first = runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();
    runtime.close(&first.path).unwrap();
    let second = runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();

    let descendants = runtime.open_descendant_paths(&AgentPath::root());
    assert_eq!(second.path, AgentPath::parse("/root/review").unwrap());
    assert_eq!(descendants, [second.path]);
}

#[test]
fn control_runtime_limits_real_execution_leases_and_releases_on_close() {
    let mut runtime = AgentControlRuntime::new(AgentControlConfig {
        max_agents: 1,
        max_active_turns: 1,
        max_depth: 1,
    });
    let review = runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();

    let lease = runtime.lease_execution(&review.path).unwrap();
    assert_eq!(runtime.active_execution_count(), 1);
    assert!(matches!(
        runtime.lease_execution(&review.path),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_ALREADY_RUNNING
    ));

    let report = runtime
        .complete_execution(&lease, AgentStatus::Completed(None), 42)
        .unwrap();
    assert!(report.execution_applied);
    let lease = runtime.lease_execution(&review.path).unwrap();
    runtime.close(&review.path).unwrap();
    assert_eq!(runtime.active_execution_count(), 0);
    let report = runtime
        .complete_execution(&lease, AgentStatus::Completed(None), 43)
        .unwrap();
    assert!(!report.execution_applied);
    assert_eq!(runtime.active_execution_count(), 0);
}

#[test]
fn control_runtime_limits_active_turns_independently_from_agent_count() {
    let mut runtime = AgentControlRuntime::new(AgentControlConfig {
        max_agents: 2,
        max_active_turns: 1,
        max_depth: 1,
    });
    let review = runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();
    let build = runtime
        .spawn(spawn_request(AgentPath::root(), "build"))
        .unwrap();

    let lease = runtime.lease_execution(&review.path).unwrap();
    assert!(matches!(
        runtime.lease_execution(&build.path),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_EXECUTION_LIMIT_REACHED
    ));

    let report = runtime
        .complete_execution(&lease, AgentStatus::Completed(None), 42)
        .unwrap();
    assert!(report.execution_applied);
    assert!(runtime.lease_execution(&build.path).is_ok());
}

#[test]
fn control_runtime_interrupt_releases_execution_lease() {
    let mut runtime = AgentControlRuntime::default();
    let review = runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();
    let lease = runtime.lease_execution(&review.path).unwrap();

    let report = runtime.interrupt(&review.path).unwrap();

    assert_eq!(report.previous_status, AgentStatus::Running);
    assert_eq!(report.status, AgentStatus::Interrupted);
    assert_eq!(report.execution_id, lease.execution_id());
    assert_eq!(runtime.active_execution_count(), 0);
}

#[test]
fn control_runtime_interrupt_rejects_agent_without_active_execution() {
    let mut runtime = AgentControlRuntime::default();
    let review = runtime
        .spawn(spawn_request(AgentPath::root(), "review"))
        .unwrap();
    let lease = runtime.lease_execution(&review.path).unwrap();
    runtime
        .complete_execution(&lease, AgentStatus::Completed(Some("done".to_owned())), 42)
        .unwrap();

    let error = runtime.interrupt(&review.path).unwrap_err();

    assert!(matches!(
        error,
        AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason))
            if reason == AGENT_CONTROL_REASON_NO_ACTIVE_EXECUTION
    ));
    assert_eq!(
        runtime.status(&review.path),
        AgentStatus::Completed(Some("done".to_owned()))
    );
}

#[test]
fn control_runtime_rejects_root_execution_and_interrupt() {
    let mut runtime = AgentControlRuntime::default();

    assert!(matches!(
        runtime.lease_execution(&AgentPath::root()),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_NOT_FOUND
    ));
    assert!(matches!(
        runtime.interrupt(&AgentPath::root()),
        Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidState(reason)))
            if reason == AGENT_CONTROL_REASON_NOT_FOUND
    ));
}
