use super::*;

#[derive(Debug, Clone)]
struct FakeExecutor {
    exit_code: i32,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

impl CommandExecutor for FakeExecutor {
    fn execute(&self, request: &ExecRequest) -> Result<ExecReport, &'static str> {
        Ok(ExecReport::new(
            request,
            &ExecOutput {
                exit_code: self.exit_code,
                timed_out: self.timed_out,
                duration_ms: 7,
                stdout: self.stdout.as_bytes().to_vec(),
                stderr: self.stderr.as_bytes().to_vec(),
            },
        ))
    }
}

#[test]
fn exec_request_rejects_empty_command_and_shell_wrapper() {
    assert_eq!(
        default_request(Vec::new(), ExecKind::Command, None).unwrap_err(),
        EXEC_EMPTY_COMMAND_ERROR
    );
    assert_eq!(
        default_request(
            vec!["sh".to_owned(), "-c".to_owned()],
            ExecKind::Command,
            None
        )
        .unwrap_err(),
        EXEC_SHELL_DENIED_ERROR
    );
    assert_eq!(
        default_request(
            vec!["pwsh.exe".to_owned(), "-c".to_owned()],
            ExecKind::Command,
            None
        )
        .unwrap_err(),
        EXEC_SHELL_DENIED_ERROR
    );
}

#[test]
fn exec_request_only_rejects_zero_explicit_timeout() {
    assert_eq!(
        default_request(vec!["echo".to_owned()], ExecKind::Command, Some(0)).unwrap_err(),
        EXEC_TIMEOUT_ERROR
    );
    let no_deadline = default_request(vec!["echo".to_owned()], ExecKind::Command, None).unwrap();
    assert_eq!(no_deadline.timeout_ms, None);
    let explicit =
        default_request(vec!["echo".to_owned()], ExecKind::Command, Some(u64::MAX)).unwrap();
    assert_eq!(explicit.timeout_ms, Some(u64::MAX));
}

#[test]
fn exec_report_is_structured_host_process_contract() {
    let request = default_request(
        vec!["echo".to_owned(), "ok".to_owned()],
        ExecKind::Command,
        None,
    )
    .unwrap();
    let report = FakeExecutor {
        exit_code: 0,
        stdout: "ok\n".to_owned(),
        stderr: String::new(),
        timed_out: false,
    }
    .execute(&request)
    .unwrap();

    assert_eq!(report.status, ExecStatus::Completed);
    assert_eq!(report.kind, EXEC_KIND_COMMAND);
    assert!(report.network_allowed);
    assert!(report.writes_files_allowed);
    assert_eq!(report.test_passed, None);
    assert!(!report.shell_used);
    assert!(!report.remote_trigger_allowed);
    assert_eq!(report.safety.policy, EXEC_POLICY_HOST_PROCESS_NO_SHELL);
    assert_eq!(report.safety.shell, EXEC_SHELL_POLICY_DENIED);
    assert_eq!(report.safety.stdin, EXEC_STDIN_POLICY_NULL);
    assert!(!report.safety.remote_trigger_allowed);
    assert_eq!(report.stdout.preview, "ok\n");
}

#[test]
fn exec_report_preserves_large_output() {
    let request = default_request(vec!["print".to_owned()], ExecKind::Command, None).unwrap();
    let report = FakeExecutor {
        exit_code: 0,
        stdout: "x".repeat(20_000),
        stderr: String::new(),
        timed_out: false,
    }
    .execute(&request)
    .unwrap();

    assert!(!report.stdout.truncated);
    assert_eq!(report.stdout.retained_chars, 20_000);
    assert_eq!(report.stdout.preview.len(), 20_000);
}

#[test]
fn exec_test_kind_projects_test_passed_without_review() {
    let request = default_request(
        vec!["cargo".to_owned(), "test".to_owned()],
        ExecKind::Test,
        None,
    )
    .unwrap();
    let report = FakeExecutor {
        exit_code: 0,
        stdout: "test result: ok\n".to_owned(),
        stderr: String::new(),
        timed_out: false,
    }
    .execute(&request)
    .unwrap();

    assert_eq!(report.kind, EXEC_KIND_TEST);
    assert_eq!(report.test_passed, Some(true));
    assert!(!report.remote_trigger_allowed);
}

fn default_request(
    command: Vec<String>,
    kind: ExecKind,
    timeout_ms: Option<u64>,
) -> Result<ExecRequest, &'static str> {
    ExecRequest::from_cli(command, kind, None, timeout_ms)
}
