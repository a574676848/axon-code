//! 宿主工作线程的单测：有界队列、背压窗口与事件映射。

use super::{AUDIO_QUEUE_CAPACITY, Command, CommandQueue, PushOutcome};
use crate::infrastructure::gateway_http::gateway_voice_ready::ready_json;

fn audio(tag: u8) -> Vec<u8> {
    vec![tag; 4]
}

#[test]
fn queue_drops_oldest_audio_and_keeps_controls() {
    let queue = CommandQueue::new(4);
    for tag in 0..4 {
        assert_eq!(queue.push_audio(audio(tag)), PushOutcome::Queued);
    }
    queue.push_control(Command::Stop(1));
    // 队列已满（4 条音频 + 1 条控制），第 5 帧音频必须丢掉最旧的一帧。
    assert_eq!(
        queue.push_audio(audio(9)),
        PushOutcome::Dropped {
            frames: 1,
            bytes: 4
        }
    );
    let drained = queue.drain();
    let audios: Vec<u8> = drained
        .iter()
        .filter_map(|command| match command {
            Command::Audio(bytes) => bytes.first().copied(),
            _ => None,
        })
        .collect();
    assert_eq!(audios, vec![1, 2, 3, 9], "应丢第 0 帧，保留最新帧");
    assert!(
        drained
            .iter()
            .any(|command| matches!(command, Command::Stop(1))),
        "控制命令不得丢"
    );
    assert_eq!(queue.totals(), (1, 4));
}

#[test]
fn queue_capacity_is_the_documented_value() {
    assert_eq!(AUDIO_QUEUE_CAPACITY, 8);
}

#[test]
fn overload_window_clears_after_recovery_gap() {
    let queue = CommandQueue::new(1);
    queue.push_audio(audio(0));
    let dropped = queue.push_audio(audio(1));
    assert!(matches!(dropped, PushOutcome::Dropped { .. }));
    assert!(queue.overload_for().is_some(), "丢帧后应开始计时");
    // 排空不等于恢复：持续超限时每轮都会排空。
    let _ = queue.drain();
    assert!(
        queue.overload_for().is_some(),
        "刚排空仍然算超限，否则 10 s 断连不可达"
    );
    // 一个恢复间隔内没有新丢帧才算恢复。
    std::thread::sleep(super::OVERLOAD_RECOVERY_GAP + std::time::Duration::from_millis(50));
    assert!(queue.overload_for().is_none(), "恢复间隔后可清空");
}

#[test]
fn ready_has_contract_fields() {
    let value: serde_json::Value = serde_json::from_str(&ready_json()).expect("合法 JSON");
    for field in [
        "asrModel",
        "ttsModel",
        "sampleRate",
        "workspaceId",
        "replyDestination",
        "sessionId",
        "epoch",
        "phase",
        "tts",
        "degraded",
        "cues",
    ] {
        assert!(value.get(field).is_some(), "ready 缺字段 {field}");
    }
    assert_eq!(value["sampleRate"], 16_000);
}
