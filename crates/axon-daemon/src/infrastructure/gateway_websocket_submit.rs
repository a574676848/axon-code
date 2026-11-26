//! WS `submit` 帧的提交分支：durable 桥调用与拒绝原因分类。
//!
//! 从 socket 循环里独立出来有两个理由：一是 durable append 是同步 sqlite 写入（含 FULL
//! 同步刷盘），必须整体离开 runtime 线程；二是拒绝要按原因码分类回执并留审计行，
//! 这段口径必须与 HTTP 面共用同一套分类，不能各写一份。

use super::super::gateway_kernel::GatewayContext;
use super::gateway_websocket_event::WsEventRequest;
use super::gateway_websocket_frames::event_rejected_json;

/// 提交一帧事件，返回要回写给对端的载荷。
pub(super) async fn submit_response(context: &GatewayContext, request: WsEventRequest) -> String {
    // 事件身份在 move 进阻塞池之前取好：被拒时要靠它把审计行指回这一次提交。
    let event = request.into_event();
    let identity = event.audit_identity();
    let submit_context = context.clone();
    let result =
        tokio::task::spawn_blocking(move || submit_context.submit_external_event(event)).await;
    match result {
        Ok(Ok(receipt)) => {
            serde_json::json!({ "type": "accepted", "receipt": receipt }).to_string()
        }
        // 与 HTTP 面同一口径：被拒事件必须留下带原因的一行，只回泛化码等于让对端
        // 去猜是内容、附件还是时效问题。
        Ok(Err(error)) => {
            let (reason, message) = error.ingress_rejection();
            axon_logger::warning(&format!(
                "[gateway-ws] 事件被拒绝 {identity} reason={reason} 原因={message}"
            ));
            event_rejected_json(reason, message)
        }
        // 阻塞池任务 panic：同样是未 durable 入账，但原因在网关自身，与报文无关。
        Err(_) => {
            axon_logger::warning(&format!(
                "[gateway-ws] 事件被拒绝 {identity} reason=ingress_internal 原因=阻塞池任务未返回"
            ));
            event_rejected_json("ingress_internal", "网关内部处理失败，请重试")
        }
    }
}
