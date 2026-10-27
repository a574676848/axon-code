//! 语音回合编排的无 IO 单测：epoch、停播回执与朗读切分。

use super::{
    DrainedOutcome, TurnPhase, VoiceTurn, drain_timeout_json, stop_audio_json,
    submit_rejected_json, turn_json,
};
use std::time::{Duration, Instant};

/// 合同值：停播到回执 10 s（PROTOCOL §5）。
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

fn voice() -> VoiceTurn {
    VoiceTurn::new(DRAIN_TIMEOUT)
}

#[test]
fn epoch_is_allocated_only_on_success() {
    let mut voice = voice();
    voice.rejected();
    voice.rejected();
    assert_eq!(voice.epoch(), 0, "被拒绝的提交不得分配轮次");
    assert_eq!(voice.phase(), TurnPhase::Listening);

    let first = voice.submitted("interaction-a");
    assert_eq!(first.epoch, 1, "会话内 epoch 从 1 起");
    assert_eq!(first.turn_id, "interaction-a:1");

    // 提交成功之后再被拒绝，不得回退或消耗 epoch。
    voice.rejected();
    assert_eq!(voice.epoch(), 1);
    assert_eq!(voice.phase(), TurnPhase::Thinking);

    let (_, stop) = voice.on_turn_completed(Instant::now(), None);
    assert_eq!(stop.expect("应停播").epoch, 1);
    assert_eq!(
        voice.on_playback_drained(1),
        DrainedOutcome::Accepted,
        "回执应命中当前轮次"
    );
    let second = voice.submitted("interaction-b");
    assert_eq!(second.epoch, 2, "下一轮在上一轮回执之后才分配");
}

#[test]
fn submit_rejected_frame_keeps_contract_fields() {
    let value: serde_json::Value =
        serde_json::from_str(&submit_rejected_json("网关拒绝了本次输入")).expect("合法 JSON");
    assert_eq!(value["type"], "error");
    assert_eq!(value["code"], "submit_rejected");
    assert_eq!(value["stage"], "thinking");
    assert_eq!(value["retryable"], false);
    assert_eq!(value["message"], "本轮未能提交：网关拒绝了本次输入");
}

#[test]
fn speaks_start_at_one_and_carry_turn_identity() {
    let mut voice = voice();
    let assigned = voice.submitted("interaction-1");
    assert!(!voice.on_assistant_delta("第一句。").is_empty());

    let speaks = voice.on_assistant_delta("第二句。");
    assert_eq!(speaks.len(), 1);
    assert_eq!(speaks[0].seq, 2, "seq 每轮从 1 起、逐句自增");
    assert_eq!(speaks[0].epoch, assigned.epoch);
    assert_eq!(speaks[0].turn_id, assigned.turn_id);
    assert_eq!(voice.phase(), TurnPhase::Speaking);

    let value: serde_json::Value =
        serde_json::from_str(&turn_json("interaction-1", &assigned, "lane-1", "lane-1"))
            .expect("合法 JSON");
    assert_eq!(value["type"], "turn");
    assert_eq!(value["interactionId"], "interaction-1");
    assert_eq!(value["turnId"], "interaction-1:1");
    assert_eq!(value["laneId"], "lane-1");
    assert_eq!(value["sessionId"], "lane-1");
    assert_eq!(value["epoch"], 1);
}

#[test]
fn speak_sequence_is_monotonic_across_deltas() {
    let mut voice = voice();
    voice.submitted("interaction-1");
    let mut all = voice.on_assistant_delta("第一句。");
    all.extend(voice.on_assistant_delta("第二句。"));
    all.extend(voice.on_assistant_delta("第三句。"));
    let seqs: Vec<u32> = all.iter().map(|speak| speak.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
}

#[test]
fn render_keeps_code_urls_paths_and_tokens_out_of_speech() {
    let mut voice = voice();
    voice.submitted("interaction-1");
    let mut speaks = voice.on_assistant_delta(
        "```rust\nfn main() { secret(); }\n```\n完成后请查看 https://example.com/private 与 C:\\Users\\me\\a.txt，令牌 sk-abcdefghijklmnop 不要外泄。",
    );
    speaks.extend(voice.on_turn_completed(Instant::now(), None).0);
    let spoken = speaks
        .iter()
        .map(|speak| speak.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(!spoken.contains("fn main"), "代码块不得外读：{spoken}");
    assert!(
        !spoken.contains("example.com/private"),
        "URL 路径不得外读：{spoken}"
    );
    assert!(!spoken.contains("C:\\Users"), "绝对路径不得外读：{spoken}");
    assert!(!spoken.contains("sk-abcdef"), "令牌不得外读：{spoken}");
    assert!(spoken.contains("代码已写入"), "代码块只读概括：{spoken}");
    assert!(spoken.contains("a.txt"), "路径只保留文件名：{spoken}");
    assert!(spoken.contains("不要外泄"), "普通正文必须保留：{spoken}");
}

#[test]
fn turn_end_stops_audio_and_waits_for_drained_receipt() {
    let mut voice = voice();
    voice.submitted("interaction-1");
    voice.on_assistant_delta("第一句。");
    let now = Instant::now();
    let (speaks, stop) = voice.on_turn_completed(now, None);
    assert!(speaks.is_empty(), "没有缓冲残句时不应再补发");
    let stop = stop.expect("一轮结束必须停播");
    assert_eq!(stop.reason, "turn_end");
    assert_eq!(voice.phase(), TurnPhase::Speaking, "回执前停在说话相位");
    assert_eq!(voice.drain_deadline(), Some(now + DRAIN_TIMEOUT));

    let value: serde_json::Value =
        serde_json::from_str(&stop_audio_json(&stop)).expect("合法 JSON");
    assert_eq!(value["type"], "stopAudio");
    assert_eq!(value["epoch"], 1);
    assert_eq!(value["reason"], "turn_end");

    assert_eq!(voice.on_playback_drained(9), DrainedOutcome::Stale);
    assert_eq!(
        voice.on_playback_drained(1),
        DrainedOutcome::Accepted,
        "回执后迁回聆听"
    );
    assert_eq!(voice.phase(), TurnPhase::Listening);
    assert_eq!(voice.epoch(), 0);
    assert_eq!(voice.on_playback_drained(1), DrainedOutcome::Idle);
}

#[test]
fn drain_timeout_forces_cleanup_with_injectable_delay() {
    let mut voice = VoiceTurn::new(Duration::from_secs(10));
    voice.submitted("interaction-1");
    voice.on_assistant_delta("第一句。");
    let now = Instant::now();
    voice.on_turn_completed(now, None);

    assert!(
        !voice.on_drain_timeout(now + Duration::from_secs(9)),
        "9 s 时不得提前清理"
    );
    assert!(voice.on_drain_timeout(now + Duration::from_secs(11)));
    assert_eq!(voice.phase(), TurnPhase::Listening);
    assert_eq!(voice.epoch(), 0, "强制清理后不得留着旧轮次");
    assert_eq!(voice.drain_deadline(), None);

    let value: serde_json::Value = serde_json::from_str(&drain_timeout_json()).expect("合法 JSON");
    assert_eq!(value["code"], "internal");
    assert_eq!(value["stage"], "playback");
    assert_eq!(value["retryable"], true);
}

#[test]
fn client_cancel_stops_immediately_and_silences_later_deltas() {
    let mut voice = voice();
    voice.submitted("interaction-1");
    voice.on_assistant_delta("第一句。");
    let stop = voice.on_client_cancel(Instant::now()).expect("应停播");
    assert_eq!(stop.reason, "user_cancel");
    assert_eq!(stop.epoch, 1);
    assert!(
        voice.on_assistant_delta("不应朗读。").is_empty(),
        "停播后不得再下发朗读"
    );
    assert!(!voice.accepts_audio(1), "停播后不得再下行该轮音频");
    assert_eq!(voice.on_playback_drained(1), DrainedOutcome::Accepted);
}

#[test]
fn audio_is_only_accepted_for_the_live_epoch() {
    let mut voice = voice();
    voice.submitted("interaction-1");
    assert!(voice.accepts_audio(1));
    assert!(!voice.accepts_audio(0), "0 不是合法对话轮次");
    assert!(!voice.accepts_audio(2), "未分配的轮次不得放行");

    voice.on_turn_completed(Instant::now(), None);
    assert!(!voice.accepts_audio(1), "停播后该轮音频一律丢弃");

    voice.on_playback_drained(1);
    voice.submitted("interaction-2");
    assert!(voice.accepts_audio(2));
    assert!(!voice.accepts_audio(1), "旧 epoch 音频必须丢弃");
}

#[test]
fn response_is_spoken_when_no_delta_arrived() {
    let mut voice = voice();
    voice.submitted("interaction-1");
    let (speaks, stop) = voice.on_turn_completed(Instant::now(), Some("结果已经写进报告。"));
    assert_eq!(speaks.len(), 1, "非流式回复必须兜底朗读");
    assert_eq!(speaks[0].seq, 1);
    assert!(speaks[0].text.contains("结果"));
    assert!(stop.is_some());
}

#[test]
fn stop_is_idempotent_per_turn() {
    let mut voice = voice();
    voice.submitted("interaction-1");
    let now = Instant::now();
    assert!(voice.on_turn_completed(now, None).1.is_some());
    assert!(
        voice.on_turn_completed(now, None).1.is_none(),
        "同一轮不得重复停播"
    );
    assert!(voice.on_client_cancel(now).is_none());
}

#[test]
fn read_aloud_off_keeps_lifecycle_without_speech() {
    let mut voice = voice();
    voice.set_read_aloud(false);
    voice.submitted("interaction-1");
    assert!(voice.on_assistant_delta("第一句。").is_empty());
    assert_eq!(voice.phase(), TurnPhase::Thinking, "不朗读不得进入说话相位");

    let (speaks, stop) = voice.on_turn_completed(Instant::now(), Some("第一句。"));
    assert!(speaks.is_empty(), "关闭朗读后终态兜底也不得补读");
    assert!(stop.is_some(), "回合终态仍需停播闭环");
    assert_eq!(voice.on_playback_drained(1), DrainedOutcome::Accepted);
    assert_eq!(voice.phase(), TurnPhase::Listening);
}
