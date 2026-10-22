use std::{path::Path, process::Stdio};

use axon_crypto::sha256_hex_prefix;
use axon_domain::{CronDeliveryAdapter, CronDeliveryTarget, CronJob, CronRunRecord};
use axon_process::non_interactive_command;

use super::{
    MEMORY_CONSOLIDATION_CRON_SCRIPT, SESSION_CLEANUP_CRON_SCRIPT,
    session_cleanup::SESSION_CLEANUP_DRY_RUN_CRON_SCRIPT,
};
use crate::{
    cron_commands::{run_session_cleanup, run_session_cleanup_once},
    run_commands::{execute_daemon_interaction, interaction_from_prompt},
    runtime_memory_consolidation::run_memory_consolidation_once,
    usage_statistics::{
        PRICING_SYNC_CRON_SCRIPT, USAGE_ROLLUP_CRON_SCRIPT, run_pricing_sync_once,
        run_usage_rollup_once,
    },
};

pub(crate) const CRON_AGENT_DESTINATION_PREFIX: &str = "cron-";
pub(crate) const CRON_AGENT_CHANNEL: &str = "desktop";
const CRON_POWERSHELL_UTF8_PREAMBLE: &str = "$utf8 = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = $utf8; $OutputEncoding = $utf8;";
const CRON_AGENT_POLICY: &str = "这是用户级 Agent Cron 定时任务。必须遵守：不得创建、修改、暂停、恢复或删除任何 cron 任务；不得请求交互式澄清；配置、凭证或投递目标缺失时必须失败闭环并说明原因。";

pub(crate) trait CronExecutionAdapter: Send + Sync {
    fn axon_root(&self) -> &Path;

    fn config_path(&self) -> &Path;

    fn delivery_target(
        &self,
        recipient_id: String,
        thread_id: Option<String>,
    ) -> CronDeliveryTarget;

    fn execute_agent(
        &self,
        job: &CronJob,
        prompt: &str,
        now_ms: u64,
    ) -> Result<CronExecutionReceipt, String>;

    fn enqueue_reply(&self, job: &CronJob, body: &str, now_ms: u64) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CronExecutionReceipt {
    pub(crate) agent_executed: bool,
    pub(crate) delivery_enqueued: bool,
    pub(crate) stdout_empty: bool,
    pub(crate) dispatch_id: Option<String>,
}

pub(crate) fn local_cron_session_key(job: &CronJob) -> String {
    format!("{CRON_AGENT_DESTINATION_PREFIX}{}", job.job_id)
}

pub(crate) fn execute_local_cron_agent(
    config_path: &Path,
    axon_root: &Path,
    job: &CronJob,
    prompt: &str,
) -> Result<String, String> {
    let cwd = job.workdir.as_deref().map(Path::new).unwrap_or(axon_root);
    let interaction = interaction_from_prompt(
        CRON_AGENT_CHANNEL,
        &local_cron_session_key(job),
        prompt,
        cwd,
        axon_time::unix_now_ms(),
    )?;
    let output =
        execute_daemon_interaction(interaction, cwd, Some(config_path), None, &|| false, None)?;
    Ok(output.response)
}

pub(crate) fn execute_cron_job(
    adapter: &dyn CronExecutionAdapter,
    job: &CronJob,
    started_at_ms: u64,
) -> CronRunRecord {
    let result = if job.no_agent {
        execute_no_agent_job(adapter, job, started_at_ms)
    } else {
        adapter.execute_agent(job, &cron_agent_prompt(job), started_at_ms)
    };
    let finished_at_ms = axon_time::unix_now_ms().max(started_at_ms);
    match result {
        Ok(receipt) => CronRunRecord {
            schema_version: job.schema_version,
            run_id: run_id(&job.job_id, started_at_ms),
            job_id: job.job_id.clone(),
            job_revision: job.revision,
            started_at_ms,
            finished_at_ms,
            success: true,
            agent_executed: receipt.agent_executed,
            delivery_enqueued: receipt.delivery_enqueued,
            stdout_empty: receipt.stdout_empty,
            dispatch_id: receipt.dispatch_id,
            error_summary: None,
        },
        Err(error) => CronRunRecord {
            schema_version: job.schema_version,
            run_id: run_id(&job.job_id, started_at_ms),
            job_id: job.job_id.clone(),
            job_revision: job.revision,
            started_at_ms,
            finished_at_ms,
            success: false,
            agent_executed: !job.no_agent,
            delivery_enqueued: false,
            stdout_empty: false,
            dispatch_id: None,
            error_summary: Some(error),
        },
    }
}

fn execute_no_agent_job(
    adapter: &dyn CronExecutionAdapter,
    job: &CronJob,
    now_ms: u64,
) -> Result<CronExecutionReceipt, String> {
    let script = job
        .script
        .as_deref()
        .ok_or_else(|| "no-agent cron 缺少 script".to_owned())?;
    match script {
        MEMORY_CONSOLIDATION_CRON_SCRIPT => {
            run_memory_consolidation_once(adapter.config_path())?;
            Ok(empty_no_agent_receipt())
        }
        USAGE_ROLLUP_CRON_SCRIPT => {
            run_usage_rollup_once(adapter.config_path())?;
            Ok(empty_no_agent_receipt())
        }
        PRICING_SYNC_CRON_SCRIPT => {
            run_pricing_sync_once(adapter.config_path())?;
            Ok(empty_no_agent_receipt())
        }
        SESSION_CLEANUP_CRON_SCRIPT => {
            let report = run_session_cleanup_once(adapter.config_path())?;
            cleanup_receipt(adapter, job, now_ms, &report)
        }
        SESSION_CLEANUP_DRY_RUN_CRON_SCRIPT => {
            let report = run_session_cleanup(adapter.config_path(), true)?;
            cleanup_receipt(adapter, job, now_ms, &report)
        }
        _ => execute_shell_job(adapter, job, script, now_ms),
    }
}

fn cleanup_receipt(
    adapter: &dyn CronExecutionAdapter,
    job: &CronJob,
    now_ms: u64,
    report: &super::session_cleanup::SessionCleanupReport,
) -> Result<CronExecutionReceipt, String> {
    let body = serde_json::to_string(report)
        .map_err(|error| format!("序列化 Cleanup 报告失败：{error}"))?;
    if job.delivery.adapter == CronDeliveryAdapter::None {
        return Ok(CronExecutionReceipt {
            agent_executed: false,
            delivery_enqueued: false,
            stdout_empty: false,
            dispatch_id: None,
        });
    }
    let dispatch_id = adapter.enqueue_reply(job, &body, now_ms)?;
    Ok(CronExecutionReceipt {
        agent_executed: false,
        delivery_enqueued: true,
        stdout_empty: false,
        dispatch_id: Some(dispatch_id),
    })
}

fn execute_shell_job(
    adapter: &dyn CronExecutionAdapter,
    job: &CronJob,
    script: &str,
    now_ms: u64,
) -> Result<CronExecutionReceipt, String> {
    let output = script_command(script, job.workdir.as_deref().map(Path::new))?;
    if !output.status.success() {
        let stderr = decode_script_output(output.stderr, "stderr")?;
        return Err(format!("no-agent cron 脚本执行失败：{stderr}"));
    }
    let stdout = decode_script_output(output.stdout, "stdout")?;
    let body = stdout.trim().to_owned();
    if body.is_empty() {
        return Ok(CronExecutionReceipt {
            agent_executed: false,
            delivery_enqueued: false,
            stdout_empty: true,
            dispatch_id: None,
        });
    }
    if job.delivery.adapter == CronDeliveryAdapter::None {
        return Ok(CronExecutionReceipt {
            agent_executed: false,
            delivery_enqueued: false,
            stdout_empty: false,
            dispatch_id: None,
        });
    }
    let dispatch_id = adapter.enqueue_reply(job, &body, now_ms)?;
    Ok(CronExecutionReceipt {
        agent_executed: false,
        delivery_enqueued: true,
        stdout_empty: false,
        dispatch_id: Some(dispatch_id),
    })
}

fn empty_no_agent_receipt() -> CronExecutionReceipt {
    CronExecutionReceipt {
        agent_executed: false,
        delivery_enqueued: false,
        stdout_empty: true,
        dispatch_id: None,
    }
}

fn script_command(script: &str, cwd: Option<&Path>) -> Result<std::process::Output, String> {
    let mut command = if cfg!(windows) {
        let mut command = non_interactive_command("pwsh");
        let utf8_script = format!("{CRON_POWERSHELL_UTF8_PREAMBLE}\n{script}");
        command.args(["-NoProfile", "-Command", &utf8_script]);
        command
    } else {
        let mut command = non_interactive_command("sh");
        command.args(["-c", script]);
        command
    };
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("no-agent cron 启动脚本失败：{error}"))
}

fn decode_script_output(bytes: Vec<u8>, stream: &str) -> Result<String, String> {
    String::from_utf8(bytes)
        .map_err(|_| format!("no-agent cron {stream} 不是有效 UTF-8，拒绝投递乱码"))
}

fn cron_agent_prompt(job: &CronJob) -> String {
    let skills = if job.skills.is_empty() {
        "无显式 skills".to_owned()
    } else {
        job.skills.join(", ")
    };
    format!(
        "{CRON_AGENT_POLICY}\n\n任务名称：{}\n任务 ID：{}\n计划表达式：{}\n显式 skills：{}\n\n任务正文：\n{}",
        job.name, job.job_id, job.schedule, skills, job.prompt
    )
}

fn run_id(job_id: &str, started_at_ms: u64) -> String {
    let source = format!("{job_id}:{started_at_ms}");
    format!("cron-run-{}", sha256_hex_prefix(source.as_bytes(), 16))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axon_domain::{CronDeliveryAdapter, CronJobInput};

    use super::*;

    struct RecordingAdapter {
        root: std::path::PathBuf,
        replies: Mutex<Vec<String>>,
    }

    impl CronExecutionAdapter for RecordingAdapter {
        fn axon_root(&self) -> &Path {
            &self.root
        }

        fn config_path(&self) -> &Path {
            &self.root
        }

        fn delivery_target(
            &self,
            recipient_id: String,
            thread_id: Option<String>,
        ) -> CronDeliveryTarget {
            CronDeliveryTarget {
                adapter: CronDeliveryAdapter::Feishu,
                session_id: 7,
                recipient_id,
                thread_id,
            }
        }

        fn execute_agent(
            &self,
            _job: &CronJob,
            _prompt: &str,
            _now_ms: u64,
        ) -> Result<CronExecutionReceipt, String> {
            const ERROR_CHARS: usize = 3_000;

            Err("x".repeat(ERROR_CHARS))
        }

        fn enqueue_reply(
            &self,
            _job: &CronJob,
            body: &str,
            _now_ms: u64,
        ) -> Result<String, String> {
            self.replies
                .lock()
                .expect("回复锁应可用")
                .push(body.to_owned());
            Ok("dispatch-test".to_owned())
        }
    }

    #[test]
    fn agent_error_is_preserved_without_private_truncation() {
        const ERROR_CHARS: usize = 3_000;

        let adapter = RecordingAdapter {
            root: axon_test_support::test_temp_root(),
            replies: Mutex::new(Vec::new()),
        };
        let job = CronJob::new(
            CronJobInput {
                job_id: "cron-agent-error".to_owned(),
                name: "Agent 错误保留".to_owned(),
                schedule: "interval:1h".to_owned(),
                prompt: "执行任务".to_owned(),
                skills: Vec::new(),
                delivery: CronDeliveryTarget::none(),
                workdir: None,
                no_agent: false,
                script: None,
            },
            1,
            Some(2),
        )
        .expect("任务应有效");

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(!run.success);
        assert_eq!(
            run.error_summary.as_deref().map(str::len),
            Some(ERROR_CHARS)
        );
    }

    #[test]
    fn no_agent_output_crosses_generic_delivery_port_once() {
        let root = axon_test_support::test_temp_root();
        let adapter = RecordingAdapter {
            root,
            replies: Mutex::new(Vec::new()),
        };
        let script = if cfg!(windows) {
            "Write-Output '日报完成'"
        } else {
            "printf '%s\\n' '日报完成'"
        };
        let job = CronJob::new(
            CronJobInput {
                job_id: "cron-test".to_owned(),
                name: "测试".to_owned(),
                schedule: "interval:1h".to_owned(),
                prompt: String::new(),
                skills: Vec::new(),
                delivery: adapter.delivery_target("oc_test".to_owned(), None),
                workdir: None,
                no_agent: true,
                script: Some(script.to_owned()),
            },
            1,
            Some(2),
        )
        .expect("任务应有效");

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(run.success);
        assert!(run.delivery_enqueued);
        assert_eq!(
            adapter.replies.lock().expect("回复锁应可用").as_slice(),
            ["日报完成"]
        );
    }

    #[test]
    fn no_delivery_job_keeps_script_output_out_of_delivery_port() {
        let adapter = RecordingAdapter {
            root: axon_test_support::test_temp_root(),
            replies: Mutex::new(Vec::new()),
        };
        let script = if cfg!(windows) {
            "Write-Output '维护完成'"
        } else {
            "printf '%s\n' '维护完成'"
        };
        let job = CronJob::new(
            CronJobInput {
                job_id: "cron-silent".to_owned(),
                name: "静默维护".to_owned(),
                schedule: "interval:1h".to_owned(),
                prompt: String::new(),
                skills: Vec::new(),
                delivery: CronDeliveryTarget::none(),
                workdir: None,
                no_agent: true,
                script: Some(script.to_owned()),
            },
            1,
            Some(2),
        )
        .expect("无投递任务应有效");

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(run.success);
        assert!(!run.stdout_empty);
        assert!(!run.delivery_enqueued);
        assert!(run.dispatch_id.is_none());
        assert!(adapter.replies.lock().expect("回复锁应可用").is_empty());
    }
}
