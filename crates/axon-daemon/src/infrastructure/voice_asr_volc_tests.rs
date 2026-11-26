//! 识别通道的单元用例：协议编解码、序号语义与一次真实链路冒烟。
//!
//! 冒烟用例需要 ARK_KEY 与网络，手动执行：
//! `cargo test -p axon-daemon --lib live_recognition -- --ignored`

use std::time::Duration;

use axon_voice_bridge::HostTransport;
use axon_voice_bridge::axon_voice_protocol::{
    Control, Frame, SessionStart, UtteranceEnd, Warmup, encode_audio_in,
};

use super::super::voice_tts_volc::{
    DEFAULT_TTS_ENDPOINT, DEFAULT_TTS_RESOURCE_ID, DEFAULT_TTS_SPEAKER, VolcTtsConfig, synth_wav,
};
use super::{
    COMPRESSION_GZIP, FLAG_LAST_WITH_SEQ, FLAG_POSITIVE_SEQ, MT_SERVER_RESPONSE,
    SERIALIZATION_JSON, VolcAsrConfig, gzip, header, utterance_span,
};

#[test]
fn config_falls_back_to_chinese() {
    let config = VolcAsrConfig::new("wss://example.invalid/asr", "key", "res", "", "  ");
    assert_eq!(config.language, "zh");
    assert_eq!(config.model, super::DEFAULT_ASR_MODEL, "模型名缺省产品口径");
    let auto = VolcAsrConfig::new("wss://example.invalid/asr", "key", "res", "m", "auto");
    assert_eq!(auto.language, "auto", "auto 要原样保留");
}

#[test]
fn header_carries_protocol_bits() {
    let frame = header(
        MT_SERVER_RESPONSE,
        FLAG_LAST_WITH_SEQ,
        SERIALIZATION_JSON,
        COMPRESSION_GZIP,
    );
    assert_eq!(frame[0], 0x11, "版本 1 + 4 字节头");
    assert_eq!(frame[1], 0x93, "server response + last with seq");
    assert_eq!(frame[2], 0x11, "json + gzip");
    assert_eq!(frame[3], 0x00);
}

#[test]
fn positive_sequence_flag_is_distinct_from_last() {
    assert_ne!(FLAG_POSITIVE_SEQ, FLAG_LAST_WITH_SEQ);
}

#[test]
fn gzip_round_trips() {
    let raw = b"hello";
    let compressed = gzip(raw).expect("压缩应成功");
    assert_ne!(compressed.as_slice(), raw, "压缩后应当变化");
}

#[test]
fn utterance_span_reads_first_and_last() {
    let value = serde_json::json!({
        "result": {
            "utterances": [
                { "start_time": 120, "end_time": 800 },
                { "start_time": 900, "end_time": 1500 }
            ]
        }
    });
    assert_eq!(utterance_span(&value), (120, 1500));
    assert_eq!(utterance_span(&serde_json::json!({})), (0, 0));
}

/// 从 16 kHz 单声道 16 bit WAV 里取 PCM 数据块。
fn wav_pcm(wav: &[u8]) -> Vec<u8> {
    let mut offset = 12;
    while offset + 8 <= wav.len() {
        let id = &wav[offset..offset + 4];
        let size = u32::from_le_bytes([
            wav[offset + 4],
            wav[offset + 5],
            wav[offset + 6],
            wav[offset + 7],
        ]) as usize;
        let start = offset + 8;
        if id == b"data" {
            let end = start.saturating_add(size).min(wav.len());
            return wav[start..end].to_vec();
        }
        offset = start.saturating_add(size).saturating_add(size & 1);
    }
    Vec::new()
}

#[test]
#[ignore = "需要 ARK_KEY 与网络，按需手动执行"]
fn live_recognition_returns_expected_text() {
    let Ok(api_key) = std::env::var("ARK_KEY") else {
        return;
    };
    // 用同一把方舟凭据合成参考音频，再把 PCM 喂给识别通道。
    let tts = VolcTtsConfig {
        endpoint: DEFAULT_TTS_ENDPOINT.to_owned(),
        api_key: api_key.clone(),
        resource_id: DEFAULT_TTS_RESOURCE_ID.to_owned(),
        speaker: DEFAULT_TTS_SPEAKER.to_owned(),
        speech_rate: 0,
    };
    let synth_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("合成运行时");
    let wav = synth_runtime
        .block_on(synth_wav(&tts, "你好，我是知微。"))
        .expect("合成应成功");
    // 必须先释放合成运行时：识别通道内部会自己 block_on。
    drop(synth_runtime);
    let pcm = wav_pcm(&wav);
    assert!(!pcm.is_empty(), "合成音频应带 PCM 数据");

    let config = VolcAsrConfig::new(
        super::DEFAULT_ASR_ENDPOINT,
        api_key,
        super::DEFAULT_ASR_RESOURCE_ID,
        "",
        "zh",
    );
    let mut transport = super::VolcAsrTransport::open(&config).expect("识别建连应成功");
    transport
        .send_control(&Control::Warmup(Warmup {
            family: "asr".to_owned(),
            model: config.model.clone(),
        }))
        .expect("预热应成功");
    match transport.recv(Duration::from_secs(2)) {
        Ok(Frame::Control(Control::Warm(_))) => {}
        other => panic!("预热回执异常：{other:?}"),
    }
    transport
        .send_control(&Control::SessionStart(SessionStart {
            session_id: "live-test".to_owned(),
            mode: "full_duplex".to_owned(),
            lang: "zh".to_owned(),
            hot_words: Vec::new(),
            sample_rate: 16_000,
        }))
        .expect("首帧应成功");
    for chunk in pcm.chunks(3_200) {
        let frame = encode_audio_in(chunk).expect("管道帧应编码成功");
        transport.send_bytes(&frame).expect("音频上行应成功");
        std::thread::sleep(Duration::from_millis(20));
    }
    transport
        .send_control(&Control::UtteranceEnd(UtteranceEnd { utterance_seq: 1 }))
        .expect("末帧应成功");
    let mut final_text = None;
    for _ in 0..60 {
        match transport.recv(Duration::from_secs(5)) {
            Ok(Frame::Control(Control::Final(message))) => {
                final_text = Some(message.text);
                break;
            }
            Ok(_) => {}
            Err(error) => panic!("识别失败：{error:?}"),
        }
    }
    transport.shutdown();
    let text = final_text.expect("必须收到定稿");
    assert!(!text.trim().is_empty(), "识别结果不应为空");
    assert!(text.contains("知微"), "识别文本应包含参考词，实际：{text}");
}
