use std::fmt::Write as _;

use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
    run_commands::{
        run_skill_evolution_materialize::promote_validated_draft,
        run_skill_evolution_store::{
            SkillEvolutionRecord, SkillEvolutionStage, SkillEvolutionStore,
        },
    },
};

use super::MemoryCliContext;

const SKILL_EVOLUTION_STATUS_READY: &str = "skill_evolution_status_ready";

/// `axon memory skill-evolution-status` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SkillEvolutionStatusCommand {
    /// 目标 session id。
    pub session_id: String,
}

/// `axon memory skill-evolution-promote` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SkillEvolutionPromoteCommand {
    /// 产生 validated draft 的 session id。
    pub session_id: String,
    /// 待晋升的 Skill 名称。
    pub skill_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillEvolutionStatusReport {
    status: &'static str,
    session_id: String,
    records: Vec<SkillEvolutionRecord>,
    state_file: String,
    review_required: bool,
}

pub(super) fn run_skill_evolution_status(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &SkillEvolutionStatusCommand,
) -> CliRunResult {
    if command.session_id.trim().is_empty() {
        return cli_error("session id 不能为空");
    }
    let store = match SkillEvolutionStore::open_at(&context.axon_root) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let records = match store.records_for_session(&command.session_id) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let review_required = records
        .iter()
        .any(|record| record.kind == "draft" && record.stage == SkillEvolutionStage::Validated);
    let report = SkillEvolutionStatusReport {
        status: SKILL_EVOLUTION_STATUS_READY,
        session_id: command.session_id.clone(),
        records,
        state_file: store.state_file().to_string_lossy().into_owned(),
        review_required,
    };
    render_status(format, &report)
}

pub(super) fn run_skill_evolution_promote(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &SkillEvolutionPromoteCommand,
) -> CliRunResult {
    if command.session_id.trim().is_empty() || command.skill_name.trim().is_empty() {
        return cli_error("session id 与 skill name 均不能为空");
    }
    let store = match SkillEvolutionStore::open_at(&context.axon_root) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let records = match store.records_for_session(&command.session_id) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let Some(validated) = records.iter().find(|record| {
        record.kind == "draft"
            && record.skill_name == command.skill_name
            && record.stage == SkillEvolutionStage::Validated
    }) else {
        return cli_error("未找到已验证的 Skill draft");
    };
    let material = match promote_validated_draft(
        &store,
        &command.skill_name,
        &context.axon_root.join("skills"),
    ) {
        Ok(material) => material,
        Err(error) => return cli_error(error),
    };
    let promoted = SkillEvolutionRecord {
        session_id: command.session_id.clone(),
        kind: "draft".to_owned(),
        skill_name: command.skill_name.clone(),
        risk: validated.risk.clone(),
        evidence_refs: validated.evidence_refs.clone(),
        stage: SkillEvolutionStage::Promoted,
        material_ref: Some(material.material_ref),
        revision: Some(material.revision),
        reason: "本地 promotion gate 已批准；下一会话扫描生效".to_owned(),
        recorded_at_ms: axon_time::unix_now_ms(),
    };
    if let Err(error) = store.record_current(&promoted) {
        return cli_error(error);
    }
    match format {
        OutputFormat::Json => json_result(&promoted),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "skill_evolution_promoted\t{}\t{}\n",
                promoted.session_id, promoted.skill_name
            ),
            stderr: String::new(),
        },
    }
}

fn render_status(format: OutputFormat, report: &SkillEvolutionStatusReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "{}\t{}\t{}",
                report.status,
                report.session_id,
                report.records.len()
            );
            for record in &report.records {
                let _ = writeln!(
                    stdout,
                    "{}\t{:?}\t{}\t{}\t{}",
                    record.kind,
                    record.stage,
                    record.risk,
                    record.skill_name,
                    record.evidence_refs.join(",")
                );
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}
