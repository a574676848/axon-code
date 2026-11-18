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
/// 只支持 `session:<session-id>`：目标是已经存在的一条会话，标识即会话身份，
/// 不再接受「渠道 + 车道标识」派生。
pub(crate) fn parse_wake(value: &str) -> Result<CronWakeTarget, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("--wake 目标不能为空".to_owned());
    }
    if let Some(rest) = trimmed.strip_prefix("session:") {
        let session_id = rest.trim();
        // 会话标识是一个整体，内部不再分段：带冒号的输入只可能是旧的
        // 「渠道 + 车道标识」形式，直接拒绝而不是把它当成标识本身。
        if session_id.contains(':') {
            return Err(WAKE_FORMAT_HINT.to_owned());
        }
        let target = CronWakeTarget::Session {
            session_id: session_id.to_owned(),
        };
        target.validate()?;
        return Ok(target);
    }
    Err(WAKE_FORMAT_HINT.to_owned())
}

/// `--wake` 目标格式提示。
const WAKE_FORMAT_HINT: &str = "--wake 目标格式必须是 session:<session-id>（会话标识不含冒号）";

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
    let CronWakeTarget::Session { session_id } = wake;
    let dispatch_id = adapter.wake_session(
        job,
        session_id.trim(),
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
            parse_wake(" session:session-01ARZ3NDEKTSV4RRFFQ69G5FAV ").expect("会话目标应可解析"),
            CronWakeTarget::Session {
                session_id: "session-01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            }
        );
        assert!(parse_wake("webhook:oc_123").is_err());
        assert!(parse_wake("session:").is_err());
        assert!(
            parse_wake("session:desktop:workbench-1").is_err(),
            "渠道加车道标识的旧形式必须被拒绝"
        );
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
                    session_id: "session-01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
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
