pub use axon_memory_ops::MemoryExportCommand;
use axon_memory_ops::run_memory_export as run_memory_export_op;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_config_error, cli_error},
};

use super::MemoryCliContext;

pub(super) fn run_memory_export(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryExportCommand,
) -> CliRunResult {
    let loaded = match axon_config::ConfigLoader::load_from_path(&context.config_path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    let ops_context =
        match axon_memory_ops::open_memory_ops_context(&context.config_path, &loaded.config) {
            Ok(context) => context,
            Err(error) => return cli_error(error),
        };
    ops_result(run_memory_export_op(
        memory_ops_format(format),
        &ops_context,
        command,
    ))
}

fn memory_ops_format(format: OutputFormat) -> axon_memory_ops::MemoryOpsOutputFormat {
    match format {
        OutputFormat::Text => axon_memory_ops::MemoryOpsOutputFormat::Text,
        OutputFormat::Json => axon_memory_ops::MemoryOpsOutputFormat::Json,
    }
}

fn ops_result(result: axon_memory_ops::MemoryOpsResult) -> CliRunResult {
    CliRunResult {
        exit_code: result.exit_code,
        stdout: result.stdout,
        stderr: result.stderr,
    }
}
