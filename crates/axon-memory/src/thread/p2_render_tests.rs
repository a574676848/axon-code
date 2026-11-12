use super::*;
use crate::store_thread::{COMMITMENT_STATUS_OPEN, THREAD_STATUS_OPEN};

fn thread(label: &str, topic: &str, hit_count: i64) -> MemoryThread {
    MemoryThread {
        thread_id: "thread-1".to_owned(),
        namespace_ref: "workspace:current".to_owned(),
        user_ref: "user:local".to_owned(),
        topic: topic.to_owned(),
        signature: String::new(),
        label: label.to_owned(),
        summary: String::new(),
        status: THREAD_STATUS_OPEN.to_owned(),
        hit_count,
        last_event_at_ms: 0,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

fn commitment(id: &str, text: &str, created_at_ms: i64) -> MemoryCommitment {
    MemoryCommitment {
        commitment_id: id.to_owned(),
        thread_id: None,
        namespace_ref: "workspace:current".to_owned(),
        user_ref: "user:local".to_owned(),
        text: text.to_owned(),
        status: COMMITMENT_STATUS_OPEN.to_owned(),
        channel: "cli".to_owned(),
        created_at_ms,
        closed_at_ms: None,
    }
}

#[test]
fn empty_inputs_render_empty_string() {
    assert_eq!(render_p2_context(None, &[], 480), "");
}

#[test]
fn thread_only_renders_fixed_template() {
    let rendered = render_p2_context(Some(&thread("", "周报", 3)), &[], 480);
    assert_eq!(rendered, "## 当前线索\n周报（命中 3 次）");
}

#[test]
fn label_wins_over_topic() {
    let rendered = render_p2_context(Some(&thread("周报收尾", "周报", 1)), &[], 480);
    assert!(rendered.contains("周报收尾（命中 1 次）"));
    assert!(!rendered.contains("周报（命中"));
}

#[test]
fn commitments_render_in_created_desc_order() {
    let rendered = render_p2_context(
        None,
        &[
            commitment("c-1", "先写的", 1_000),
            commitment("c-2", "后写的", 2_000),
        ],
        480,
    );
    let later = rendered.find("后写的").expect("应包含后写的");
    let earlier = rendered.find("先写的").expect("应包含先写的");
    assert!(later < earlier, "承诺必须按 created_at DESC：{rendered}");
}

#[test]
fn commitments_are_capped_at_five() {
    let items = (0..8)
        .map(|index| commitment(&format!("c-{index}"), &format!("承诺{index}"), index as i64))
        .collect::<Vec<_>>();
    let rendered = render_p2_context(None, &items, 480);
    assert_eq!(
        rendered
            .lines()
            .filter(|line| line.starts_with("- "))
            .count(),
        5
    );
}

#[test]
fn render_is_byte_stable() {
    let thread = thread("周报收尾", "周报", 5);
    let items = vec![commitment("c-1", "明天提醒我交周报", 1_760_000_000_000)];
    let first = render_p2_context(Some(&thread), &items, 480);
    let second = render_p2_context(Some(&thread), &items, 480);
    assert_eq!(first, second);
    assert!(first.starts_with("## 当前线索"));
    assert!(first.contains("## 未完成承诺"));
}

#[test]
fn overlong_thread_text_is_truncated_within_budget() {
    let long = "超长线索".repeat(200);
    let rendered = render_p2_context(Some(&thread("", &long, 1)), &[], 480);
    assert!(rendered.len() <= P2_THREAD_MAX_BYTES);
    assert!(rendered.is_char_boundary(rendered.len()));
}

#[test]
fn overlong_commitments_stay_within_budget() {
    let long = "超长承诺".repeat(400);
    let rendered = render_p2_context(None, &[commitment("c-1", &long, 0)], 480);
    assert!(rendered.len() <= P2_COMMITMENT_MAX_BYTES);
}

#[test]
fn local_minute_format_uses_timezone_offset() {
    // 1_760_000_000_000 ms = 2025-10-09 08:53 UTC = 2025-10-09 16:53 +08:00
    let ms = 1_760_000_000_000_i64;
    assert_eq!(format_local_minute(ms, 480), "2025-10-09 16:53");
    assert_eq!(format_local_minute(ms, 0), "2025-10-09 08:53");
}

#[test]
fn blank_commitment_text_is_skipped() {
    let rendered = render_p2_context(None, &[commitment("c-1", "   ", 1)], 480);
    assert_eq!(rendered, "");
}
