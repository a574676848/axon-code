//! 宿主工作线程的单测：有界队列、背压窗口与事件映射。

use super::{AUDIO_QUEUE_CAPACITY, Command, CommandQueue, PushOutcome, VoiceOut, event_out};
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

#[test]
fn speak_and_cancel_are_queued_as_controls() {
    let queue = CommandQueue::new(1);
    assert_eq!(queue.push_audio(audio(0)), PushOutcome::Queued);
    queue.push_control(Command::Speak {
        turn_id: "interaction-1:1".to_owned(),
        epoch: 1,
        seq: 1,
        text: "第一句。".to_owned(),
    });
    queue.push_control(Command::Cancel("user_cancel".to_owned()));
    // 音频容量只约束音频：新音频挤掉最旧音频，控制命令必须留到收尾。
    assert!(matches!(
        queue.push_audio(audio(9)),
        PushOutcome::Dropped { frames: 1, .. }
    ));
    let drained = queue.drain();
    assert!(drained.iter().any(|command| matches!(
        command,
        Command::Speak { turn_id, epoch, seq, text }
            if turn_id == "interaction-1:1" && *epoch == 1 && *seq == 1 && text == "第一句。"
    )));
    assert!(
        drained
            .iter()
            .any(|command| matches!(command, Command::Cancel(reason) if reason == "user_cancel")),
        "取消命令不得被音频挤掉"
    );
}

#[test]
fn audio_out_keeps_host_epoch_and_sequence() {
    let event = axon_voice_bridge::VoiceEvent::Audio {
        epoch: 3,
        seq: 7,
        audio: vec![1, 2, 3, 4],
    };
    match event_out(event) {
        Some(VoiceOut::Audio { epoch, seq, pcm }) => {
            assert_eq!(epoch, 3, "epoch 必须原样透传，不重编号");
            assert_eq!(seq, 7, "pcmSeq 必须原样透传，不重编号");
            assert_eq!(pcm, vec![1, 2, 3, 4]);
        }
        other => panic!("期望音频输出，得到 {other:?}"),
    }
}
