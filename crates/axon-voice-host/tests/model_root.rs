//! 模型目录：退出码 13、清单绑定、缺失与不可用分类。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_host::supervisor::{HostClient, HostConfig};
use axon_voice_protocol::{Control, Frame, Warmup};

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

fn manifest(name: &str, sha: &str, bytes: u64, version: &str) -> String {
    format!(
        r#"{{
            "family": "asr",
            "name": "{name}",
            "version": "{version}",
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

fn install_model(root: &TempRoot) -> String {
    root.write("asr/demo/model.bin", b"data");
    let json = manifest("demo", &hex_digest(b"data"), 4, "1.0.0");
    root.write("asr/demo/manifest.json", json.as_bytes());
    hex_digest(json.as_bytes())
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

fn client_for(root: &Path) -> HostClient {
    let mut config = HostConfig::new(exe(), BUILD_COMMIT);
    config.args.push("--model-root".to_owned());
    config.args.push(root.display().to_string());
    config.handshake_timeout = Duration::from_secs(5);
    HostClient::spawn(&config).expect("握手应成功")
}

fn warmup(manifest_sha256: &str) -> Control {
    Control::Warmup(Warmup {
        family: "asr".to_owned(),
        model: "demo".to_owned(),
        manifest_sha256: manifest_sha256.to_owned(),
    })
}

fn expect_error(client: &mut HostClient, code: &str) -> (bool, Option<String>) {
    match client.recv_timeout(Duration::from_secs(2)).expect("响应") {
        Frame::Control(Control::Error(error)) => {
            assert_eq!(error.code, code);
            (
                error.retryable,
                error.detail.and_then(|detail| detail.reason),
            )
        }
        other => panic!("期望 {code}，得到 {other:?}"),
    }
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
fn verified_model_reports_runtime_unavailable() {
    let root = TempRoot::new("verified");
    let manifest_sha = install_model(&root);
    let mut client = client_for(root.path());
    client.send(&warmup(&manifest_sha)).expect("写 warmup");
    let (retryable, reason) = expect_error(&mut client, "model_unusable");
    assert!(!retryable);
    let reason = reason.expect("应带原因");
    assert!(
        reason.contains("运行时"),
        "原因应说明运行时未接入：{reason}"
    );

    // 心跳必须如实报 cold，而不是伪造 warm。
    match client.recv_timeout(Duration::from_secs(3)).expect("心跳") {
        Frame::Control(Control::Heartbeat(beat)) => assert_eq!(beat.model_state, "cold"),
        other => panic!("期望心跳，得到 {other:?}"),
    }
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn manifest_digest_mismatch_is_unusable() {
    let root = TempRoot::new("manifest-sha");
    install_model(&root);
    let mut client = client_for(root.path());
    client.send(&warmup(&"0".repeat(64))).expect("写 warmup");
    let (_, reason) = expect_error(&mut client, "model_unusable");
    assert_eq!(reason.as_deref(), Some("清单摘要与请求不一致"));
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn missing_model_reports_install_path() {
    let root = TempRoot::new("missing");
    let mut client = client_for(root.path());
    client.send(&warmup(&"0".repeat(64))).expect("写 warmup");
    let (retryable, reason) = expect_error(&mut client, "model_missing");
    assert!(retryable);
    let reason = reason.expect("应带安装提示");
    assert!(reason.contains("asr"), "安装提示应包含模型族：{reason}");
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn digest_mismatch_is_detected_at_warmup() {
    let root = TempRoot::new("digest");
    root.write("asr/demo/model.bin", b"data");
    let json = manifest("demo", &"0".repeat(64), 4, "1.0.0");
    root.write("asr/demo/manifest.json", json.as_bytes());
    let mut client = client_for(root.path());
    client
        .send(&warmup(&hex_digest(json.as_bytes())))
        .expect("写 warmup");
    let (retryable, reason) = expect_error(&mut client, "model_unusable");
    assert!(!retryable);
    assert!(reason.unwrap_or_default().contains("摘要"));
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn uppercase_manifest_digest_is_accepted() {
    let root = TempRoot::new("manifest-upper");
    let manifest_sha = install_model(&root);
    let mut client = client_for(root.path());
    client
        .send(&warmup(&manifest_sha.to_uppercase()))
        .expect("写 warmup");
    let (_, reason) = expect_error(&mut client, "model_unusable");
    assert!(reason.unwrap_or_default().contains("运行时"));
    assert_eq!(client.close().expect("关闭").code(), Some(0));
}

#[test]
fn future_version_fails_preflight_with_13() {
    let root = TempRoot::new("future");
    root.write("asr/demo/model.bin", b"data");
    let json = manifest("demo", &hex_digest(b"data"), 4, "2.0.0");
    root.write("asr/demo/manifest.json", json.as_bytes());
    assert_eq!(exit_code_of(root.path()), 13);
}

#[test]
fn warmup_after_session_is_rejected() {
    let root = TempRoot::new("late-warmup");
    let manifest_sha = install_model(&root);
    let mut client = client_for(root.path());
    client
        .send(&Control::SessionStart(axon_voice_protocol::SessionStart {
            session_id: "s-1".to_owned(),
            mode: "full_duplex".to_owned(),
            lang: "zh".to_owned(),
            hot_words: vec![],
            sample_rate: 16_000,
        }))
        .expect("写 sessionStart");
    // 会话中预热属于序列错误，宿主直接退出 11。
    let _ = client.send(&warmup(&manifest_sha));
    assert_eq!(wait_for_exit(&mut client), Some(11));
}

fn wait_for_exit(client: &mut HostClient) -> Option<i32> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Some(status) = client.try_wait().expect("查询") {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    client.terminate();
    None
}
