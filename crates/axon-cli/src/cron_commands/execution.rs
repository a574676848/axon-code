use std::{path::Path, process::Stdio};

use axon_crypto::sha256_hex_prefix;
use axon_domain::CronJob;
use axon_process::non_interactive_command;

use super::{
    MEMORY_CONSOLIDATION_CRON_SCRIPT, SESSION_CLEANUP_CRON_SCRIPT,
    session_cleanup::SESSION_CLEANUP_DRY_RUN_CRON_SCRIPT, wake::dispatch_wake,
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

pub(crate) const CRON_AGENT_DESTINATION_PREFIX: &str = axon_domain::CRON_LANE_DESTINATION_PREFIX;
pub(crate) const CRON_AGENT_CHANNEL: &str = axon_domain::desktop_session_channel();
const CRON_POWERSHELL_UTF8_PREAMBLE: &str = "$utf8 = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = $utf8; $OutputEncoding = $utf8;";
const CRON_AGENT_POLICY: &str = "这是用户级 Agent Cron 定时任务。必须遵守：不得创建、修改、暂停、恢复或删除任何 cron 任务；不得请求交互式澄清；配置、凭证或投递目标缺失时必须失败闭环并说明原因。";

/// Cron 执行所需的外部能力。
///
/// 结果投递不是 adapter 的职责：执行层只产出正文，由 [`dispatch_wake`] 按任务的显式
/// 唤醒目标选择原语。这样新增目标类型时不需要改动每一个 adapter。
pub(crate) trait CronExecutionAdapter: Send + Sync {
    fn axon_root(&self) -> &Path;

    fn config_path(&self) -> &Path;

    /// 执行 Agent 任务并返回可投递正文。
    fn execute_agent(&self, job: &CronJob, prompt: &str) -> Result<String, String>;

    /// 把正文登记到持久收件箱并定向投递到指定渠道目标。
    fn enqueue_reply(
        &self,
        job: &CronJob,
        channel: &str,
        destination: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String>;

    /// 向目标复合车道提交一次主 turn 注入。
    fn wake_session(
        &self,
        job: &CronJob,
        channel: &str,
        destination: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<String, String>;
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

/// 执行一次 Cron 并按任务的显式唤醒目标派发结果。
///
/// 失败同样派发：任务已经跑完是事实，沉默会让用户无法判断它是没触发还是失败了。
pub(crate) fn execute_cron_job(
    adapter: &dyn CronExecutionAdapter,
    job: &CronJob,
    started_at_ms: u64,
) -> axon_domain::CronRunRecord {
    let outcome = if job.no_agent {
        execute_no_agent_job(adapter, job)
    } else {
        adapter.execute_agent(job, &cron_agent_prompt(job))
    };
    let finished_at_ms = axon_time::unix_now_ms().max(started_at_ms);
    let run_id = run_id(&job.job_id, started_at_ms);
    let (body, execution_error) = match outcome {
        Ok(body) => (body, None),
        Err(error) => (String::new(), Some(error)),
    };
    let dispatch = dispatch_wake(
        adapter,
        job,
        &run_id,
        &body,
        execution_error.is_none(),
        finished_at_ms,
    );
    let mut dispatch_id = None;
    let dispatch_error = match dispatch {
        Ok(id) => {
            dispatch_id = id;
            None
        }
        Err(error) => Some(error),
    };
    let (success, error_summary) = match (execution_error, dispatch_error) {
        (None, None) => (true, None),
        (None, Some(error)) => (false, Some(format!("唤醒派发失败：{error}"))),
        // 两者同时失败时必须都留下：只报执行错误会让运维以为失败告警已经发出去了。
        (Some(error), Some(dispatch)) => {
            (false, Some(format!("{error}\n唤醒派发失败：{dispatch}")))
        }
        (Some(error), None) => (false, Some(error)),
    };
    axon_domain::CronRunRecord {
        schema_version: job.schema_version,
        run_id,
        job_id: job.job_id.clone(),
        job_revision: job.revision,
        started_at_ms,
        finished_at_ms,
        success,
        agent_executed: !job.no_agent,
        delivery_enqueued: dispatch_id.is_some(),
        stdout_empty: body.trim().is_empty(),
        dispatch_id,
        error_summary,
    }
}

fn execute_no_agent_job(
    adapter: &dyn CronExecutionAdapter,
    job: &CronJob,
) -> Result<String, String> {
    let script = job
        .script
        .as_deref()
        .ok_or_else(|| "no-agent cron 缺少 script".to_owned())?;
    match script {
        MEMORY_CONSOLIDATION_CRON_SCRIPT => {
            run_memory_consolidation_once(adapter.config_path())?;
            Ok(String::new())
        }
        USAGE_ROLLUP_CRON_SCRIPT => {
            run_usage_rollup_once(adapter.config_path())?;
            Ok(String::new())
        }
        PRICING_SYNC_CRON_SCRIPT => {
            run_pricing_sync_once(adapter.config_path())?;
            Ok(String::new())
        }
        SESSION_CLEANUP_CRON_SCRIPT => {
            let report = run_session_cleanup_once(adapter.config_path())?;
            serde_json::to_string(&report)
                .map_err(|error| format!("序列化 Cleanup 报告失败：{error}"))
        }
        SESSION_CLEANUP_DRY_RUN_CRON_SCRIPT => {
            let report = run_session_cleanup(adapter.config_path(), true)?;
            serde_json::to_string(&report)
                .map_err(|error| format!("序列化 Cleanup 报告失败：{error}"))
        }
        _ => execute_shell_job(job, script),
    }
}

fn execute_shell_job(job: &CronJob, script: &str) -> Result<String, String> {
    let output = script_command(script, job.workdir.as_deref().map(Path::new))?;
    if !output.status.success() {
        let stderr = decode_script_output(output.stderr, "stderr")?;
        return Err(format!("no-agent cron 脚本执行失败：{stderr}"));
    }
    let stdout = decode_script_output(output.stdout, "stdout")?;
    Ok(stdout.trim().to_owned())
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

pub(super) fn run_id(job_id: &str, started_at_ms: u64) -> String {
    let source = format!("{job_id}:{started_at_ms}");
    format!("cron-run-{}", sha256_hex_prefix(source.as_bytes(), 16))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axon_domain::{CronJobInput, CronWakeTarget};

    use super::*;

    /// 记录派发动作的测试 adapter；不触达任何外部系统。
    struct RecordingAdapter {
        root: std::path::PathBuf,
        agent_body: Result<String, String>,
        replies: Mutex<Vec<(String, String, String)>>,
        turns: Mutex<Vec<(String, String, String)>>,
    }

    impl RecordingAdapter {
        fn new(agent_body: Result<String, String>) -> Self {
            Self {
                root: axon_test_support::test_temp_root(),
                agent_body,
                replies: Mutex::new(Vec::new()),
                turns: Mutex::new(Vec::new()),
            }
        }
    }

    impl CronExecutionAdapter for RecordingAdapter {
        fn axon_root(&self) -> &Path {
            &self.root
        }

        fn config_path(&self) -> &Path {
            &self.root
        }

        fn execute_agent(&self, _job: &CronJob, _prompt: &str) -> Result<String, String> {
            self.agent_body.clone()
        }

        fn enqueue_reply(
            &self,
            _job: &CronJob,
            channel: &str,
            destination: &str,
            body: &str,
            _now_ms: u64,
        ) -> Result<String, String> {
            self.replies.lock().expect("回复锁应可用").push((
                channel.to_owned(),
                destination.to_owned(),
                body.to_owned(),
            ));
            Ok("dispatch-test".to_owned())
        }

        fn wake_session(
            &self,
            _job: &CronJob,
            channel: &str,
            destination: &str,
            body: &str,
            _now_ms: u64,
        ) -> Result<String, String> {
            self.turns.lock().expect("回合锁应可用").push((
                channel.to_owned(),
                destination.to_owned(),
                body.to_owned(),
            ));
            Ok("interaction-test".to_owned())
        }
    }

    #[test]
    fn agent_error_is_preserved_without_private_truncation() {
        const ERROR_CHARS: usize = 3_000;

        let adapter = RecordingAdapter::new(Err("x".repeat(ERROR_CHARS)));
        let job = job_with_wake("cron-agent-error", None, false);

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(!run.success);
        assert_eq!(
            run.error_summary.as_deref().map(str::len),
            Some(ERROR_CHARS)
        );
    }

    #[test]
    fn silent_job_never_dispatches_without_declared_wake() {
        let adapter = RecordingAdapter::new(Ok("维护完成".to_owned()));
        let job = job_with_wake("cron-silent", None, false);

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(run.success);
        assert!(!run.delivery_enqueued);
        assert!(run.dispatch_id.is_none());
        assert!(adapter.replies.lock().expect("回复锁应可用").is_empty());
        assert!(adapter.turns.lock().expect("回合锁应可用").is_empty());
    }

    #[test]
    fn feishu_wake_delivers_body_to_declared_chat() {
        let adapter = RecordingAdapter::new(Ok("日报完成".to_owned()));
        let job = job_with_wake(
            "cron-feishu",
            Some(CronWakeTarget::Feishu("oc_test".to_owned())),
            false,
        );

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(run.success);
        assert!(run.delivery_enqueued);
        assert_eq!(run.dispatch_id.as_deref(), Some("dispatch-test"));
        assert_eq!(
            adapter.replies.lock().expect("回复锁应可用").as_slice(),
            [(
                "feishu".to_owned(),
                "oc_test".to_owned(),
                "日报完成".to_owned()
            )]
        );
        assert!(adapter.turns.lock().expect("回合锁应可用").is_empty());
    }

    #[test]
    fn session_wake_injects_turn_into_declared_lane() {
        let adapter = RecordingAdapter::new(Ok("已生成报告".to_owned()));
        let job = job_with_wake(
            "cron-session",
            Some(CronWakeTarget::Session {
                channel: "desktop".to_owned(),
                destination: "workbench-1".to_owned(),
            }),
            false,
        );

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(run.success);
        assert_eq!(run.dispatch_id.as_deref(), Some("interaction-test"));
        let turns = adapter.turns.lock().expect("回合锁应可用");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].0, "desktop");
        assert_eq!(turns[0].1, "workbench-1");
        assert!(turns[0].2.contains("已执行完成"));
        assert!(turns[0].2.contains("结果摘要"));
        assert!(adapter.replies.lock().expect("回复锁应可用").is_empty());
    }

    #[test]
    fn failed_run_still_wakes_declared_target() {
        let adapter = RecordingAdapter::new(Err("凭证缺失".to_owned()));
        let job = job_with_wake(
            "cron-failed-wake",
            Some(CronWakeTarget::Feishu("oc_test".to_owned())),
            false,
        );

        let run = execute_cron_job(&adapter, &job, 2);

        assert!(!run.success);
        assert_eq!(run.error_summary.as_deref(), Some("凭证缺失"));
        assert!(run.delivery_enqueued);
        assert_eq!(
            adapter.replies.lock().expect("回复锁应可用").len(),
            1,
            "失败也必须告知，否则用户无法判断任务是否触发"
        );
    }

    fn job_with_wake(job_id: &str, wake: Option<CronWakeTarget>, no_agent: bool) -> CronJob {
        CronJob::new(
            CronJobInput {
                job_id: job_id.to_owned(),
                name: "测试".to_owned(),
                schedule: "interval:1h".to_owned(),
                prompt: if no_agent {
                    String::new()
                } else {
                    "执行任务".to_owned()
                },
                skills: Vec::new(),
                wake,
                workdir: None,
                no_agent,
                script: None,
            },
            1,
            Some(2),
        )
        .expect("任务应有效")
    }
}
