//! 监督闭环：心跳超时、进程崩溃与重启上限。
//!
//! 仅在 `fake-host` feature 下运行（需要编译假宿主二进制）。

#![cfg(feature = "fake-host")]

use std::time::{Duration, Instant};

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::supervisor::{HostClient, HostConfig, Supervisor, SupervisorAction};

fn fake_host_config(mode: &str) -> HostConfig {
    let mut config = HostConfig::new(env!("CARGO_BIN_EXE_axon-voice-fake-host"), BUILD_COMMIT);
    config
        .env
        .push(("AXON_FAKE_HOST_MODE".to_owned(), mode.to_owned()));
    config
}

#[test]
fn silent_host_times_out_and_requests_restart() {
    let config = fake_host_config("silent");
    let client = HostClient::spawn(&config).expect("握手应成功");
    std::thread::sleep(Duration::from_millis(3200));
    let ok = client.heartbeat_ok(Instant::now());
    assert!(!ok, "静默宿主不应被判定为健康");
    let mut supervisor = Supervisor::new(config);
    assert_eq!(supervisor.tick(ok), SupervisorAction::Restart);
    let status = client.kill().expect("强杀应成功");
    assert!(!status.success());
}

#[test]
fn crashed_host_is_detected_and_restart_is_limited() {
    let config = fake_host_config("crash");
    let mut client = HostClient::spawn(&config).expect("握手应成功");
    std::thread::sleep(Duration::from_millis(200));
    let exited = client.try_wait().expect("查询应成功");
    assert!(exited.is_some_and(|status| !status.success()));

    let mut supervisor = Supervisor::new(config);
    for _ in 0..3 {
        assert_eq!(supervisor.after_exit(), SupervisorAction::Restart);
    }
    assert_eq!(supervisor.after_exit(), SupervisorAction::GiveUp);
}
