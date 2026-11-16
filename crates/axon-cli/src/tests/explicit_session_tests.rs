//! `axon run --session` 的存在性校验测试。

use super::{AxonCli, RunCommand, validate_explicit_session};
use crate::AxonCommand;
use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore as _, SqliteRuntimeStore};

fn parse_run(config_path: &str, session: &str) -> (AxonCli, RunCommand) {
    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path,
        "run",
        "继续上次的话题",
        "--session",
        session,
    ]);
    let Some(AxonCommand::Run(command)) = cli.command.clone() else {
        panic!("必须解析成 run 子命令");
    };
    (cli, command)
}

#[test]
fn unknown_session_is_rejected_with_actionable_message() {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    SqliteRuntimeStore::from_root(directory.path().join("data"))
        .initialize()
        .expect("初始化运行态库");
    let (cli, command) = parse_run(config_path.to_str().expect("UTF-8 路径"), "missing-session");

    let error = validate_explicit_session(&cli, &command).expect_err("未知会话必须失败");
    assert!(
        error.contains("会话不存在"),
        "错误信息应说明会话不存在：{error}"
    );
    assert!(
        error.contains("axon sessions list"),
        "错误信息应指出下一步命令：{error}"
    );
}

#[test]
fn existing_session_passes_validation() {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    let store = SqliteRuntimeStore::from_root(directory.path().join("data"));
    store.initialize().expect("初始化运行态库");
    let session_id = DaemonSessionId::new("session-known").expect("合法 ID");
    store
        .create_session(&session_id, "/workspace", 1)
        .expect("建会话");
    let (cli, command) = parse_run(config_path.to_str().expect("UTF-8 路径"), "session-known");

    validate_explicit_session(&cli, &command).expect("已存在的会话必须通过校验");
}

#[test]
fn run_without_session_is_not_validated_here() {
    let cli = AxonCli::parse_from_args(["axon", "run", "你好"]);
    let Some(AxonCommand::Run(command)) = cli.command.clone() else {
        panic!("必须解析成 run 子命令");
    };
    validate_explicit_session(&cli, &command).expect("不带 --session 时本函数不做任何检查");
}
