//! 模型目录：退出码 13、预热成功与缺失路径。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_host::supervisor::{HostClient, HostConfig};
use axon_voice_protocol::{
    Control, Frame, PROTOCOL_VERSION, Ready, Warmup, read_frame, write_frame,
};

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_axon-voice-host")
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("axon-voice-root-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("创建临时目录");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录");
        }
        std::fs::write(path, bytes).expect("写文件");
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn manifest(name: &str, sha: &str, bytes: u64) -> String {
    format!(
        r#"{{
            "family": "asr",
            "name": "{name}",
            "version": "1.0.0",
            "runtime": "fixture",
            "sampleRate": 16000,
            "languages": ["zh"],
            "license": "MIT",
            "rtfP50": 0.2,
            "rtfP95": 0.3,
            "ramMb": 64,
            "files": [{{"path": "model.bin", "sha256": "{sha}", "bytes": {bytes}}}]
        }}"#
    )
}

fn install_model(root: &TempRoot) {
    root.write("asr/demo/model.bin", b"data");
    root.write(
        "asr/demo/manifest.json",
        manifest("demo", &hex_digest(b"data"), 4).as_bytes(),
    );
}

fn exit_code_of(root: &Path) -> i32 {
    let status = Command::new(exe())
        .arg("--model-root")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("宿主应能启动");
    status.code().unwrap_or(-1)
}

#[test]
fn invalid_manifest_root_exits_13() {
    let root = TempRoot::new("bad-manifest");
    root.write("asr/demo/manifest.json", b"{not-json}");
    assert_eq!(exit_code_of(root.path()), 13);
}

#[test]
fn non_directory_root_exits_13() {
    let root = TempRoot::new("file-root");
    root.write("plain.txt", b"x");
    assert_eq!(exit_code_of(&root.path().join("plain.txt")), 13);
    assert_eq!(exit_code_of(&root.path().join("missing")), 13);
}

#[test]
fn valid_model_warmup_returns_warm_and_heartbeat() {
    let root = TempRoot::new("warm");
    install_model(&root);
    let mut config = HostConfig::new(exe(), BUILD_COMMIT);
    config.args.push("--model-root".to_owned());
    config.args.push(root.path().display().to_string());
    config.handshake_timeout = Duration::from_secs(5);
    let mut client = HostClient::spawn(&config).expect("握手应成功");

    client
        .send(&Control::Warmup(Warmup {
            family: "asr".to_owned(),
            model: "demo".to_owned(),
            manifest_sha256: hex_digest(b"data"),
        }))
        .expect("写 warmup");
    match client.recv_timeout(Duration::from_secs(2)).expect("响应") {
        Frame::Control(Control::Warm(warm)) => {
            assert_eq!(warm.model, "demo");
            assert!((warm.rtf_p50 - 0.2).abs() < f64::EPSILON);
        }
        other => panic!("期望 warm，得到 {other:?}"),
    }

    client
        .send(&Control::SessionStart(axon_voice_protocol::SessionStart {
            session_id: "s-1".to_owned(),
            mode: "full_duplex".to_owned(),
            lang: "zh".to_owned(),
            hot_words: vec![],
            sample_rate: 16_000,
        }))
        .expect("写 sessionStart");
    match client.recv_timeout(Duration::from_secs(3)).expect("心跳") {
        Frame::Control(Control::Heartbeat(beat)) => assert_eq!(beat.model_state, "warm"),
        other => panic!("期望心跳，得到 {other:?}"),
    }
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn missing_model_reports_model_missing() {
    let root = TempRoot::new("missing");
    let mut config = HostConfig::new(exe(), BUILD_COMMIT);
    config.args.push("--model-root".to_owned());
    config.args.push(root.path().display().to_string());
    let mut client = HostClient::spawn(&config).expect("握手应成功");
    client
        .send(&Control::Warmup(Warmup {
            family: "asr".to_owned(),
            model: "demo".to_owned(),
            manifest_sha256: "0".repeat(64),
        }))
        .expect("写 warmup");
    match client.recv_timeout(Duration::from_secs(2)).expect("响应") {
        Frame::Control(Control::Error(error)) => assert_eq!(error.code, "model_missing"),
        other => panic!("期望 model_missing，得到 {other:?}"),
    }
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn digest_mismatch_reports_model_unusable() {
    let root = TempRoot::new("digest");
    root.write("asr/demo/model.bin", b"data");
    root.write(
        "asr/demo/manifest.json",
        manifest("demo", &"0".repeat(64), 4).as_bytes(),
    );
    let mut config = HostConfig::new(exe(), BUILD_COMMIT);
    config.args.push("--model-root".to_owned());
    config.args.push(root.path().display().to_string());
    let mut client = HostClient::spawn(&config).expect("握手应成功");
    client
        .send(&Control::Warmup(Warmup {
            family: "asr".to_owned(),
            model: "demo".to_owned(),
            manifest_sha256: "0".repeat(64),
        }))
        .expect("写 warmup");
    match client.recv_timeout(Duration::from_secs(2)).expect("响应") {
        Frame::Control(Control::Error(error)) => {
            assert_eq!(error.code, "model_unusable");
            assert!(!error.retryable);
        }
        other => panic!("期望 model_unusable，得到 {other:?}"),
    }
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn handshake_still_required_with_model_root() {
    let root = TempRoot::new("handshake");
    install_model(&root);
    let mut child = Command::new(exe())
        .arg("--model-root")
        .arg(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    write_frame(
        &mut stdin,
        &Control::Hello(axon_voice_protocol::Hello {
            protocol: PROTOCOL_VERSION,
            build_commit: BUILD_COMMIT.to_owned(),
            role: "daemon".to_owned(),
            pid: std::process::id(),
        }),
    )
    .expect("写 hello");
    assert!(matches!(
        read_frame(&mut stdout).expect("读取").expect("帧"),
        Frame::Control(Control::Ready(Ready { .. }))
    ));
    drop(stdin);
    let status = child.wait().expect("等待");
    assert_eq!(status.code(), Some(0));
}
