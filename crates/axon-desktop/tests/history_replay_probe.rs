//! 已安装网关的只读历史回放验证；显式运行时要求本机环境就绪。

use std::time::Duration;

use axon_desktop::conversation::history;
use axon_desktop::conversation::wire::{ClientFrame, LANES_PATH, WS_PATH};
use axon_desktop::gateway;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue};

const FRAME_TIMEOUT: Duration = Duration::from_secs(10);
const SUBSCRIBE_ID: &str = "history-replay-probe";

fn historical_target() -> (String, String) {
    let payload = gateway::get_json_at(LANES_PATH).expect("真实网关必须可用");
    let lanes = payload["lanes"].as_array().expect("车道列表必须有效");
    for lane in lanes {
        let lane_id = lane["laneId"].as_str().expect("车道标识");
        let interactions = history::lane_interactions(lane_id, None, None).expect("读取历史交互");
        if let Some(interaction) = interactions.first() {
            return (
                lane["workspaceId"].as_str().expect("工作区标识").to_owned(),
                interaction.interaction_id.clone(),
            );
        }
    }
    panic!("只读验证要求至少一条已有历史的会话");
}

#[tokio::test]
#[ignore = "要求已安装新版网关及本机历史数据"]
async fn installed_gateway_explicitly_marks_history_as_replay() {
    let (workspace_id, interaction_id) = historical_target();
    let base = gateway::websocket_base_url().expect("网关地址");
    let token = gateway::api_token().expect("本机网关凭据");
    let mut request = format!("{base}{WS_PATH}")
        .into_client_request()
        .expect("连接请求");
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("凭据格式"),
    );
    let (mut socket, _) =
        tokio::time::timeout(FRAME_TIMEOUT, tokio_tungstenite::connect_async(request))
            .await
            .expect("连接不能超时")
            .expect("连接真实网关");
    let frame = ClientFrame::Subscribe {
        subscribe_id: SUBSCRIBE_ID.to_owned(),
        workspace_id,
        interaction_id: Some(interaction_id),
        session_id: None,
        resume_cursor: None,
    };
    socket
        .send(Message::Text(
            serde_json::to_string(&frame).expect("订阅帧").into(),
        ))
        .await
        .expect("发送订阅");
    loop {
        let incoming = tokio::time::timeout(FRAME_TIMEOUT, socket.next())
            .await
            .expect("历史帧不能超时")
            .expect("连接不能提前关闭")
            .expect("读取帧");
        let Message::Text(text) = incoming else {
            continue;
        };
        // 检查原始 JSON，避免客户端的默认值掩盖旧网关缺字段。
        let value: serde_json::Value = serde_json::from_str(&text).expect("有效 JSON 帧");
        if value["type"] == "event" {
            assert_eq!(value["subscribeId"].as_str(), Some(SUBSCRIBE_ID));
            assert_eq!(value["replay"].as_bool(), Some(true));
            break;
        }
        assert_ne!(value["type"], "error", "历史订阅不能失败");
    }
    socket.close(None).await.expect("关闭只读连接");
}
