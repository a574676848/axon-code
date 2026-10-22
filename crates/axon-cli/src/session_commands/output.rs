use std::{fs, path::PathBuf};

use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

/// `axon debug session output` 参数。
#[derive(Debug, Clone, clap::Args, PartialEq, Eq)]
pub struct SessionOutputCommand {
    /// tool output 归档路径。
    pub path: PathBuf,
    /// 可选输出字符上限。
    #[arg(long)]
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionOutputReport {
    status: &'static str,
    path: String,
    preview: String,
    truncated: bool,
    original_chars: usize,
    retained_chars: usize,
}

pub(super) fn run_session_output(
    format: OutputFormat,
    command: &SessionOutputCommand,
) -> CliRunResult {
    if command.max_chars == Some(0) {
        return cli_error("session output --max-chars 必须大于 0");
    }
    let Ok(text) = fs::read_to_string(&command.path) else {
        return cli_error(format!("无法读取输出归档：{}", command.path.display()));
    };
    let original_chars = text.chars().count();
    let preview = command.max_chars.map_or_else(
        || text.clone(),
        |limit| text.chars().take(limit).collect::<String>(),
    );
    let retained_chars = preview.chars().count();
    let report = SessionOutputReport {
        status: "session_output_ready",
        path: command.path.display().to_string(),
        preview,
        truncated: original_chars > retained_chars,
        original_chars,
        retained_chars,
    };
    render_output(format, &report)
}

fn render_output(format: OutputFormat, report: &SessionOutputReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "{}\n\n-- output: {} chars={} retained={} truncated={} --\n",
                report.preview,
                report.path,
                report.original_chars,
                report.retained_chars,
                report.truncated
            ),
            stderr: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_output_preserves_full_archive_by_default() {
        const OUTPUT_CHARS: usize = 70_000;

        let path = axon_test_support::test_temp_root().join(format!(
            "axon-session-output-{}-{}.txt",
            std::process::id(),
            OUTPUT_CHARS
        ));
        std::fs::write(&path, "x".repeat(OUTPUT_CHARS)).expect("write output archive");

        let result = run_session_output(
            OutputFormat::Json,
            &SessionOutputCommand {
                path: path.clone(),
                max_chars: None,
            },
        );
        let report: serde_json::Value =
            serde_json::from_str(&result.stdout).expect("session output report");

        assert_eq!(report["originalChars"], OUTPUT_CHARS);
        assert_eq!(report["retainedChars"], OUTPUT_CHARS);
        assert_eq!(report["truncated"], false);
        let _ = std::fs::remove_file(path);
    }
}
