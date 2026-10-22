use super::*;

#[test]
fn store_keeps_only_current_state_for_each_skill() {
    let root = axon_test_support::test_temp_dir();
    let store = SkillEvolutionStore::open_at(root.path()).unwrap();
    for stage in [
        SkillEvolutionStage::Candidate,
        SkillEvolutionStage::Draft,
        SkillEvolutionStage::Validated,
    ] {
        store
            .record_current(&SkillEvolutionRecord {
                session_id: "session-1".to_owned(),
                kind: "draft".to_owned(),
                skill_name: "release-review".to_owned(),
                risk: "low".to_owned(),
                evidence_refs: vec!["receipt:1".to_owned()],
                stage,
                material_ref: None,
                revision: None,
                reason: "fixture".to_owned(),
                recorded_at_ms: 1,
            })
            .unwrap();
    }

    let records = store.records_for_session("session-1").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].stage, SkillEvolutionStage::Validated);
    assert!(store.state_file().ends_with("skill-evolution/state.json"));
}

#[test]
fn missing_state_is_an_empty_current_snapshot() {
    let root = axon_test_support::test_temp_dir();
    let store = SkillEvolutionStore::open_at(root.path()).unwrap();

    assert!(
        store
            .records_for_session("session-without-draft")
            .unwrap()
            .is_empty()
    );
}
