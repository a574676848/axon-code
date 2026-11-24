use clap::Parser;

use super::{AxonCli, run_parsed_cli};

#[test]
fn config_path_defaults_to_user_config_and_preserves_explicit_override() {
    let user_config = axon_config::ConfigLoader::default_user_config_path().unwrap();
    assert_eq!(
        crate::management_commands::resolved_config_path(None).unwrap(),
        user_config
    );

    let explicit = std::path::Path::new("isolated/config.toml");
    assert_eq!(
        crate::management_commands::resolved_config_path(Some(explicit)).unwrap(),
        explicit
    );
}

#[test]
fn exec_command_accepts_hyphenated_program_args() {
    let cli = AxonCli::parse_from_args([
        "axon",
        "exec",
        "--timeout-ms",
        "10000",
        "cargo",
        "--version",
    ]);

    let Some(super::AxonCommand::Exec(command)) = cli.command else {
        panic!("exec 子命令应被解析");
    };
    assert_eq!(command.timeout_ms, Some(10_000));
    assert_eq!(command.kind, super::ExecKind::Command);
    assert_eq!(command.command, vec!["cargo", "--version"]);
}

#[test]
fn coding_commands_are_top_level_and_planned() {
    let run = AxonCli::parse_from_args(["axon", "run", "检查仓库"]);
    assert!(matches!(run.command, Some(super::AxonCommand::Run(_))));
    assert_eq!(
        super::plan_invocation(&run),
        super::CliInvocation::Command { name: "run" }
    );

    let stop = AxonCli::parse_from_args(["axon", "stop", "--session", "session-1"]);
    assert!(matches!(stop.command, Some(super::AxonCommand::Stop(_))));

    let session = AxonCli::parse_from_args(["axon", "debug", "session", "list"]);
    assert!(matches!(
        session.command,
        Some(super::AxonCommand::Debug(super::DebugCommand {
            command: Some(super::DebugSubcommand::Session(_)),
        }))
    ));

    let removed = AxonCli::try_parse_from(["axon", "session", "list"]);
    assert!(removed.is_err());

    let chat = AxonCli::parse_from_args(["axon", "chat"]);
    assert!(chat.command.is_none());
    assert_eq!(chat.prompt.as_deref(), Some("chat"));

    let work = AxonCli::parse_from_args(["axon", "work"]);
    assert!(work.command.is_none());
    assert_eq!(work.prompt.as_deref(), Some("work"));
}

#[cfg(feature = "tui")]
#[test]
fn no_subcommand_without_prompt_starts_agent_tui_entry() {
    let cli = AxonCli::parse_from_args(["axon", "--format", "json"]);
    let result = run_parsed_cli(&cli);
    let report: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(report["status"], "agent_tui_started");
    assert_eq!(report["kind"], "agent");
    assert_eq!(report["focusTarget"], "agent_composer");
    assert!(report["submittedPrompt"].is_null());
}

#[cfg(feature = "tui")]
#[test]
fn no_subcommand_with_prompt_prefills_agent_tui_entry() {
    let cli = AxonCli::parse_from_args(["axon", "--format", "json", "总结仓库"]);
    let result = run_parsed_cli(&cli);
    let report: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(report["status"], "agent_tui_started");
    assert_eq!(report["prompt"], "总结仓库");
    assert_eq!(report["focusTarget"], "agent_composer");
}
