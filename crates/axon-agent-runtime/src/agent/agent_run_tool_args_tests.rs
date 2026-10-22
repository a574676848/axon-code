use std::fs;
use std::path::Path;

use super::*;

#[test]
fn parses_search_schema_v3_arguments() {
    let input = agent_run_search_tool_input_from_provider_args(
        Path::new("E:/work"),
        r#"{"query":"axon","paths":["src/**/*.rs"],"regex_mode":"fixed_string","case_sensitive":true,"gitignore":false,"hidden":true,"context_before":2,"context_after":3,"file_offset":4,"max_files":5,"max_matches_per_file":6}"#,
    )
    .expect("search input");

    assert_eq!(input.query, "axon");
    assert_eq!(input.root, PathBuf::from("E:/work"));
    assert_eq!(input.paths, vec!["src/**/*.rs"]);
    assert_eq!(input.regex_mode, RegexSearchMode::FixedString);
    assert!(input.case_sensitive);
    assert!(!input.gitignore);
    assert!(input.hidden);
    assert_eq!(input.context_before, 2);
    assert_eq!(input.context_after, 3);
    assert_eq!(input.file_offset, 4);
    assert_eq!(input.max_files, Some(5));
    assert_eq!(input.max_matches_per_file, Some(6));
    assert!(!input.remote_requested);
}

#[test]
fn parses_invoke_skill_with_only_skill_name() {
    let input = agent_run_invoke_skill_tool_input_from_provider_args(
        r#"{"skill_name":"open-source-docs"}"#,
    )
    .expect("invoke_skill input");
    assert_eq!(input.skill_name, "open-source-docs");
    assert!(
        agent_run_invoke_skill_tool_input_from_provider_args(
            r#"{"skill_name":"open-source-docs","arguments":"ignored"}"#,
        )
        .is_err()
    );
}

#[test]
fn accepts_search_scalar_path_and_rejects_invalid_numeric_fields() {
    let input = agent_run_search_tool_input_from_provider_args(
        Path::new("E:/work"),
        r#"{"query":"axon","paths":"src"}"#,
    )
    .expect("单个路径字符串应归一化为单元素数组");
    assert_eq!(input.paths, vec!["src"]);
    assert!(
        agent_run_search_tool_input_from_provider_args(
            Path::new("E:/work"),
            r#"{"query":"axon","max_files":0}"#,
        )
        .is_err()
    );
}

#[test]
fn parses_read_file_and_normalizes_separators() {
    let workspace = temp_workspace("read-file-args");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(workspace.join("src").join("main.rs"), "fn main() {}\n").unwrap();
    let input =
        agent_run_read_file_tool_input_from_provider_args(&workspace, r#"{"path":"src\\main.rs"}"#)
            .expect("read_file input");

    assert_eq!(input.path, PathBuf::from("src/main.rs"));
    assert_same_path(&input.root, &workspace);
    assert_eq!(input.filter_level, None);
    assert_eq!(input.offset, None);
    assert_eq!(input.limit, None);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn parses_read_file_and_glob_pagination() {
    let workspace = temp_workspace("fs-pagination");
    let read = agent_run_read_file_tool_input_from_provider_args(
        &workspace,
        r#"{"path":"src/main.rs","offset":21,"limit":40}"#,
    )
    .expect("read_file pagination");
    let glob = agent_run_glob_tool_input_from_provider_args(
        &workspace,
        r#"{"paths":["src/**/*.rs"],"offset":0,"limit":20,"include":"files"}"#,
    )
    .expect("glob pagination");

    assert_eq!((read.offset, read.limit), (Some(21), Some(40)));
    assert_eq!((glob.offset, glob.limit), (0, Some(20)));
    assert_eq!(glob.paths, vec!["src/**/*.rs"]);
    assert_eq!(glob.include, GlobInclude::Files);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn parses_glob_include_array_as_combined_filter() {
    let workspace = temp_workspace("glob-include-array");
    let input = agent_run_glob_tool_input_from_provider_args(
        &workspace,
        r#"{"include":["files","directories"]}"#,
    )
    .expect("glob include 数组");
    assert_eq!(input.include, GlobInclude::All);

    let error = agent_run_glob_tool_input_from_provider_args(&workspace, r#"{"include":[]}"#)
        .err()
        .expect("空 include 数组必须拒绝");
    assert_eq!(error, "builtin.glob include 数组不能为空");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn accepts_provider_root_as_tool_context() {
    let workspace = temp_workspace("read-file-root-args");
    let input = agent_run_read_file_tool_input_from_provider_args(
        &workspace,
        r#"{"root":"tools","path":"config.toml"}"#,
    )
    .expect("工具参数解析不承担 root 权限校验");

    assert_eq!(input.path, PathBuf::from("config.toml"));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn accepts_absolute_and_parent_tool_paths() {
    let workspace = temp_workspace("provider-path-boundary");
    let absolute = agent_run_glob_tool_input_from_provider_args(
        &workspace,
        r#"{"paths":["E:\\project\\outside"]}"#,
    )
    .expect("绝对路径应交由审批策略处理");
    let parent = agent_run_read_file_tool_input_from_provider_args(
        &workspace,
        r#"{"path":"../outside.txt"}"#,
    )
    .expect("父级路径应交由审批策略处理");

    assert_eq!(absolute.paths, vec![r"E:\project\outside"]);
    assert_eq!(parent.path, PathBuf::from("../outside.txt"));
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn rejects_glob_empty_path_item() {
    let workspace = temp_workspace("glob-args");
    let error = agent_run_glob_tool_input_from_provider_args(&workspace, r#"{"paths":[""]}"#)
        .err()
        .expect("空 path 必须拒绝");

    assert_eq!(error, "builtin.glob paths 只能包含非空字符串");
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn parses_structured_shell_command_without_local_execution() {
    let input = agent_run_shell_tool_args_from_provider_input(
        &ToolCallInput::json(serde_json::json!({
            "command": "echo axon",
            "workdir": "nested",
            "timeout_ms": 1000,
            "tty": true,
            "yield_time_ms": 0,
            "max_output_tokens": 64
        })),
        Path::new("workspace"),
    )
    .expect("shell args");

    assert!(input.command.iter().any(|part| part.contains("echo axon")));
    assert_eq!(input.cwd, PathBuf::from("workspace/nested"));
    assert_eq!(input.timeout_ms, Some(1000));
    assert_eq!(input.yield_time_ms, 0);
    assert!(input.tty);
    assert_eq!(input.max_output_tokens, Some(64));
}

#[test]
fn shell_preserves_backslash_escapes() {
    let script = r#"printf \"READY\\n\"; printf \"\\\\done\""#;
    let input = agent_run_shell_tool_args_from_provider_input(
        &ToolCallInput::json(serde_json::json!({ "command": script })),
        Path::new("workspace"),
    )
    .expect("shell args");

    assert!(input.command.iter().any(|part| part.contains(script)));
}

#[test]
fn tty_shell_defaults_to_immediate_session_return() {
    let input = agent_run_shell_tool_args_from_provider_input(
        &ToolCallInput::json(serde_json::json!({
            "command": "IFS= read -r line",
            "tty": true
        })),
        Path::new("workspace"),
    )
    .expect("shell args");

    assert_eq!(input.yield_time_ms, 0);
}

#[test]
fn shell_rejects_raw_text_while_write_stdin_keeps_session_controls() {
    assert!(
        agent_run_shell_tool_args_from_provider_input(
            &ToolCallInput::text("echo axon"),
            Path::new("workspace"),
        )
        .is_err()
    );
    let write_stdin = agent_run_write_stdin_tool_input_from_provider_args(
        r#"{"session_id":"session-1","yield_time_ms":0,"max_output_tokens":64}"#,
        "test-owner",
    )
    .expect("write_stdin args");
    assert_eq!(write_stdin.owner_id, "test-owner");
    assert_eq!(write_stdin.yield_time_ms, 0);
    assert_eq!(write_stdin.max_output_tokens, Some(64));
}

#[test]
fn shell_rejects_invalid_timeout() {
    assert!(
        agent_run_shell_tool_args_from_provider_input(
            &ToolCallInput::json(serde_json::json!({
                "command": "printf terminal-ok",
                "timeout_ms": 999
            })),
            Path::new("workspace"),
        )
        .is_err()
    );
}

#[test]
fn shell_wrapper_auto_unwraps_outer_wrapper() {
    let args = agent_run_shell_tool_args_from_provider_input(
        &ToolCallInput::json(serde_json::json!({
            "command": "powershell -Command \"New-Item -ItemType Directory\""
        })),
        Path::new("workspace"),
    )
    .expect("外层 shell wrapper 应当被自动解包成功");

    assert!(
        args.command
            .iter()
            .any(|arg| arg.contains("New-Item -ItemType Directory"))
    );
}

#[test]
fn shell_wrapper_auto_unwraps_multiline_pwsh_command_from_screenshot_case() {
    let multiline_input = "pwsh -Command '\n$files = Get-ChildItem -Path crates -Recurse -Filter \"*.rs\"\n$results = @()\n\nforeach ($file in $files) {\n    $results += $file\n}\n'";
    let args = agent_run_shell_tool_args_from_provider_input(
        &ToolCallInput::json(serde_json::json!({
            "command": multiline_input
        })),
        Path::new("workspace"),
    )
    .expect("截图场景多行 pwsh -Command 应当被正确解包为裸脚本并成功执行");

    assert!(
        args.command
            .iter()
            .any(|arg| arg.contains("Get-ChildItem -Path crates"))
    );
    assert!(!args.command.iter().any(|arg| arg.contains("pwsh -Command")));
}

#[test]
fn shell_wrapper_extracts_windows_cd_prefix_and_uses_its_workdir() {
    let args = agent_run_shell_tool_args_from_provider_input(
        &ToolCallInput::json(serde_json::json!({
            "command": r#"cd /d E:\zbg\axon-code && powershell -Command "Get-Content README.md""#
        })),
        Path::new("workspace"),
    )
    .expect("Windows cd 前缀和 PowerShell wrapper 应当都能归一化");

    assert_eq!(args.cwd, PathBuf::from("E:/zbg/axon-code"));
    assert!(
        args.command
            .iter()
            .any(|arg| arg.contains("Get-Content README.md"))
    );
    assert!(
        !args
            .command
            .iter()
            .any(|arg| arg.contains("powershell -Command"))
    );
}

#[test]
fn shell_wrapper_accepts_internal_nesting_in_a_valid_script() {
    let args = agent_run_shell_tool_args_from_provider_input(
        &ToolCallInput::json(serde_json::json!({
            "command": "Write-Host test; pwsh -Command \"New-Item -ItemType Directory\""
        })),
        Path::new("workspace"),
    )
    .expect("内部嵌套 shell wrapper 不应再被拒绝");

    assert!(args.command.iter().any(|arg| arg.contains("pwsh -Command")));
}

#[test]
fn keeps_bracketed_text_that_is_not_apply_patch_noise() {
    for input in [
        "[Update total: 3]\nDEL 1.=1",
        "[notes.md#1A2B3C]\nDEL 1.=1",
        "[src/a.ts#1A2B3C]\nINS.TAIL:\n+fn added() {}",
    ] {
        let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input))
            .expect("edit args");
        assert_eq!(edit.input, input, "输入 {input} 不应被改写");
    }
}

#[test]
fn edit_tool_args_supports_aliases() {
    // code_edit alias
    let args1 =
        agent_run_edit_tool_args_from_provider_input(&ToolCallInput::json(serde_json::json!({
            "code_edit": "*** Begin Patch\n*** Add File: a.txt\n+hello\n*** End Patch"
        })))
        .unwrap();
    assert!(args1.input.contains("*** Add File: a.txt"));

    // patch alias
    let args2 =
        agent_run_edit_tool_args_from_provider_input(&ToolCallInput::json(serde_json::json!({
            "patch": "*** Begin Patch\n*** Add File: b.txt\n+world\n*** End Patch"
        })))
        .unwrap();
    assert!(args2.input.contains("*** Add File: b.txt"));
}

#[test]
fn edit_tool_args_unwraps_pure_hashline_in_patch_envelope() {
    let input = "*** Begin Patch\n[src/main.rs#ABCDEF]\nSWAP 1:\n-old\n+new\n*** End Patch";
    let args = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input)).unwrap();
    assert_eq!(args.input, "[src/main.rs#ABCDEF]\nSWAP 1:\n-old\n+new");
}

#[test]
fn edit_tool_args_strips_hashline_header_inside_update_file() {
    let input = "*** Begin Patch\n*** Update File: src/main.rs\n[src/main.rs#ABCDEF]\n@@\n-old\n+new\n*** End Patch";
    let args = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input)).unwrap();
    assert!(!args.input.contains("[src/main.rs#ABCDEF]"));
    assert!(args.input.contains("*** Update File: src/main.rs"));
}

#[test]
fn parses_job_tool_arguments_with_optional_read_window() {
    let args = agent_run_job_tool_args_from_provider_input(
        &ToolCallInput::from(r#"{"job_id":"12","yield_time_ms":1500,"max_output_tokens":500}"#),
        "builtin.job_output",
    )
    .expect("作业参数应解析成功");

    assert_eq!(args.job_id, crate::job_runtime::JobId(12));
    assert_eq!(args.yield_time_ms, Some(1_500));
    assert_eq!(args.max_output_tokens, Some(500));

    let bare = agent_run_job_tool_args_from_provider_input(
        &ToolCallInput::from(r#"{"job_id":"12"}"#),
        "builtin.job_kill",
    )
    .expect("只带 job_id 也应解析成功");
    assert_eq!(bare.job_id, crate::job_runtime::JobId(12));
    assert_eq!(bare.yield_time_ms, None);
    assert_eq!(bare.max_output_tokens, None);
}

/// 会话 ID 与臆造的 ID 必须在解析层就被拒绝，不得落到「猜一个作业」的逻辑里。
#[test]
fn job_tool_arguments_reject_session_ids_and_non_positive_ids() {
    for invalid in [
        r#"{"job_id":"sess_123"}"#,
        r#"{"job_id":"0"}"#,
        r#"{"job_id":"-1"}"#,
        r#"{"job_id":"1.5"}"#,
        r#"{"job_id":12}"#,
        r#"{}"#,
    ] {
        assert!(
            agent_run_job_tool_args_from_provider_input(
                &ToolCallInput::from(invalid),
                "builtin.job_output"
            )
            .is_err(),
            "非法作业 ID 必须被拒绝：{invalid}"
        );
    }

    assert!(
        agent_run_job_tool_args_from_provider_input(
            &ToolCallInput::text("12"),
            "builtin.job_output"
        )
        .is_err(),
        "作业工具只接受 JSON 参数"
    );
}

fn temp_workspace(tag: &str) -> PathBuf {
    let dir = axon_test_support::test_temp_root()
        .join(format!("axon-tool-args-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn assert_same_path(actual: &Path, expected: &Path) {
    assert_eq!(
        actual.canonicalize().unwrap(),
        expected.canonicalize().unwrap()
    );
}

#[path = "agent_run_patch_args_tests.rs"]
mod agent_run_patch_args_tests;
