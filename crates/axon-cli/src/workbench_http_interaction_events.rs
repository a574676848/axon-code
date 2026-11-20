//! 交互历史帧路由：把一条 Interaction 的 durable 事件读成 HTTP 响应。
//!
//! 从 `workbench_http_adapter` 拆出：这一组的职责是把错误种类映射成状态码，
//! 与只读列表载荷的组装没有交集；独立成文后单文件硬限也不会被挤爆。

use std::path::Path;

use axon_daemon::infrastructure::gateway_replay::interaction_history_page;
use axon_daemon::{GatewayHttpResponse, GatewayKernelError};
use axon_domain::InteractionId;

use super::{MAX_INTERACTION_EVENTS_LIMIT, runtime_store};

/// 一条 Interaction 的历史帧页。
///
/// 正文一次取回，客户端读完历史后再从 `nextAfter` 起订阅实时尾：事件通道上因此
/// 只出现真正的增量，不再承担整段回放。
///
/// 错误必须按种类分档：标识非法是 400，记录不存在是 404，其余是 500。把三者
/// 都折成 404 会让桌面端把「正文读失败」翻译成「网关不含该接口，后台服务版本
/// 可能过旧」，指向完全错误的修复动作。
pub(super) fn interaction_events_payload(
    axon_root: &Path,
    interaction_id: &str,
    after: u64,
    limit: u32,
    tail: bool,
) -> GatewayHttpResponse {
    let limit = limit.clamp(1, MAX_INTERACTION_EVENTS_LIMIT);
    let Ok(id) = InteractionId::new(interaction_id) else {
        return GatewayHttpResponse::error(
            400,
            "invalid_interaction_id",
            "交互标识不能为空且长度必须在允许范围内",
            None,
        );
    };
    let page = match interaction_history_page(&runtime_store(axon_root), &id, after, limit, tail) {
        Ok(page) => page,
        Err(GatewayKernelError::NotFound(message)) => {
            return GatewayHttpResponse::error(404, "interaction_not_found", &message, None);
        }
        Err(error) => {
            return GatewayHttpResponse::error(
                500,
                "gateway_error",
                &format!("读取历史帧失败：{error}"),
                None,
            );
        }
    };
    let events: Vec<serde_json::Value> = page
        .events
        .iter()
        .map(|event| {
            serde_json::json!({
                "eventType": event.event_type,
                "payloadJson": event.payload_json,
                "cursor": event.cursor,
                "replay": event.replay,
            })
        })
        .collect();
    GatewayHttpResponse::json(
        200,
        &serde_json::json!({
            "interactionId": interaction_id,
            "events": events,
            "nextAfter": page.next_after,
            "hasMore": page.has_more,
            "tailSequence": page.tail_sequence,
            "tailCursor": page.tail_cursor,
            // 因正文归档已回收而被跳过的帧数：界面据此说明「这一轮的部分过程已回收」，
            // 而不是把缺帧读成「这一轮本来就没有过程」。
            "skippedArchivedFrames": page.skipped_archived_frames,
        }),
    )
}
