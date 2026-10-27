//! 人格库持久化边界用例。

use super::*;
use crate::domain::{
    persona_event::{TurnFinalStatus, TurnSettledInput},
    persona_state::UserAffect,
};

fn settled(now_ms: u64) -> PersonaEvent {
    PersonaEvent::TurnSettled(TurnSettledInput {
        final_status: TurnFinalStatus::Completed,
        user_affect: UserAffect::Neutral,
        persona_drift_bp: 0,
        drift_reason: None,
        now_ms,
    })
}

fn open_temp(dir: &tempfile::TempDir) -> PersonaStore {
    PersonaStore::open(&dir.path().join("persona.sqlite3")).expect("人格库应可打开")
}

#[test]
fn fresh_library_writes_schema_identity() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = open_temp(&dir);

    let identity: String = store
        .connection
        .query_row("SELECT identity FROM persona_schema_identity", [], |row| {
            row.get(0)
        })
        .expect("应写入身份");

    assert_eq!(identity, PERSONA_SCHEMA_IDENTITY);
}

#[test]
fn schema_identity_mismatch_fails_fast() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("persona.sqlite3");
    {
        let connection = Connection::open(&path).expect("可建库");
        connection
            .execute_batch(
                "CREATE TABLE persona_schema_identity (identity TEXT PRIMARY KEY NOT NULL);
                 INSERT INTO persona_schema_identity(identity) VALUES('axon-persona-v0');",
            )
            .expect("写入旧身份");
    }

    let error = PersonaStore::open(&path).expect_err("身份不符必须失败");

    assert_eq!(
        error,
        PersonaStoreError::SchemaMismatch {
            found: "axon-persona-v0".to_owned()
        }
    );
}

#[test]
fn settlement_persists_and_reloads_state() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);

    let outcome = store
        .settle("owner-a", &settled(1_000), Audience::Private, "job-1")
        .expect("结算应成功");

    let SettlementOutcome::Applied { state, cause } = outcome else {
        panic!("期望写入成功，实际为 {outcome:?}");
    };
    assert_eq!(state.revision, 1);
    assert_eq!(cause, "任务完成");
    assert_eq!(store.load_state("owner-a").unwrap(), Some(state));
    assert_eq!(store.transition_count("owner-a").unwrap(), 1);
}

#[test]
fn missing_owner_has_no_state_instead_of_a_fake_baseline() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = open_temp(&dir);

    assert_eq!(store.load_state("nobody").unwrap(), None);
}

#[test]
fn replaying_the_same_idempotency_key_never_double_applies() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);

    let first = store
        .settle("owner-a", &settled(1_000), Audience::Private, "job-1")
        .expect("首次结算");
    let second = store
        .settle("owner-a", &settled(2_000), Audience::Private, "job-1")
        .expect("重放");
    let third = store
        .settle("owner-a", &settled(3_000), Audience::Private, "job-1")
        .expect("再重放");

    assert!(matches!(first, SettlementOutcome::Applied { .. }));
    assert_eq!(second, SettlementOutcome::AlreadyApplied);
    assert_eq!(third, SettlementOutcome::AlreadyApplied);
    assert_eq!(store.load_state("owner-a").unwrap().unwrap().revision, 1);
    assert_eq!(store.transition_count("owner-a").unwrap(), 1);
}

#[test]
fn group_audience_is_not_persisted() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);

    let outcome = store
        .settle("owner-a", &settled(1_000), Audience::Group, "job-1")
        .expect("群聊结算不报错");

    assert_eq!(outcome, SettlementOutcome::Unchanged);
    assert_eq!(store.load_state("owner-a").unwrap(), None);
    assert_eq!(store.transition_count("owner-a").unwrap(), 0);
}

#[test]
fn empty_reset_reason_surfaces_domain_error() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);

    let error = store
        .settle(
            "owner-a",
            &PersonaEvent::Reset {
                reason: " ".to_owned(),
                now_ms: 1,
            },
            Audience::Private,
            "reset-1",
        )
        .expect_err("空原因必须被拒绝");

    assert_eq!(error, PersonaStoreError::Domain(PersonaError::EmptyCause));
}

#[test]
fn retention_keeps_only_the_newest_transitions() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);
    for index in 0..5u64 {
        store
            .insert_synthetic_transition("owner-a", &format!("k{index}"), 1_000 + index)
            .expect("插入合成转移");
    }
    assert_eq!(store.transition_count("owner-a").unwrap(), 5);

    let remaining = store.prune_for_test("owner-a", 3).expect("裁剪");

    assert_eq!(remaining, 3);
}

#[test]
fn concurrent_settlements_serialize_under_one_owner() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("persona.sqlite3");
    PersonaStore::open(&path).expect("建库");
    let handles = (0..4u64)
        .map(|index| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut store = PersonaStore::open(&path).expect("打开人格库");
                store
                    .settle(
                        "owner-a",
                        &settled(1_000 + index),
                        Audience::Private,
                        &format!("job-{index}"),
                    )
                    .expect("并发结算应成功")
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        assert!(matches!(
            handle.join().expect("线程不应 panic"),
            SettlementOutcome::Applied { .. }
        ));
    }

    let store = PersonaStore::open(&path).expect("复读人格库");
    assert_eq!(store.load_state("owner-a").unwrap().unwrap().revision, 4);
    assert_eq!(store.transition_count("owner-a").unwrap(), 4);
}

#[test]
fn pending_style_publishes_once_then_respects_interval() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);
    assert!(
        store
            .add_pending_style("owner-a", "少用寒暄", "evidence://1", 1_000)
            .unwrap()
    );

    let first = store.publish_style_if_due("owner-a", 2_000).unwrap();
    assert_eq!(first, Some(1));
    assert_eq!(store.published_style("owner-a").unwrap().entries.len(), 1);

    assert!(
        store
            .add_pending_style("owner-a", "结论先行", "evidence://2", 3_000)
            .unwrap()
    );
    let too_soon = store
        .publish_style_if_due(
            "owner-a",
            2_000 + crate::constants::STYLE_PUBLISH_INTERVAL_MS - 1,
        )
        .unwrap();
    assert_eq!(too_soon, None, "限频窗口内不得发布");

    let after_interval = store
        .publish_style_if_due(
            "owner-a",
            2_000 + crate::constants::STYLE_PUBLISH_INTERVAL_MS,
        )
        .unwrap();
    assert_eq!(after_interval, Some(2));
    assert_eq!(store.published_style("owner-a").unwrap().entries.len(), 2);
}

#[test]
fn style_input_is_validated_and_deduplicated() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);

    assert!(!store.add_pending_style("owner-a", "   ", "e", 1).unwrap());
    assert!(
        !store
            .add_pending_style(
                "owner-a",
                &"长".repeat(crate::constants::MAX_LEARNED_STYLE_CHARS + 1),
                "e",
                2
            )
            .unwrap()
    );
    assert!(
        store
            .add_pending_style("owner-a", "少用寒暄", "e", 3)
            .unwrap()
    );
    assert!(
        !store
            .add_pending_style("owner-a", "少用寒暄", "e", 4)
            .unwrap(),
        "同一条风格不得重复入库"
    );
}

#[test]
fn published_style_count_is_capped_and_oldest_retires() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);
    let overflow = crate::constants::MAX_PUBLISHED_STYLE + 4;
    for index in 0..overflow {
        assert!(
            store
                .add_pending_style(
                    "owner-a",
                    &format!("风格条目{index}"),
                    "evidence://x",
                    1_000 + u64::try_from(index).unwrap(),
                )
                .unwrap()
        );
    }

    let revision = store.publish_style_if_due("owner-a", 200_000).unwrap();
    let published = store.published_style("owner-a").unwrap();

    assert_eq!(revision, Some(1));
    assert_eq!(
        published.entries.len(),
        crate::constants::MAX_PUBLISHED_STYLE
    );
    assert!(
        published
            .entries
            .iter()
            .all(|entry| entry.text != "风格条目0"),
        "最旧的条目应被退役"
    );
}

#[test]
fn retire_marks_style_unavailable_and_is_idempotent() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);
    store
        .add_pending_style("owner-a", "少用寒暄", "e", 1_000)
        .unwrap();
    store.publish_style_if_due("owner-a", 2_000).unwrap();
    let style_id = store.published_style("owner-a").unwrap().entries[0]
        .style_id
        .clone();

    assert!(store.retire_style("owner-a", &style_id).unwrap());
    assert!(!store.retire_style("owner-a", &style_id).unwrap());
    assert_eq!(store.published_style("owner-a").unwrap().entries.len(), 0);
}

#[test]
fn read_only_open_rejects_missing_or_foreign_library() {
    let dir = tempfile::tempdir().expect("临时目录");
    let missing = dir.path().join("persona.sqlite3");
    assert!(
        PersonaStore::open_read_only(&missing).is_err(),
        "库不存在必须失败"
    );

    let foreign = dir.path().join("foreign.sqlite3");
    {
        let connection = Connection::open(&foreign).expect("建库");
        connection
            .execute_batch("CREATE TABLE other (id INTEGER PRIMARY KEY);")
            .expect("建表");
    }
    assert!(
        PersonaStore::open_read_only(&foreign).is_err(),
        "非人格库必须失败"
    );
}

#[test]
fn read_only_open_never_creates_the_library() {
    let dir = tempfile::tempdir().expect("临时目录");
    let mut store = open_temp(&dir);
    store
        .add_pending_style("owner-a", "少用寒暄", "e", 1_000)
        .expect("写入风格");
    drop(store);
    let path = dir.path().join("persona.sqlite3");
    let before = std::fs::metadata(&path).expect("库存在").len();

    let read_only = PersonaStore::open_read_only(&path).expect("只读打开应成功");
    assert!(read_only.load_state("owner-a").is_ok());

    let after = std::fs::metadata(&path).expect("库存在").len();
    assert_eq!(before, after, "只读路径不得改变库体积");
}
