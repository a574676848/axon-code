use base64::Engine as _;

use super::super::gateway_voice_realtime_provider::RealtimeEvent;
use super::{
    Resampler24kTo16k, SessionConfig, TurnDetection, audio_append_json, merge_transcript,
    parse_server_event, playback_chunks,
};

fn s16(values: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 2);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decoded(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes(*chunk))
        .collect()
}

#[test]
fn session_update_carries_the_probe_verified_fields() {
    let config = SessionConfig {
        voice: "longanlingxin_v3.1".to_owned(),
        instructions: "你是知微，回答简短口语化。".to_owned(),
        ..SessionConfig::default()
    };
    let value: serde_json::Value =
        serde_json::from_str(&config.session_update_json()).expect("合法 JSON");
    assert_eq!(value["type"], "session.update");
    let session = &value["session"];
    assert_eq!(session["modalities"], serde_json::json!(["text", "audio"]));
    assert_eq!(session["voice"], "longanlingxin_v3.1");
    assert_eq!(session["instructions"], "你是知微，回答简短口语化。");
    assert_eq!(
        session["input_audio_transcription"]["language"], "zh",
        "转写语言必须随配置下发"
    );
    assert_eq!(session["turn_detection"]["type"], "server_vad");
    assert_eq!(session["turn_detection"]["threshold"], 0.5);
    assert_eq!(session["turn_detection"]["silence_duration_ms"], 800);
    assert!(
        session.get("speech_rate").is_none(),
        "未配置语速时不下发，避免与服务端默认值较劲"
    );
    assert!(
        session.get("max_history_turns").is_none(),
        "未配置历史轮数时不下发"
    );
}

#[test]
fn manual_turn_detection_serializes_as_null() {
    let config = SessionConfig {
        turn_detection: TurnDetection::Manual,
        ..SessionConfig::default()
    };
    let value: serde_json::Value =
        serde_json::from_str(&config.session_update_json()).expect("合法 JSON");
    assert!(
        value["session"]["turn_detection"].is_null(),
        "手动提交必须显式下发 null"
    );
}

#[test]
fn optional_fields_are_sent_only_when_configured() {
    let config = SessionConfig {
        speech_rate: Some(1.2),
        max_history_turns: Some(10),
        ..SessionConfig::default()
    };
    let value: serde_json::Value =
        serde_json::from_str(&config.session_update_json()).expect("合法 JSON");
    assert_eq!(value["session"]["speech_rate"], serde_json::json!(1.2f32));
    assert_eq!(value["session"]["max_history_turns"], 10);
}

#[test]
fn audio_append_is_base64_of_raw_pcm() {
    let value: serde_json::Value =
        serde_json::from_str(&audio_append_json(&[0x01, 0x02, 0xff])).expect("合法 JSON");
    assert_eq!(value["type"], "input_audio_buffer.append");
    assert_eq!(value["audio"], "AQL/");
}

/// 手拼的 JSON 必须与序列化器产物**语义一致**：省掉中间对象分配不能改变线上形状。
///
/// 比较解析后的值而不是字节：JSON 对象字段无序，`json!` 宏走 BTreeMap 会按字母序输出，
/// 逐字节比较只会把「字段顺序」这种无关差异变成假失败。
#[test]
fn hand_built_audio_append_matches_serde_output() {
    for pcm in [&[][..], &[0x00][..], &[0x01, 0x02, 0xff][..], &[0x7f; 640][..]] {
        let actual: serde_json::Value =
            serde_json::from_str(&audio_append_json(pcm)).expect("合法 JSON");
        let expected = serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": base64::engine::general_purpose::STANDARD.encode(pcm),
        });
        assert_eq!(actual, expected, "PCM 长度 {}", pcm.len());
    }
}

#[test]
fn transcript_delta_keeps_both_accumulated_halves() {
    let event = parse_server_event(
        r#"{"type":"conversation.item.input_audio_transcription.delta","text":"今天","stash":"天气"}"#,
    );
    assert_eq!(
        event,
        RealtimeEvent::InputTranscriptDelta {
            text: "今天".to_owned(),
            stash: "天气".to_owned(),
        }
    );
    assert_eq!(merge_transcript("今天", "天气"), "今天天气");
    assert_eq!(merge_transcript("今天天气", ""), "今天天气");
}

#[test]
fn transcript_completed_is_the_authoritative_text() {
    let event = parse_server_event(
        r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"今天天气怎么样"}"#,
    );
    assert_eq!(
        event,
        RealtimeEvent::InputTranscriptCompleted {
            transcript: "今天天气怎么样".to_owned(),
        }
    );
}

#[test]
fn audio_delta_is_decoded_from_base64() {
    let event = parse_server_event(r#"{"type":"response.audio.delta","delta":"AQL/"}"#);
    assert_eq!(
        event,
        RealtimeEvent::AudioDelta {
            pcm: vec![0x01, 0x02, 0xff]
        }
    );
}

#[test]
fn broken_audio_delta_does_not_break_the_session() {
    assert_eq!(
        parse_server_event(r#"{"type":"response.audio.delta","delta":"@@@"}"#),
        RealtimeEvent::Ignored
    );
}

#[test]
fn lifecycle_and_speech_events_are_recognized() {
    assert_eq!(
        parse_server_event(r#"{"type":"session.created","session":{"id":"s"}}"#),
        RealtimeEvent::SessionCreated
    );
    assert_eq!(
        parse_server_event(r#"{"type":"session.updated","session":{"id":"s"}}"#),
        RealtimeEvent::SessionUpdated
    );
    assert_eq!(
        parse_server_event(r#"{"type":"input_audio_buffer.speech_started"}"#),
        RealtimeEvent::SpeechStarted
    );
    assert_eq!(
        parse_server_event(r#"{"type":"input_audio_buffer.speech_stopped","reason":"vad"}"#),
        RealtimeEvent::SpeechStopped
    );
    assert_eq!(
        parse_server_event(r#"{"type":"response.created","response":{"id":"r"}}"#),
        RealtimeEvent::ResponseCreated
    );
    assert_eq!(
        parse_server_event(r#"{"type":"response.done","response":{"status":"completed"}}"#),
        RealtimeEvent::ResponseDone
    );
    assert_eq!(
        parse_server_event(r#"{"type":"response.audio_transcript.delta","delta":"你好"}"#),
        RealtimeEvent::OutputTranscriptDelta {
            text: "你好".to_owned()
        }
    );
    assert_eq!(
        parse_server_event(r#"{"type":"response.text.delta","delta":"你好"}"#),
        RealtimeEvent::OutputTranscriptDelta {
            text: "你好".to_owned()
        }
    );
    assert_eq!(
        parse_server_event(
            r#"{"type":"response.audio_transcript.done","transcript":"你好，先生。"}"#
        ),
        RealtimeEvent::OutputTranscriptDone {
            transcript: "你好，先生。".to_owned()
        }
    );
}

#[test]
fn unknown_and_malformed_frames_are_ignored() {
    assert_eq!(parse_server_event("not json"), RealtimeEvent::Ignored);
    assert_eq!(parse_server_event(r#"{"no":"type"}"#), RealtimeEvent::Ignored);
    assert_eq!(
        parse_server_event(r#"{"type":"response.audio_transcript.delta"}"#),
        RealtimeEvent::OutputTranscriptDelta {
            text: String::new()
        }
    );
    assert_eq!(
        parse_server_event(r#"{"type":"conversation.item.created"}"#),
        RealtimeEvent::Ignored
    );
}

#[test]
fn error_code_prefers_the_nested_error_object() {
    assert_eq!(
        parse_server_event(
            r#"{"type":"error","error":{"code":"invalid_parameter","message":"voice 不支持"}}"#
        ),
        RealtimeEvent::Error {
            code: "invalid_parameter".to_owned(),
            message: "voice 不支持".to_owned(),
        }
    );
    assert_eq!(
        parse_server_event(r#"{"type":"error","code":"rate_limited","message":"太快"}"#),
        RealtimeEvent::Error {
            code: "rate_limited".to_owned(),
            message: "太快".to_owned(),
        }
    );
    assert_eq!(
        parse_server_event(r#"{"type":"error"}"#),
        RealtimeEvent::Error {
            code: "provider_error".to_owned(),
            message: "语音提供方返回错误".to_owned(),
        }
    );
}

#[test]
fn resampler_keeps_the_3_to_2_ratio() {
    let mut resampler = Resampler24kTo16k::new();
    let out = resampler.push(&s16(&[0, 30, 60, 3, 6, 9]));
    assert_eq!(decoded(&out), vec![0, 45, 3, 7]);
    assert!(decoded(&resampler.push(&[])).is_empty(), "空包不产出音频");
}

#[test]
fn resampler_carries_partial_groups_across_chunks() {
    let mut resampler = Resampler24kTo16k::new();
    // 两个采样凑不满一组，必须留到下一包。
    assert!(resampler.push(&s16(&[0, 30])).is_empty());
    let out = resampler.push(&s16(&[60, 3, 6, 9]));
    assert_eq!(decoded(&out), vec![0, 45, 3, 7]);
}

#[test]
fn resampler_carries_the_odd_byte() {
    let mut resampler = Resampler24kTo16k::new();
    let mut first = s16(&[0, 30]);
    first.push(0x40);
    assert!(resampler.push(&first).is_empty());
    // 首字节与上一包落单的 0x40 拼成 0x0040 = 64。
    let out = resampler.push(&[0x00, 60, 0x00, 3, 0x00, 6, 0x00]);
    assert_eq!(decoded(&out), vec![0, 47, 60, 4]);
}

#[test]
fn playback_chunks_split_at_twenty_milliseconds() {
    let pcm = vec![0u8; 1_600];
    let sizes: Vec<usize> = playback_chunks(&pcm).map(<[u8]>::len).collect();
    assert_eq!(sizes, vec![640, 640, 320]);
}
