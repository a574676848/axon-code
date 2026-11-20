//! 最新一轮事实的读取口径：轮序最大者胜出，增量按轮归属套用。

use super::*;

#[test]
fn latest_turn_facts_take_the_highest_turn_sequence_over_a_late_patch() {
    let root = temp_root("latest-turn-facts");
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut first = session_record("第一轮", "第一轮结论", 100);
    first.reasoning_effort = Some("medium".to_owned());
    first.model_name = Some("model-first".to_owned());
    let mut second = session_record("第二轮", "第二轮结论", 200);
    second.reasoning_effort = Some("max".to_owned());
    second.model_name = Some("model-second".to_owned());
    second.assistant_part_id = "assistant:session-1:second".to_owned();
    second.turn_events = Vec::new();
    let thread_id = first.thread_id.clone();
    let first_turn = turn_from_record(&first, 0).unwrap();
    let second_turn = turn_from_record(&second, 1).unwrap();
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: thread_id.clone(),
                session_root_id: thread_id.clone(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings_from_record(&first),
                created_at_ms: first.created_at_ms,
            },
        )
        .unwrap();
    for (recorded_at_ms, turn) in [
        (first.created_at_ms, first_turn.clone()),
        (second.created_at_ms, second_turn),
    ] {
        service
            .append_turn(
                &root,
                AppendTurnCommand {
                    thread_id: thread_id.clone(),
                    recorded_at_ms,
                    turn,
                },
            )
            .unwrap();
    }
    // 治理回写晚到：给第一轮补一条反思提示增量，追加在第二轮之后。
    service
        .append_turn_metadata_patch(
            &root,
            &thread_id,
            300,
            &first.assistant_part_id,
            Some("记忆反思：本轮新增 1 条记忆"),
            &[],
        )
        .unwrap();

    let facts = latest_turn_facts_at_root(&root, &thread_id).expect("尾部必须给出最新一轮事实");
    assert_eq!(
        facts.reasoning_effort.as_deref(),
        Some("max"),
        "轮序最大的那一轮才是最新，不能按追加位置取"
    );
    assert_eq!(facts.model_name.as_deref(), Some("model-second"));
    assert!(
        facts.reflection_notice.is_none(),
        "晚到的旧轮增量不能冒充最新一轮的反思提示"
    );

    // 同一轮自己的增量必须生效。
    service
        .append_turn_metadata_patch(
            &root,
            &thread_id,
            400,
            &second.assistant_part_id,
            Some("记忆反思：本轮新增 2 条记忆"),
            &[],
        )
        .unwrap();
    let patched = latest_turn_facts_at_root(&root, &thread_id).expect("增量落地后仍可读");
    assert_eq!(
        patched.reflection_notice.as_deref(),
        Some("记忆反思：本轮新增 2 条记忆")
    );
    assert!(
        latest_turn_facts_at_root(&root, "thread-missing").is_none(),
        "读不到就当作没有"
    );
}
