//! 持久收件箱的飞书渠道投递器。
//!
//! 产品定义：Task/Execution 完成消息与 Cron 投递只登记一条投递项，由各渠道消费者
//! 各自确认送达。飞书没有客户端 presence，因此由守护进程内的本投递器按渠道整批取出
//! 待投递条目，把正文写入 durable artifact 与出站队列后才确认送达：写入失败的条目
//! 保留在收件箱等待下一轮，隔天未读自动失效，绝不静默 ack，也绝不静默丢弃。

use std::path::Path;

use axon_session::ClientInboxEntry;
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

use super::remote_feishu_daemon_egress::{FeishuEgressInput, persist_egress};

/// 飞书渠道标识。
const FEISHU_CHANNEL: &str = "feishu";

/// 排空一批待投递的飞书收件箱条目，返回本轮确认送达的数量。
///
/// 只有在正文写入 durable artifact 与出站队列之后才确认送达；飞书出站队列自身带
/// 重试与 LRU 索引，因此确认送达等价于「消息已经进入有重试保证的持久队列」。
pub(crate) fn drain_feishu_completion_inbox<S, E, P, C, I>(
    host: &axon_daemon::DaemonHost<S, E, P, C, I>,
    axon_root: &Path,
    config_path: &Path,
    limit: u32,
) -> Result<u32, String>
where
    S: axon_session::RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: axon_daemon::ProcessIdentity + Clone + 'static,
{
    if limit == 0 {
        return Ok(0);
    }
    let target = match feishu_notification_target(config_path) {
        Ok(target) => target,
        Err(error) => {
            host.set_feishu_notification_chat_id(None)
                .map_err(|error| error.to_string())?;
            return Err(error);
        }
    };
    host.set_feishu_notification_chat_id(target.as_deref())
        .map_err(|error| error.to_string())?;
    let Some(target) = target else {
        return Ok(0);
    };
    let entries = host
        .dispatched_channel_inbox_for_destination(FEISHU_CHANNEL, &target, limit)
        .map_err(|error| format!("读取飞书投递收件箱失败：{error}"))?;
    let mut delivered = 0_u32;
    for entry in entries {
        if let Err(error) = deliver_entry(axon_root, &entry) {
            axon_logger::warning(&format!(
                "飞书完成消息投递失败，条目保留在收件箱等待下一轮: notification={} error={error}",
                entry.notification_id
            ));
            continue;
        }
        if let Err(error) = host.acknowledge_channel_delivery(&entry) {
            axon_logger::warning(&format!(
                "飞书完成消息已入队但确认送达失败，下一轮按同一 dispatch 幂等重投: notification={} error={error}",
                entry.notification_id
            ));
            continue;
        }
        delivered = delivered.saturating_add(1);
    }
    Ok(delivered)
}

/// 判断飞书渠道当前是否可投递。
///
/// 飞书未启用时直接跳过：此时登记到飞书渠道的投递项保持未送达，等渠道可用后补投，
/// 而不是在这里制造一次「假装已送达」的确认。
/// 把一条收件箱条目写入飞书 artifact 与 durable 出站队列。
fn deliver_entry(axon_root: &Path, entry: &ClientInboxEntry) -> Result<(), String> {
    persist_egress(axon_root, FeishuEgressInput::from_inbox_entry(entry))
}

fn feishu_notification_target(config_path: &Path) -> Result<Option<String>, String> {
    crate::run_commands::load_run_env_file(&crate::setup_commands::env_path_for_config(
        config_path,
    ))?;
    let loaded = axon_config::ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取 Feishu 投递配置失败：{error:?}"))?;
    let config = loaded.config.remote.feishu_websocket;
    Ok((config.enabled
        && config.enable_notification_sending
        && !config.notification_chat_id.trim().is_empty())
    .then(|| config.notification_chat_id.trim().to_owned()))
}
