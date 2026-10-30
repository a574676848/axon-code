//! 持久收件箱的飞书渠道投递器。
//!
//! 产品定义：完成消息只投递给任务自己声明的目标。飞书没有客户端 presence，因此由
//! 守护进程内的本投递器按渠道整批取出待投递条目，用**条目自带的 destination** 作为
//! chat_id 写入 durable artifact 与出站队列后才确认送达。
//!
//! 这里不再按全局通知目标过滤：每条投递项的目标在登记时就已经确定，按全局值二次
//! 过滤会让指向其它会话的条目永远取不出来，最终停在 `dispatched` 直到隔天过期。
//!
//! 但「渠道是否允许出站」仍然必须门禁：飞书未启用时若照常写 outbox 并确认送达，
//! 通知会被标记 Delivered、对应 Result 被结算为已读，而实际一条消息都没发出去，
//! 这正是「任何渠道都不允许静默 ack」要禁止的情形。

use std::path::Path;

use axon_session::ClientInboxEntry;
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

use super::remote_feishu_daemon_egress::{FeishuEgressInput, persist_egress};

/// 飞书渠道标识。
const FEISHU_CHANNEL: &str = axon_domain::FEISHU_CHANNEL;

/// 飞书通道是否允许出站。
///
/// 只认「通道是否启用」这一个开关：任务结果的目标由任务自己声明，用「是否发送
/// 后台通知」去门控它会让显式声明的目标永远发不出去，同时又不敢确认送达。
pub(crate) fn feishu_channel_enabled(config_path: &Path) -> bool {
    axon_config::ConfigLoader::load_from_path(config_path)
        .map(|loaded| loaded.config.remote.feishu_websocket.enabled)
        .unwrap_or(false)
}

/// 排空一批待投递的飞书收件箱条目，返回本轮确认送达的数量。
///
/// 只有在正文写入 durable artifact 与出站队列之后才确认送达；飞书出站队列自身带
/// 重试与 LRU 索引，因此确认送达等价于「消息已经进入有重试保证的持久队列」。
/// 通道未启用时直接返回 0 且不确认任何条目，让它们留在收件箱等通道可用。
pub(crate) fn drain_feishu_completion_inbox<S, E, P, C, I>(
    host: &axon_daemon::DaemonHost<S, E, P, C, I>,
    axon_root: &Path,
    allow_outbound: bool,
    limit: u32,
) -> Result<u32, String>
where
    S: axon_session::RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: axon_daemon::ProcessIdentity + Clone + 'static,
{
    if limit == 0 || !allow_outbound {
        return Ok(0);
    }
    let entries = host
        .dispatched_channel_inbox(FEISHU_CHANNEL, limit)
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

/// 把一条收件箱条目写入飞书 artifact 与 durable 出站队列。
///
/// 目标对话取自条目自身的 `destination`，因此同一轮 tick 可以同时服务多个不同会话。
fn deliver_entry(axon_root: &Path, entry: &ClientInboxEntry) -> Result<(), String> {
    persist_egress(axon_root, FeishuEgressInput::from_inbox_entry(entry))
}
