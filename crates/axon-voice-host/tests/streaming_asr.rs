//! 流式识别真机测试：需要本地 sherpa-onnx 运行时与模型。
//!
//! 运行方式（在 worktree 根目录）：
//! ```text
//! AXON_VOICE_MODEL_ROOT=<运行态根>/voice/models \
//! cargo test -p axon-voice-host --test streaming_asr -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axon_voice_host::inference::Inference;
use axon_voice_host::streaming::{AsrEvent, StreamingAsr};

/// 从样例 WAV 取出 16 位 PCM。
fn sample_pcm() -> Vec<u8> {
    let path = std::env::var("AXON_VOICE_SAMPLE_WAV").unwrap_or_else(|_| {
        let root = std::env::var("AXON_VOICE_MODEL_ROOT").expect("需要 AXON_VOICE_MODEL_ROOT");
        let base = PathBuf::from(root);
        base.parent()
            .expect("模型根应有上级目录")
            .join("samples")
            .join("hello.wav")
            .display()
            .to_string()
    });
    let bytes = std::fs::read(&path).expect("读取样例 WAV");
    assert!(bytes.len() > 44, "样例 WAV 过短");
    bytes[44..].to_vec()
}

fn layout() -> Inference {
    let root = std::env::var("AXON_VOICE_MODEL_ROOT").expect("需要 AXON_VOICE_MODEL_ROOT");
    Inference::discover(&PathBuf::from(root)).expect("运行时与模型应可用")
}

#[test]
#[ignore = "需要本地运行时与模型"]
fn streaming_emits_partials_then_final() {
    let mut asr = StreamingAsr::start(&layout()).expect("启动流式识别服务");
    let pcm = sample_pcm();
    let chunk = 3_200 * 2; // 100 ms 的字节数

    let started = Instant::now();
    let mut partials: Vec<String> = Vec::new();
    let mut finals: Vec<(u32, String, u64, u64, u64)> = Vec::new();
    // 按真实节奏喂入，中间结果才有意义。
    for piece in pcm.chunks(chunk) {
        asr.push_audio(piece).expect("推入音频");
        for event in asr.poll(Duration::from_millis(20)).expect("轮询事件") {
            match event {
                AsrEvent::Partial { text, .. } => partials.push(text),
                AsrEvent::Final {
                    utterance_seq,
                    text,
                    start_ms,
                    end_ms,
                    finalize_ms,
                } => finals.push((utterance_seq, text, start_ms, end_ms, finalize_ms)),
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for event in asr.finish_utterance().expect("收取定稿") {
        if let AsrEvent::Final {
            utterance_seq,
            text,
            start_ms,
            end_ms,
            finalize_ms,
        } = event
        {
            finals.push((utterance_seq, text, start_ms, end_ms, finalize_ms));
        }
    }
    let elapsed = started.elapsed();
    asr.shutdown();

    println!("partials: {partials:?}");
    println!("finals: {finals:?}");
    println!("elapsed: {elapsed:?}");
    assert!(
        partials.iter().any(|text| !text.is_empty()),
        "必须产生真实中间结果"
    );
    assert!(!finals.is_empty(), "必须产生定稿");
    for (_, text, start_ms, end_ms, finalize_ms) in &finals {
        assert!(!text.is_empty(), "定稿文本不应为空");
        assert!(
            start_ms <= end_ms && end_ms <= finalize_ms,
            "时间戳必须单调：{start_ms} ≤ {end_ms} ≤ {finalize_ms}"
        );
    }
    assert!(
        finals
            .iter()
            .any(|(_, text, ..)| text.contains("天") || text.contains("好")),
        "定稿内容应包含样例语句片段：{finals:?}"
    );
}

#[test]
#[ignore = "需要本地运行时与模型"]
fn warm_rtf_is_within_contract() {
    let mut asr = StreamingAsr::start(&layout()).expect("启动流式识别服务");
    let rtf = asr.measure_rtf().expect("实测 RTF");
    asr.shutdown();
    println!("warm rtf p50: {rtf}");
    assert!(rtf > 0.0, "RTF 必须为正数");
    assert!(rtf <= 0.6, "P-06 要求 warm RTF ≤ 0.6，实测 {rtf}");
}

#[test]
#[ignore = "需要本地运行时与模型"]
fn cold_start_is_within_contract() {
    let started = Instant::now();
    let mut asr = StreamingAsr::start(&layout()).expect("启动流式识别服务");
    let cold = started.elapsed();
    asr.shutdown();
    println!("cold start: {cold:?}");
    assert!(
        cold <= Duration::from_secs(10),
        "start → warm 要求 ≤ 10 s，实测 {cold:?}"
    );
}

/// 预热总预算：模型校验 + 识别服务启动必须一起落在 10 s 内。
#[test]
#[ignore = "需要本地运行时与模型"]
fn warmup_budget_covers_model_verification() {
    use axon_voice_host::model::sha256::hex_digest;
    use axon_voice_host::model::{ModelFamily, ModelResolution, ModelStore};

    let root = std::env::var("AXON_VOICE_MODEL_ROOT").expect("需要 AXON_VOICE_MODEL_ROOT");
    let store = ModelStore::new(&root);
    let manifest = std::fs::read(
        PathBuf::from(&root)
            .join("asr")
            .join("sherpa-streaming-zh-en")
            .join("manifest.json"),
    )
    .expect("读取清单");
    let digest = hex_digest(&manifest);

    let started = Instant::now();
    let resolution = store.resolve_verified(ModelFamily::Asr, "sherpa-streaming-zh-en", &digest);
    let verify = started.elapsed();
    assert!(
        matches!(resolution, ModelResolution::Ready { .. }),
        "模型校验应通过：{resolution:?}"
    );

    let started = Instant::now();
    let mut asr = StreamingAsr::start(&layout()).expect("启动流式识别服务");
    let service = started.elapsed();
    asr.shutdown();

    println!("verify: {verify:?} service: {service:?}");
    assert!(
        verify + service <= Duration::from_secs(10),
        "start → warm 要求 ≤ 10 s，实测校验 {verify:?} + 启动 {service:?}"
    );
}
