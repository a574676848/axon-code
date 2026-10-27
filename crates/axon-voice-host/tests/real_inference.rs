//! 真机推理测试：需要本地 sherpa-onnx 运行时与模型，默认忽略。
//!
//! 运行方式（PowerShell）：
//!   $env:AXON_VOICE_MODEL_ROOT = Join-Path $env:AXON_RUNTIME_ROOT 'voice\models'
//!   cargo test -p axon-voice-host --test real_inference -- --ignored --nocapture

use std::io::BufReader;
use std::process::{Command, Stdio};

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::audio;
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_protocol::{
    Control, Final, Frame, Hello, PROTOCOL_VERSION, SessionEnd, SessionStart, Speak, Warmup,
    encode_audio_in, read_frame, write_bytes, write_frame,
};

fn model_root() -> String {
    std::env::var("AXON_VOICE_MODEL_ROOT").expect("需要 AXON_VOICE_MODEL_ROOT")
}

#[test]
#[ignore = "需要本地 sherpa-onnx 运行时与模型"]
fn real_transcription_and_tts() {
    let root = model_root();
    let manifest = format!("{root}/asr/sherpa-streaming-zh-en/manifest.json");
    let manifest_sha = hex_digest(&std::fs::read(&manifest).expect("读取清单"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_axon-voice-host"))
        .arg("--model-root")
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("启动宿主");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    write_frame(
        &mut stdin,
        &Control::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            build_commit: BUILD_COMMIT.to_owned(),
            role: "daemon".to_owned(),
            pid: std::process::id(),
        }),
    )
    .expect("写 hello");
    assert!(matches!(
        read_frame(&mut stdout).expect("读帧").expect("帧"),
        Frame::Control(Control::Ready(_))
    ));

    write_frame(
        &mut stdin,
        &Control::Warmup(Warmup {
            family: "asr".to_owned(),
            model: "sherpa-streaming-zh-en".to_owned(),
            manifest_sha256: manifest_sha,
        }),
    )
    .expect("写 warmup");
    match next_non_heartbeat(&mut stdout) {
        Frame::Control(Control::Warm(warm)) => {
            println!("warm: rtf_p50={:.3} rss={}KB", warm.rtf_p50, warm.rss_kb);
            assert!(warm.rtf_p50 > 0.0);
        }
        other => panic!("期望 warm，得到 {other:?}"),
    }

    write_frame(
        &mut stdin,
        &Control::SessionStart(SessionStart {
            session_id: "real-1".to_owned(),
            mode: "full_duplex".to_owned(),
            lang: "zh".to_owned(),
            hot_words: Vec::new(),
            sample_rate: 16_000,
        }),
    )
    .expect("写 sessionStart");

    let wav = std::fs::read("E:/zbg/axon-runtime/voice/samples/hello.wav")
        .or_else(|_| std::fs::read("../axon-runtime/voice/samples/hello.wav"))
        .expect("需要 hello.wav 样例");
    let clip = audio::parse(&wav).expect("解析样例 WAV");
    for chunk in clip.samples.chunks(3_200) {
        let mut bytes = Vec::with_capacity(chunk.len() * 2);
        for sample in chunk {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let frame = encode_audio_in(&bytes).expect("编码音频");
        write_bytes(&mut stdin, &frame).expect("写音频");
    }

    write_frame(
        &mut stdin,
        &Control::SessionEnd(SessionEnd {
            session_id: "real-1".to_owned(),
            reason: "client_close".to_owned(),
        }),
    )
    .expect("写 sessionEnd");
    match next_non_heartbeat(&mut stdout) {
        Frame::Control(Control::Final(Final { text, .. })) => {
            println!("transcript: {text}");
            assert!(!text.is_empty(), "转写不应为空");
            assert!(
                text.contains("天气") || text.contains("你好"),
                "转写应包含样例内容：{text}"
            );
        }
        other => panic!("期望 final，得到 {other:?}"),
    }

    // 合成：在会话内发 speak，应收到至少一帧对话音频。
    write_frame(
        &mut stdin,
        &Control::SessionStart(SessionStart {
            session_id: "real-2".to_owned(),
            mode: "full_duplex".to_owned(),
            lang: "zh".to_owned(),
            hot_words: Vec::new(),
            sample_rate: 16_000,
        }),
    )
    .expect("写第二个 sessionStart");
    write_frame(
        &mut stdin,
        &Control::Speak(Speak {
            turn_id: "turn-1".to_owned(),
            epoch: 1,
            seq: 1,
            text: "你好，世界".to_owned(),
        }),
    )
    .expect("写 speak");
    match next_non_heartbeat(&mut stdout) {
        Frame::AudioOut { epoch, audio, .. } => {
            println!("tts audio: epoch={epoch} bytes={}", audio.len());
            assert!(audio.len() > 1_000);
        }
        other => panic!("期望音频，得到 {other:?}"),
    }

    write_frame(&mut stdin, &Control::Close).expect("写 close");
    let _ = read_frame(&mut stdout);
    drop(stdin);
    let status = child.wait().expect("等待退出");
    assert!(status.success(), "宿主应正常退出");
}

fn next_non_heartbeat(reader: &mut BufReader<std::process::ChildStdout>) -> Frame {
    loop {
        match read_frame(reader).expect("读帧").expect("帧") {
            Frame::Control(Control::Heartbeat(_)) => continue,
            frame => return frame,
        }
    }
}
