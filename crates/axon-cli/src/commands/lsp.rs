//! `axon lsp` 命令面。

use std::path::{Path, PathBuf};

use axon_agent_runtime::{
    LspCatalogRuntime, LspDiagnosticsMode, LspDiagnosticsQuery, LspDiagnosticsRuntime,
    LspDocumentLocationMethod, LspDocumentLocationQuery, LspDocumentLocationRuntime,
    LspWorkspaceSymbolQuery, LspWorkspaceSymbolRuntime,
};
use serde::Serialize;

use crate::{
    CliRunResult, LspCommand, LspDiagnosticsCommand, LspDiagnosticsModeArg, LspPositionCommand,
    LspServersCommand, LspSubcommand, LspWorkspaceSymbolCommand, OutputFormat,
    management_commands::{cli_error, json_result},
};

/// 执行 `axon lsp`。
pub(crate) fn run_lsp(
    command: &LspCommand,
    format: OutputFormat,
    cwd: Option<&Path>,
) -> CliRunResult {
    match &command.command {
        LspSubcommand::Servers(command) => run_lsp_servers(command, format, cwd),
        LspSubcommand::WorkspaceSymbol(command) => run_workspace_symbol(command, format, cwd),
        LspSubcommand::Definition(command) => {
            run_document_location(command, &LspDocumentLocationMethod::Definition, format, cwd)
        }
        LspSubcommand::References(command) => {
            run_document_location(command, &LspDocumentLocationMethod::References, format, cwd)
        }
        LspSubcommand::Diagnostics(command) => run_diagnostics(command, format, cwd),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LspServersReport {
    workspace_root: String,
    server_count: usize,
    available_servers: Vec<String>,
    launchable_servers: Vec<String>,
    coverage_matched_servers: Vec<String>,
    coverage_filtered_servers: Vec<String>,
    scanned_files: usize,
    truncated: bool,
    capabilities: Vec<String>,
}

fn run_lsp_servers(
    command: &LspServersCommand,
    format: OutputFormat,
    cwd: Option<&Path>,
) -> CliRunResult {
    let workspace_root = match resolve_workspace_root(command.workspace_root.as_deref(), cwd) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let summary = LspCatalogRuntime::summary();
    let coverage = LspCatalogRuntime::resolve_workspace_coverage(&workspace_root);
    let launchable_servers = LspCatalogRuntime::resolve_workspace_launch_plans(&workspace_root)
        .into_iter()
        .map(|plan| plan.server_id)
        .collect::<Vec<_>>();
    let report = LspServersReport {
        workspace_root: workspace_root.display().to_string(),
        server_count: summary.total_servers,
        available_servers: summary
            .available_servers
            .into_iter()
            .map(|server| server.id)
            .collect(),
        launchable_servers,
        coverage_matched_servers: coverage.matched_servers,
        coverage_filtered_servers: coverage.filtered_servers,
        scanned_files: coverage.scanned_files,
        truncated: coverage.truncated,
        capabilities: summary.exposed_capabilities,
    };
    render_report(format, &report, |report| {
        format!(
            "Axon LSP servers\n- workspace_root: {}\n- server_count: {}\n- available_servers: {}\n- launchable_servers: {}\n- coverage_matched_servers: {}\n- coverage_filtered_servers: {}\n- scanned_files: {}\n- truncated: {}\n- capabilities: {}\n",
            report.workspace_root,
            report.server_count,
            display_list(&report.available_servers),
            display_list(&report.launchable_servers),
            display_list(&report.coverage_matched_servers),
            display_list(&report.coverage_filtered_servers),
            report.scanned_files,
            report.truncated,
            display_list(&report.capabilities),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LspWorkspaceSymbolCliReport {
    query: String,
    workspace_root: String,
    plan_only: bool,
    launchable_servers: Vec<String>,
    coverage_matched_servers: Vec<String>,
    hit_servers: Vec<String>,
    hit_count: usize,
}

fn run_workspace_symbol(
    command: &LspWorkspaceSymbolCommand,
    format: OutputFormat,
    cwd: Option<&Path>,
) -> CliRunResult {
    let workspace_root = match resolve_workspace_root(command.workspace_root.as_deref(), cwd) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let query = LspWorkspaceSymbolQuery {
        query: command.query.clone(),
        workspace_root: workspace_root.display().to_string(),
        text_document_uri: None,
    };
    let report = if command.plan_only {
        LspWorkspaceSymbolRuntime::plan_only(&query)
    } else {
        LspWorkspaceSymbolRuntime::query(&query)
    };
    let report = match report {
        Ok(report) => report,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let view = LspWorkspaceSymbolCliReport {
        query: command.query.clone(),
        workspace_root: workspace_root.display().to_string(),
        plan_only: command.plan_only,
        launchable_servers: report.launchable_servers,
        coverage_matched_servers: report.coverage.matched_servers,
        hit_servers: report
            .hits
            .iter()
            .map(|hit| hit.server_id.clone())
            .collect(),
        hit_count: report.hits.len(),
    };
    render_report(format, &view, |report| {
        format!(
            "Axon LSP workspace-symbol\n- query: {}\n- workspace_root: {}\n- plan_only: {}\n- launchable_servers: {}\n- coverage_matched_servers: {}\n- hit_servers: {}\n- hit_count: {}\n",
            report.query,
            report.workspace_root,
            report.plan_only,
            display_list(&report.launchable_servers),
            display_list(&report.coverage_matched_servers),
            display_list(&report.hit_servers),
            report.hit_count,
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LspDocumentLocationCliReport {
    method: String,
    workspace_root: String,
    file: String,
    line: u32,
    character: u32,
    plan_only: bool,
    launchable_servers: Vec<String>,
    hit_servers: Vec<String>,
    hit_count: usize,
}

fn run_document_location(
    command: &LspPositionCommand,
    method: &LspDocumentLocationMethod,
    format: OutputFormat,
    cwd: Option<&Path>,
) -> CliRunResult {
    let workspace_root = match resolve_workspace_root(command.workspace_root.as_deref(), cwd) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let file = match resolve_existing_file(&workspace_root, &command.file) {
        Ok(file) => file,
        Err(error) => return cli_error(error),
    };
    let text_document_uri = match path_to_file_uri(&file) {
        Ok(uri) => uri,
        Err(error) => return cli_error(error),
    };
    let launchable_servers =
        match LspCatalogRuntime::resolve_document_launch_plans(&workspace_root, &text_document_uri)
        {
            Ok(plans) => plans.into_iter().map(|plan| plan.server_id).collect(),
            Err(error) => return cli_error(format!("{error:?}")),
        };
    if command.plan_only {
        let report = LspDocumentLocationCliReport {
            method: location_method_name(method).to_owned(),
            workspace_root: workspace_root.display().to_string(),
            file: file.display().to_string(),
            line: command.line,
            character: command.character,
            plan_only: true,
            launchable_servers,
            hit_servers: Vec::new(),
            hit_count: 0,
        };
        return render_report(format, &report, |report| {
            format!(
                "Axon LSP {}\n- workspace_root: {}\n- file: {}\n- position: {}:{}\n- plan_only: true\n- launchable_servers: {}\n",
                report.method,
                report.workspace_root,
                report.file,
                report.line,
                report.character,
                display_list(&report.launchable_servers),
            )
        });
    }
    let query = LspDocumentLocationQuery {
        method: method.clone(),
        workspace_root: workspace_root.display().to_string(),
        text_document_uri,
        line: command.line,
        character: command.character,
    };
    let report = match LspDocumentLocationRuntime::query(&query) {
        Ok(report) => report,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let view = LspDocumentLocationCliReport {
        method: location_method_name(method).to_owned(),
        workspace_root: workspace_root.display().to_string(),
        file: file.display().to_string(),
        line: command.line,
        character: command.character,
        plan_only: false,
        launchable_servers,
        hit_servers: report
            .hits
            .iter()
            .map(|hit| hit.server_id.clone())
            .collect(),
        hit_count: report.hits.len(),
    };
    render_report(format, &view, |report| {
        format!(
            "Axon LSP {}\n- workspace_root: {}\n- file: {}\n- position: {}:{}\n- launchable_servers: {}\n- hit_servers: {}\n- hit_count: {}\n",
            report.method,
            report.workspace_root,
            report.file,
            report.line,
            report.character,
            display_list(&report.launchable_servers),
            display_list(&report.hit_servers),
            report.hit_count,
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LspDiagnosticsCliReport {
    mode: String,
    workspace_root: String,
    file: Option<String>,
    plan_only: bool,
    launchable_servers: Vec<String>,
    hit_servers: Vec<String>,
    hit_count: usize,
}

fn run_diagnostics(
    command: &LspDiagnosticsCommand,
    format: OutputFormat,
    cwd: Option<&Path>,
) -> CliRunResult {
    let workspace_root = match resolve_workspace_root(command.workspace_root.as_deref(), cwd) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let file = match (&command.mode, &command.file) {
        (LspDiagnosticsModeArg::Document, None) => {
            return cli_error("document diagnostics 需要 --file");
        }
        (LspDiagnosticsModeArg::Document | LspDiagnosticsModeArg::Workspace, Some(path)) => {
            match resolve_existing_file(&workspace_root, path) {
                Ok(file) => Some(file),
                Err(error) => return cli_error(error),
            }
        }
        (LspDiagnosticsModeArg::Workspace, None) => None,
    };
    let text_document_uri = match file.as_deref().map(path_to_file_uri).transpose() {
        Ok(uri) => uri,
        Err(error) => return cli_error(error),
    };
    let launchable_servers = match command.mode {
        LspDiagnosticsModeArg::Document => {
            match LspCatalogRuntime::resolve_document_launch_plans(
                &workspace_root,
                text_document_uri.as_deref().unwrap_or_default(),
            ) {
                Ok(plans) => plans.into_iter().map(|plan| plan.server_id).collect(),
                Err(error) => return cli_error(format!("{error:?}")),
            }
        }
        LspDiagnosticsModeArg::Workspace => {
            LspCatalogRuntime::resolve_workspace_launch_plans(&workspace_root)
                .into_iter()
                .map(|plan| plan.server_id)
                .collect()
        }
    };
    if command.plan_only {
        let report = LspDiagnosticsCliReport {
            mode: diagnostics_mode_name(command.mode).to_owned(),
            workspace_root: workspace_root.display().to_string(),
            file: file.as_ref().map(|file| file.display().to_string()),
            plan_only: true,
            launchable_servers,
            hit_servers: Vec::new(),
            hit_count: 0,
        };
        return render_report(format, &report, |report| {
            format!(
                "Axon LSP diagnostics\n- mode: {}\n- workspace_root: {}\n- file: {}\n- plan_only: true\n- launchable_servers: {}\n",
                report.mode,
                report.workspace_root,
                report.file.as_deref().unwrap_or("none"),
                display_list(&report.launchable_servers),
            )
        });
    }
    let query = LspDiagnosticsQuery {
        mode: match command.mode {
            LspDiagnosticsModeArg::Document => LspDiagnosticsMode::Document,
            LspDiagnosticsModeArg::Workspace => LspDiagnosticsMode::Workspace,
        },
        workspace_root: workspace_root.display().to_string(),
        text_document_uri,
    };
    let report = match LspDiagnosticsRuntime::query(&query) {
        Ok(report) => report,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let view = LspDiagnosticsCliReport {
        mode: diagnostics_mode_name(command.mode).to_owned(),
        workspace_root: workspace_root.display().to_string(),
        file: file.as_ref().map(|file| file.display().to_string()),
        plan_only: false,
        launchable_servers,
        hit_servers: report
            .hits
            .iter()
            .map(|hit| hit.server_id.clone())
            .collect(),
        hit_count: report.hits.len(),
    };
    render_report(format, &view, |report| {
        format!(
            "Axon LSP diagnostics\n- mode: {}\n- workspace_root: {}\n- file: {}\n- launchable_servers: {}\n- hit_servers: {}\n- hit_count: {}\n",
            report.mode,
            report.workspace_root,
            report.file.as_deref().unwrap_or("none"),
            display_list(&report.launchable_servers),
            display_list(&report.hit_servers),
            report.hit_count,
        )
    })
}

fn resolve_workspace_root(explicit: Option<&Path>, cwd: Option<&Path>) -> Result<PathBuf, String> {
    let root = explicit
        .map(Path::to_path_buf)
        .or_else(|| cwd.map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "无法解析 LSP 工作区根目录".to_owned())?;
    if root.is_dir() {
        Ok(root)
    } else {
        Err(format!("LSP 工作区根目录不存在：{}", root.display()))
    }
}

fn resolve_existing_file(workspace_root: &Path, file: &Path) -> Result<PathBuf, String> {
    let candidate = if file.is_absolute() {
        file.to_path_buf()
    } else {
        workspace_root.join(file)
    };
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(format!("LSP 目标文件不存在：{}", candidate.display()))
    }
}

fn location_method_name(method: &LspDocumentLocationMethod) -> &'static str {
    match method {
        LspDocumentLocationMethod::Definition => "definition",
        LspDocumentLocationMethod::References => "references",
        LspDocumentLocationMethod::Hover => "hover",
        LspDocumentLocationMethod::Implementation => "implementation",
        LspDocumentLocationMethod::PrepareCallHierarchy => "prepareCallHierarchy",
        LspDocumentLocationMethod::IncomingCalls => "incomingCalls",
        LspDocumentLocationMethod::OutgoingCalls => "outgoingCalls",
        LspDocumentLocationMethod::DocumentSymbol => "documentSymbol",
    }
}

fn diagnostics_mode_name(mode: LspDiagnosticsModeArg) -> &'static str {
    match mode {
        LspDiagnosticsModeArg::Document => "document",
        LspDiagnosticsModeArg::Workspace => "workspace",
    }
}

fn display_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(",")
    }
}

fn path_to_file_uri(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path)
        .map(String::from)
        .map_err(|()| format!("无法转换 LSP 文件 URI：{}", path.display()))
}

fn render_report<T>(
    format: OutputFormat,
    report: &T,
    render_text: impl FnOnce(&T) -> String,
) -> CliRunResult
where
    T: Serialize,
{
    match format {
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: render_text(report),
            stderr: String::new(),
        },
        OutputFormat::Json => json_result(report),
    }
}
