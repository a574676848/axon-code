use std::time::Duration;

use axon_agent_runtime::RunCancelToken;
use axon_domain::{AxonError, AxonResult};
use tokio::time::Instant as TokioInstant;
use tokio_tungstenite::tungstenite::Message;

use super::provider_response_chain::WebSocketResponseState;
use super::provider_websocket_pump::ProviderWebSocketPump;
use super::{
    ProviderStreamingHttpRequestPlan, WebSocketReadResult, observe_response_event,
    observe_websocket_rate_limit,
};

pub(super) async fn read_websocket_stream<F, G>(
    socket: &mut ProviderWebSocketPump,
    request_plan: &ProviderStreamingHttpRequestPlan,
    timeout: Duration,
    cancel_token: &RunCancelToken,
    on_frame: &mut F,
    on_progress: &mut G,
) -> AxonResult<WebSocketReadResult>
where
    F: FnMut(&str),
    G: FnMut(),
{
    let mut received_response_frame = false;
    let mut response_state = WebSocketResponseState::default();
    let mut closed_before_first_response_frame = false;
    let mut close_detail = None;
    let mut idle_sleep = Box::pin(tokio::time::sleep(timeout));
    let mut progress_sleep = Box::pin(tokio::time::sleep(Duration::from_millis(250)));
    loop {
        tokio::select! {
            message = socket.next() => {
                let Some(message) = message else {
                    closed_before_first_response_frame = !received_response_frame;
                    break;
                };
                match message {
                    Ok(Message::Text(text)) => {
                        received_response_frame = true;
                        on_frame(text.as_ref());
                        observe_websocket_rate_limit(request_plan, text.as_ref());
                        observe_response_event(&mut response_state, &text);
                        if response_state.completed.is_some() || response_state.failure.is_some() {
                            break;
                        }
                        idle_sleep.as_mut().reset(TokioInstant::now() + timeout);
                    }
                    Ok(Message::Binary(bytes)) => {
                        received_response_frame = true;
                        let text = String::from_utf8_lossy(&bytes);
                        on_frame(text.as_ref());
                        observe_websocket_rate_limit(request_plan, text.as_ref());
                        observe_response_event(&mut response_state, &text);
                        if response_state.completed.is_some() || response_state.failure.is_some() {
                            break;
                        }
                        idle_sleep.as_mut().reset(TokioInstant::now() + timeout);
                    }
                    Ok(Message::Close(frame)) => {
                        close_detail = frame.map(|frame| {
                            format!(
                                "websocket close：code={}；reason={}",
                                frame.code, frame.reason
                            )
                        });
                        closed_before_first_response_frame = !received_response_frame;
                        break;
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                    Err(_) if received_response_frame => break,
                    Err(_) => {
                        return Err(AxonError::provider_failure(
                            "provider websocket streaming 响应读取失败",
                        ));
                    }
                }
            }
            () = &mut idle_sleep => {
                return Err(AxonError::provider_failure("provider websocket stream idle timeout"));
            }
            () = &mut progress_sleep => {
                on_progress();
                progress_sleep
                    .as_mut()
                    .reset(TokioInstant::now() + Duration::from_millis(250));
            }
            () = cancel_token.cancelled() => {
                return Err(AxonError::provider_transport(
                    "provider websocket streaming 已取消",
                    None,
                ));
            }
        }
    }
    if !received_response_frame && !closed_before_first_response_frame {
        return Err(AxonError::EmptyInput);
    }
    Ok(WebSocketReadResult {
        response_state,
        closed_before_first_response_frame,
        close_detail,
    })
}
