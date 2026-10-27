//! 监督闭环：心跳超时、进程崩溃与重启上限。
//!
//! 仅在 `fake-host` feature 下运行（需要编译假宿主二进制）。

#![cfg(feature = "fake-host")]

use std::time::{Duration, Instant};

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::supervisor::{HostClient, HostConfig, Supervisor, SupervisorAction};

fn fake_host_config(mode: &str) -> HostConfig {
    let mut config = HostConfig::new(env!("CARGO_BIN_EXE_axon-voice-fake-host"), BUILD_COMMIT);
    config.heartbeat_timeout = Duration::from_millis(300);
    config
        .env
        .push(("AXON_FAKE_HOST_MODE".to_owned(), mode.to_owned()));
    config
}

#[test]
fn silent_host_is_killed_and_restart_requested() {
    let config = fake_host_config("silent");
    let mut client = HostClient::spawn(&config).expect("握手应成功");
    std::thread::sleep(Duration::from_millis(500));
    let mut supervisor = Supervisor::new(config);
    let (action, failure) = supervisor.guard(&mut client, Instant::now());
    assert_eq!(action, SupervisorAction::Restart);
    let failure = failure.expect("应有失败信息");
    assert_eq!(failure.sub_stage, "io");
}

#[test]
fn restart_limit_gives_up_after_three_restarts() {
    let config = fake_host_config("silent");
    let mut supervisor = Supervisor::new(config.clone());
    for expected in [
        SupervisorAction::Restart,
        SupervisorAction::Restart,
        SupervisorAction::Restart,
        SupervisorAction::GiveUp,
    ] {
        let mut client = HostClient::spawn(&config).expect("握手应成功");
        std::thread::sleep(Duration::from_millis(500));
        let (action, failure) = supervisor.guard(&mut client, Instant::now());
        assert_eq!(action, expected);
        assert!(failure.is_some());
    }
}

#[test]
fn crashed_host_is_detected() {
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
