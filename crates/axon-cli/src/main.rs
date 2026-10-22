//! Axon CLI binary。

use std::{io::Write, process::ExitCode};

use axon_cli::{AxonCli, run_parsed_cli_with_writer};

fn main() -> ExitCode {
    let cli = AxonCli::parse_from_args(std::env::args_os());
    let mut stdout = std::io::stdout().lock();
    let result = run_parsed_cli_with_writer(&cli, &mut stdout);
    let exit_code = ExitCode::from(result.exit_code);

    write_and_flush(&mut stdout, &result.stdout);
    write_and_flush(std::io::stderr().lock(), &result.stderr);

    exit_code
}

fn write_and_flush(mut writer: impl Write, content: &str) {
    if !content.is_empty() {
        let _ = writer.write_all(content.as_bytes());
    }
    let _ = writer.flush();
}
