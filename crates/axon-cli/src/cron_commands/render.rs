use std::fmt::Write as _;

use crate::{CliRunResult, OutputFormat, management_commands::json_result};

use super::application::CronReport;

pub(super) fn render_cron(format: OutputFormat, report: &CronReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = format!(
                "Axon cron: {}\n- jobs: total={} listed={}\n- runs: executed={} success={} failed={}\n- delivery_executed: {}\n",
                report.status,
                report.total_jobs,
                report.listed_jobs,
                report.executed_jobs,
                report.success_runs,
                report.failed_runs,
                report.delivery_executed
            );
            for job in &report.jobs {
                let _ = writeln!(
                    stdout,
                    "{}\t{}\t{}\tnext={:?}\tno_agent={}\twake={}",
                    job.job_id,
                    job.status,
                    job.schedule,
                    job.next_run_at_ms,
                    job.no_agent,
                    job.wake.as_deref().unwrap_or("none")
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
