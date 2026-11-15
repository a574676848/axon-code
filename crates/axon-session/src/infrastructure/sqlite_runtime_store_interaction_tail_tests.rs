//! Interaction 尾窗读取回归：只返回目标交互的帧，不混入同 lane 的其他交互。

use super::SqliteRuntimeStore;
use super::sqlite_runtime_store_interaction_tests::{LEASE_DURATION_MS, sample_interaction};
use crate::{InteractionStore, RuntimeStore, interaction_session_lane_id};
use axon_domain::{DaemonSessionId, RuntimeHostId};

#[test]
fn interaction_tail_window_reads_only_the_target_interaction() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("tail-window-host").expect("host"),
            1,
            None,
            1,
            1_000,
        )
        .expect("host lease");
    let interaction = sample_interaction();
    store.append(&interaction).expect("追加 Interaction");
    let session_id = DaemonSessionId::new(interaction_session_lane_id(&interaction).expect("lane"))
        .expect("Session ID");
    store.create_session(&session_id, ".", 1).expect("Session");
    let lease = store
        .claim_processing_by_id(&interaction.interaction_id, 20, LEASE_DURATION_MS)
        .expect("领取")
        .expect("lease");

    // 另一条交互的事件先写入，确保尾窗不会把它带出来。
    store
        .append_event(
            &session_id,
            &host,
            "wire_event",
            "{\"interactionId\":\"other-interaction\",\"text\":\"other\"}",
            25,
        )
        .expect("另一条交互事件");
    for (index, text) in ["a", "b", "c"].iter().enumerate() {
        store
            .append_interaction_events(
                &session_id,
                &host,
                &interaction.interaction_id,
                &lease.lease_token,
                &[(
                    "wire_event".to_owned(),
                    format!(
                        "{{\"interactionId\":\"{}\",\"text\":\"{text}\"}}",
                        interaction.interaction_id.as_str()
                    ),
                )],
                30 + u64::try_from(index).expect("index"),
            )
            .expect("目标交互事件");
    }

    let window = store
        .interaction_tail_window(&session_id, &interaction.interaction_id, 2)
        .expect("尾窗读取");
    assert_eq!(window.len(), 2, "只取目标交互的尾部两帧");
    assert!(window[0].payload_json.contains("\"text\":\"b\""));
    assert!(window[1].payload_json.contains("\"text\":\"c\""));
    assert!(
        window.iter().all(|event| event
            .payload_json
            .contains(interaction.interaction_id.as_str())),
        "尾窗不得混入其他交互的帧"
    );
}
