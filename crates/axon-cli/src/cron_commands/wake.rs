//! Cron 完成后唤醒目标的解析与分派。
//!
//! 产品定义：定时任务只在创建时显式声明目标时才产生投递。目标是复合车道的主 turn
//! 注入，会真实执行一轮，因此必须走 Daemon 的 Interaction 提交原语，不能退化成一次
//! 本地写文件。

use axon_domain::{CronJob, CronWakeTarget};

use super::execution::CronExecutionAdapter;

/// Cron 唤醒 Interaction 的来源 ID 前缀。
///
/// 不得以 `cron:` 开头：那个前缀属于投递项 ID 空间，混用会让同一轮回复被重复登记。
pub(crate) const CRON_WAKE_SOURCE_PREFIX: &str = axon_domain::CRON_WAKE_SOURCE_PREFIX;

/// 会话唤醒正文里内联的结果摘要上限，避免把大段正文灌进目标会话历史。
const WAKE_NOTICE_SUMMARY_MAX_CHARS: usize = 1_200;

/// 解析 `--wake` 参数。
///
/// 支持 `session:<channel>:<destination>[:<workspace>]`。会话目标只在第二层切分：
/// `destination` 不含冒号时，第三段就是目标所属 Workspace。
pub(crate) fn parse_wake(value: &str) -> Result<CronWakeTarget, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("--wake 目标不能为空".to_owned());
    }
    if let Some(rest) = trimmed.strip_prefix("session:") {
        let Some((channel, remainder)) = rest.split_once(':') else {
            return Err(WAKE_FORMAT_HINT.to_owned());
        };
        // 第三段是目标会话所属的 Workspace：车道身份由
        // (workspace, trust_scope, channel, destination) 派生，只给后两维无法
        // 区分跨项目的同名会话。省略时退回按任务执行目录反推。
        let (destination, workspace) = match remainder.split_once(':') {
            Some((destination, workspace)) => (destination, Some(workspace.trim().to_owned())),
            None => (remainder, None),
        };
        let target = CronWakeTarget::Session {
            channel: channel.trim().to_owned(),
            destination: destination.trim().to_owned(),
            workspace: workspace.filter(|value| !value.is_empty()),
        };
        target.validate()?;
        return Ok(target);
    }
    Err(WAKE_FORMAT_HINT.to_owned())
}

/// `--wake` 目标格式提示。
const WAKE_FORMAT_HINT: &str =
    "--wake 目标格式必须是 session:<channel>:<destination>[:<workspace>]";

/// 派发一次唤醒；没有声明目标时什么都不做。
///
/// 失败同样派发：任务已经跑完是事实，沉默会让用户无法判断它是没触发还是失败了。
pub(crate) fn dispatch_wake(
    adapter: &dyn CronExecutionAdapter,
    job: &CronJob,
    run_id: &str,
    body: &str,
    success: bool,
    now_ms: u64,
) -> Result<Option<String>, String> {
    let Some(wake) = job.wake.as_ref() else {
        return Ok(None);
    };
    let CronWakeTarget::Session {
        channel,
        destination,
        ..
    } = wake;
    let dispatch_id = adapter.wake_session(
        job,
        channel.trim(),
        destination.trim(),
        &turn_injection_body(job, run_id, body, success),
        now_ms,
    )?;
    Ok(Some(dispatch_id))
}

/// 主 turn 注入正文：指路提示加有界摘要。
///
/// 目标会话需要的是「哪个任务、哪一次运行、去哪里取事实」，而不是把大段结果灌进它的
/// 上下文；摘要只用于让人在会话里一眼看出发生了什么。
fn turn_injection_body(job: &CronJob, run_id: &str, body: &str, success: bool) -> String {
    let mut text = completion_notice(job, run_id);
    if !success {
        text.push_str("\n本次执行失败，失败原因见运行记录。");
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return text;
    }
    let excerpt = truncate_chars(trimmed, WAKE_NOTICE_SUMMARY_MAX_CHARS);
    text.push_str("\n\n结果摘要：\n");
    text.push_str(&excerpt);
    if excerpt.len() < trimmed.len() {
        text.push_str("\n（摘要已截断，完整内容请查询运行记录。）");
    }
    text
}

fn completion_notice(job: &CronJob, run_id: &str) -> String {
    format!(
        "定时任务「{}」已执行完成。\n任务 ID：{}\n运行 ID：{}",
        job.name, job.job_id, run_id
    )
}

/// 按字符数截断，避免在多字节边界切开。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_target_parses_supported_forms_only() {
        assert_eq!(
            parse_wake(" session:desktop:workbench-1 ").expect("会话目标应可解析"),
            CronWakeTarget::Session {
                channel: "desktop".to_owned(),
                destination: "workbench-1".to_owned(),
                workspace: None,
            }
        );
        assert_eq!(
            parse_wake("session:desktop:workbench-1:workspace-9d4b642b")
                .expect("会话目标应可携带 Workspace"),
            CronWakeTarget::Session {
                channel: "desktop".to_owned(),
                destination: "workbench-1".to_owned(),
                workspace: Some("workspace-9d4b642b".to_owned()),
            }
        );
        assert!(parse_wake("webhook:oc_123").is_err());
        assert!(parse_wake("session:desktop").is_err());
        assert!(parse_wake("tui:terminal-1").is_err());
        assert!(parse_wake("session:tui:terminal-1").is_err());
        assert!(parse_wake("").is_err());
    }

    #[test]
    fn turn_injection_body_truncates_long_results() {
        let job = test_job();
        let long = "结".repeat(WAKE_NOTICE_SUMMARY_MAX_CHARS + 10);
        let body = turn_injection_body(&job, "cron-run-1", &long, true);

        assert!(body.contains("运行 ID：cron-run-1"));
        assert!(body.contains("摘要已截断"));
        assert!(body.chars().count() < long.chars().count() + 200);
    }

    #[test]
    fn failed_run_marks_reason_in_turn_injection() {
        let body = turn_injection_body(&test_job(), "cron-run-2", "", false);
        assert!(body.contains("本次执行失败"));
    }

    fn test_job() -> CronJob {
        CronJob::new(
            axon_domain::CronJobInput {
                job_id: "cron-wake".to_owned(),
                name: "唤醒测试".to_owned(),
                schedule: "interval:1h".to_owned(),
                prompt: "执行".to_owned(),
                skills: Vec::new(),
                wake: Some(CronWakeTarget::Session {
                    channel: "desktop".to_owned(),
                    destination: "workbench-1".to_owned(),
                    workspace: None,
                }),
                workdir: None,
                no_agent: false,
                script: None,
            },
            1,
            Some(2),
        )
        .expect("任务应有效")
    }
}
