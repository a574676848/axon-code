//! 会话事件正文分级与回读回归：分级后日志只剩标记，回读按归档回源。

use super::SqliteRuntimeStore;
use crate::RuntimeStore;
use crate::ports::EventLogStore as _;
use axon_domain::{DaemonSessionId, RuntimeHostId};

/// 正文判定阈值是 4 KiB：测试载荷必须明显超过它。
const BODY_TEXT: usize = 16 * 1024;

struct Fixture {
    /// 临时根：必须与 store 同生命周期，drop 之后归档与日志都不可读。
    _directory: axon_test_support::TestTempDir,
    store: SqliteRuntimeStore,
    session_id: DaemonSessionId,
    host: crate::domain::DaemonHostRecord,
}

impl Fixture {
    fn new() -> Self {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let host = store
            .claim_host(
                &RuntimeHostId::new("body-tiering-host").expect("host"),
                1,
                None,
                1,
                1_000,
            )
            .expect("host lease");
        let session_id = DaemonSessionId::new("session-body-tiering").expect("会话标识");
        store.create_session(&session_id, ".", 1).expect("建会话");
        Self {
            _directory: directory,
            store,
            session_id,
            host,
        }
    }

    fn append(&self, interaction_id: &str, text: &str, now_ms: u64) {
        let payload = serde_json::json!({
            "interactionId": interaction_id,
            "kind": { "type": "textDelta", "payload": { "text": text } },
        });
        self.store
            .append_attributed_events(
                &self.session_id,
                &self.host,
                &axon_domain::InteractionId::new(interaction_id.to_owned()).expect("交互标识"),
                &[("wire_event".to_owned(), payload.to_string())],
                now_ms,
            )
            .expect("追加事件");
    }

    fn log_path(&self) -> std::path::PathBuf {
        self.store.event_log(&self.session_id).path()
    }

    fn log_len(&self) -> u64 {
        std::fs::metadata(self.log_path()).map_or(0, |meta| meta.len())
    }

    fn archive_path(&self, interaction_id: &str) -> std::path::PathBuf {
        crate::body_archive_path(
            &self.store.event_archive_root(),
            self.session_id.as_str(),
            interaction_id,
        )
    }
}

#[test]
fn tiering_moves_old_bodies_to_the_archive_and_keeps_the_log_small() {
    let fixture = Fixture::new();
    let old = "interaction-old";
    let recent = "interaction-recent";
    fixture.append(old, &"x".repeat(BODY_TEXT), 10);
    fixture.append(old, &"y".repeat(BODY_TEXT), 11);
    fixture.append(recent, &"z".repeat(BODY_TEXT), 12);
    let log_before = fixture.log_len();

    let plan = fixture
        .store
        .plan_event_body_tiering(&fixture.session_id, 1)
        .expect("算账")
        .expect("有可分级正文");
    assert_eq!(plan.tiered_interactions, 1, "只分级最旧的那条交互");
    assert_eq!(plan.tiered_frames, 2);
    assert_eq!(fixture.log_len(), log_before, "dry-run 不得改动日志文件");

    let report = fixture
        .store
        .tier_event_body(&fixture.session_id, 1)
        .expect("分级")
        .expect("应真的分级");
    assert_eq!(report.tiered_interactions, 1);
    assert_eq!(report.tiered_frames, 2);
    assert!(report.tiered_bytes >= (BODY_TEXT as u64) * 2);
    assert_eq!(report.skipped_interactions, 0);
    assert!(
        report.log_bytes_after < report.log_bytes_before,
        "正文离开日志后日志必须变小：{} -> {}",
        report.log_bytes_before,
        report.log_bytes_after
    );
    assert!(fixture.archive_path(old).is_file(), "归档必须落盘");
    assert!(
        !fixture.archive_path(recent).exists(),
        "保留窗口内的交互不得被分级"
    );
}

#[test]
fn keyed_reads_hydrate_the_archived_body_back() {
    let fixture = Fixture::new();
    let interaction_id = axon_domain::InteractionId::new("interaction-old".to_owned()).unwrap();
    fixture.append("interaction-old", &"x".repeat(BODY_TEXT), 10);
    fixture.append("interaction-recent", &"z".repeat(BODY_TEXT), 11);
    fixture
        .store
        .tier_event_body(&fixture.session_id, 1)
        .expect("分级")
        .expect("应真的分级");

    let window = fixture
        .store
        .interaction_tail_window(&fixture.session_id, &interaction_id, 10)
        .expect("尾窗读取");
    assert_eq!(window.len(), 1, "标记帧必须被正文替换，而不是被丢弃");
    assert_eq!(window[0].event_type, "wire_event");
    assert!(
        window[0].payload_json.len() >= BODY_TEXT,
        "回读必须拿回完整正文"
    );

    let paged = fixture
        .store
        .replay_interaction_events(&fixture.session_id, &interaction_id, 0, 10)
        .expect("按键读取");
    assert_eq!(paged.len(), 1);
    assert_eq!(paged[0].event_type, "wire_event");

    // 整条会话回放同样要拿到正文：CLI 的会话历史就是这条路径重建的。
    let replay = fixture
        .store
        .replay_events(&fixture.session_id, 0, 100)
        .expect("整条会话回放");
    assert_eq!(replay.len(), 2, "两条交互各一帧");
    assert!(
        replay
            .iter()
            .any(|event| event.payload_json.len() >= BODY_TEXT),
        "整条会话回放必须从归档回源，而不是拿到标记载荷"
    );
}

#[test]
fn reclaimed_archive_degrades_to_a_placeholder_instead_of_an_empty_turn() {
    let fixture = Fixture::new();
    let interaction_id = axon_domain::InteractionId::new("interaction-old".to_owned()).unwrap();
    fixture.append("interaction-old", &"x".repeat(BODY_TEXT), 10);
    fixture.append("interaction-recent", &"z".repeat(BODY_TEXT), 11);
    fixture
        .store
        .tier_event_body(&fixture.session_id, 1)
        .expect("分级")
        .expect("应真的分级");
    std::fs::remove_file(fixture.archive_path("interaction-old")).expect("模拟归档被回收");

    let window = fixture
        .store
        .interaction_tail_window(&fixture.session_id, &interaction_id, 10)
        .expect("尾窗读取");
    assert_eq!(window.len(), 1, "占位帧必须保留序号，不能整条消失");
    assert_eq!(
        window[0].event_type,
        crate::SESSION_EVENT_MISSING_ARCHIVE_TYPE
    );
    assert!(window[0].payload_json.contains("reclaimed"));
}

#[test]
fn metrics_and_small_frames_never_leave_the_log() {
    let fixture = Fixture::new();
    fixture.append("interaction-old", &"x".repeat(BODY_TEXT), 10);
    fixture.append("interaction-old", "短正文", 11);
    fixture.append("interaction-recent", &"z".repeat(BODY_TEXT), 12);
    fixture
        .store
        .tier_event_body(&fixture.session_id, 1)
        .expect("分级")
        .expect("应真的分级");

    let interaction_id = axon_domain::InteractionId::new("interaction-old".to_owned()).unwrap();
    let window = fixture
        .store
        .interaction_tail_window(&fixture.session_id, &interaction_id, 10)
        .expect("尾窗读取");
    assert_eq!(window.len(), 2, "两帧都必须保留");
    assert!(
        window[1].payload_json.contains("短正文"),
        "未达阈值的正文原样留在日志里"
    );
}

#[test]
fn second_run_is_a_no_op_once_every_body_is_tiered() {
    let fixture = Fixture::new();
    fixture.append("interaction-old", &"x".repeat(BODY_TEXT), 10);
    fixture.append("interaction-recent", &"z".repeat(BODY_TEXT), 11);
    assert!(
        fixture
            .store
            .tier_event_body(&fixture.session_id, 1)
            .expect("首次分级")
            .is_some()
    );
    assert!(
        fixture
            .store
            .tier_event_body(&fixture.session_id, 1)
            .expect("二次分级")
            .is_none(),
        "没有可分级正文时不得重写日志"
    );
}
