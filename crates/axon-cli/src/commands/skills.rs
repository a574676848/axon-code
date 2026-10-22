use std::{fmt::Write as _, path::Path};

use axon_agent_runtime::{Skill, SkillLoadError};
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result, resolved_config_path},
    runtime_tools::build_runtime_skills_manager,
};

/// `axon skills` 参数。
#[derive(Debug, Clone, clap::Args, PartialEq, Eq)]
#[command(after_help = "运行 `axon skills --summary` 查看当前已加载 skill 数量。")]
pub struct SkillsCommand {
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    /// 输出运行时加载摘要，便于确认本机当前加载了多少个 skill。
    #[arg(long)]
    pub summary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillsReport {
    status: &'static str,
    total_skills: usize,
    load_errors: usize,
    skills: Vec<SkillView>,
    errors: Vec<SkillLoadErrorView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillView {
    name: String,
    description: String,
    scope: String,
    source_ref: String,
    content_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillLoadErrorView {
    path: String,
    message: String,
}

pub(crate) fn run_skills(
    command: &SkillsCommand,
    global_format: OutputFormat,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let manager = match build_runtime_skills_manager(&path, cwd) {
        Ok(manager) => manager,
        Err(error) => return cli_error(error),
    };
    let outcome = manager.load();
    let report = skills_report(outcome.skills, outcome.errors);
    if command.summary {
        return render_skills_summary(command.format.unwrap_or(global_format), &report);
    }
    render_skills(command.format.unwrap_or(global_format), &report)
}

fn skills_report(skills: Vec<Skill>, errors: Vec<SkillLoadError>) -> SkillsReport {
    let total_skills = skills.len();
    let mut views = skills.into_iter().map(skill_view).collect::<Vec<_>>();
    views.sort_by(|left, right| left.name.cmp(&right.name));
    SkillsReport {
        status: "skills_ready",
        total_skills,
        load_errors: errors.len(),
        skills: views,
        errors: errors.into_iter().map(error_view).collect(),
    }
}

fn skill_view(skill: Skill) -> SkillView {
    let metadata = skill.metadata;
    let source_ref = metadata.source_ref();
    SkillView {
        name: metadata.name,
        description: metadata.description,
        scope: metadata.scope.to_string(),
        source_ref,
        content_digest: metadata.content_digest,
    }
}

fn error_view(error: SkillLoadError) -> SkillLoadErrorView {
    SkillLoadErrorView {
        path: error.path.display().to_string(),
        message: error.message,
    }
}

fn render_skills(format: OutputFormat, report: &SkillsReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: render_skills_text(report),
            stderr: String::new(),
        },
    }
}

fn render_skills_summary(format: OutputFormat, report: &SkillsReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon skills 已加载 {} 个，加载错误 {} 个\n",
                report.total_skills, report.load_errors
            ),
            stderr: String::new(),
        },
    }
}

fn render_skills_text(report: &SkillsReport) -> String {
    let mut output = format!(
        "Axon skills: total={} errors={}\n",
        report.total_skills, report.load_errors
    );
    if report.skills.is_empty() {
        output.push_str("未加载 Skill\n");
        return output;
    }
    for skill in &report.skills {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}",
            skill.name, skill.scope, skill.source_ref, skill.description
        );
    }
    output
}
