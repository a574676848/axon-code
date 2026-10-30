//! Cron 完成后唤醒目标的解析与分派。
//!
//! 产品定义：定时任务只在创建时显式声明目标时才产生投递。两类目标的动作不同，成本
//! 也不同：飞书是正文直投，不产生 Agent 回合；会话是主 turn 注入，会真实执行一轮。
//! 因此两者必须走不同的 Daemon 原语，不能合并成同一个「投递」动作。

use axon_domain::{CronJob, CronWakeTarget, FEISHU_CHANNEL};

use super::execution::CronExecutionAdapter;

/// Cron 唤醒 Interaction 的来源 ID 前缀。
///
/// 不得以 `cron:` 开头：那个前缀属于投递项 ID 空间，混用会让同一轮回复被重复登记。
pub(crate) const CRON_WAKE_SOURCE_PREFIX: &str = axon_domain::CRON_WAKE_SOURCE_PREFIX;

/// 会话唤醒正文里内联的结果摘要上限，避免把大段正文灌进目标会话历史。
const WAKE_NOTICE_SUMMARY_MAX_CHARS: usize = 1_200;

/// 解析 `--wake` 参数。
///
/// 支持 `feishu:<chat_id>` 与 `session:<channel>:<destination>` 两种形态。只按第一个
/// 冒号切分，渠道内目标本身含冒号时保持完整。
pub(crate) fn parse_wake(value: &str) -> Result<CronWakeTarget, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("--wake 目标不能为空".to_owned());
    }
    if let Some(chat_id) = trimmed.strip_prefix("feishu:") {
        let target = CronWakeTarget::Feishu(chat_id.trim().to_owned());
        target.validate()?;
        return Ok(target);
    }
    if let Some(rest) = trimmed.strip_prefix("session:") {
        let Some((channel, destination)) = rest.split_once(':') else {
            return Err("--wake session 目标格式必须是 session:<channel>:<destination>".to_owned());
        };
        let target = CronWakeTarget::Session {
            channel: channel.trim().to_owned(),
            destination: destination.trim().to_owned(),
        };
        target.validate()?;
        return Ok(target);
    }
    Err("--wake 目标格式必须是 feishu:<chat_id> 或 session:<channel>:<destination>".to_owned())
}

/// 已知的旧投递参数中文迁移提示。
pub(crate) const LEGACY_DELIVERY_ARGS_HINT: &str = "--feishu-chat-id / --feishu-thread-id 已移除：请改用 --wake feishu:<chat_id>；会话唤醒用 --wake session:<channel>:<destination>";

/// 全局通知配置里的飞书 chat_id。
///
/// 这是用户级全局配置，作为「用户没有特意指明通知目标时的默认选择」提供给调用方；
/// 是否采用由 Agent 在创建任务时决定，后端不做隐式兜底。配置缺失时返回 `None`。
pub(crate) fn suggested_feishu_chat_id(config_path: &std::path::Path) -> Option<String> {
    let loaded = axon_config::ConfigLoader::load_from_path(config_path).ok()?;
    let chat_id = loaded.config.remote.feishu_websocket.notification_chat_id;
    let chat_id = chat_id.trim();
    // 提示值只回答「本机配了哪个群」；能不能发出去不在这里判断。
    (!chat_id.is_empty()).then(|| chat_id.to_owned())
}

/// 派发一次唤醒；没有声明目标时什么都不做。
///
/// 空正文的处理按「是否有事实要告知」决定：成功的飞书直投没有正文就不必发消息；
/// 失败与会话唤醒都必须告知，否则用户无法判断任务是没有触发还是执行失败。
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
    let has_body = !body.trim().is_empty();
    if !has_body && success && !wake.wakes_turn() {
        return Ok(None);
    }
    let dispatch_id = match wake {
        CronWakeTarget::Feishu(chat_id) => {
            let chat_id = chat_id.trim();
            if chat_id.is_empty() {
                return Err("飞书唤醒目标的 chat_id 不能为空".to_owned());
            }
            adapter.enqueue_reply(
                job,
                FEISHU_CHANNEL,
                chat_id,
                &wake_body(job, run_id, body, success),
                now_ms,
            )?
        }
        CronWakeTarget::Session {
            channel,
            destination,
        } => adapter.wake_session(
            job,
            channel.trim(),
            destination.trim(),
            &turn_injection_body(job, run_id, body, success),
            now_ms,
        )?,
    };
    Ok(Some(dispatch_id))
}

/// 飞书直投正文：完整结果本身就是交付内容，为空时退化成完成提示。
fn wake_body(job: &CronJob, run_id: &str, body: &str, success: bool) -> String {
    let trimmed = body.trim();
    if !trimmed.is_empty() {
        return trimmed.to_owned();
    }
    let mut text = completion_notice(job, run_id);
    if !success {
        text.push_str("\n本次执行失败，失败原因见 axon cron list 的运行记录。");
    }
    text
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
            parse_wake("feishu:oc_123").expect("飞书目标应可解析"),
            CronWakeTarget::Feishu("oc_123".to_owned())
        );
        assert_eq!(
            parse_wake(" session:desktop:workbench-1 ").expect("会话目标应可解析"),
            CronWakeTarget::Session {
                channel: "desktop".to_owned(),
                destination: "workbench-1".to_owned(),
            }
        );
        assert!(parse_wake("feishu:").is_err());
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
                wake: Some(CronWakeTarget::Feishu("oc_test".to_owned())),
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
