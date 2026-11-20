//! Daemon 健康快照旁路。
//!
//! 主处理链由一把互斥锁串行：一次长提交会把排在后面的探活一起堵住，上游据此把
//! 「排队」误判成「无响应」。这里让一条独立线程按固定节奏走同一个 `GetHealth`
//! 入口刷新快照，旁路只读这份快照，探活因此不再排在长请求后面；快照只落后一个
//! 刷新周期，而探活要判读的是「还活着吗」，几百毫秒的陈旧不影响结论。
//!
//! 旁路只认 `GetHealth`：其它命令一律交回主处理链，不在这里造第二套语义。

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use axon_daemon::infrastructure::ipc::{IpcDispatch, IpcFastPath};
use axon_protocol::{DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest, DaemonResponse};

/// 健康快照的刷新间隔：探活只判读「是否还活着」，几百毫秒的陈旧不影响结论。
const HEALTH_SNAPSHOT_REFRESH_MS: u64 = 500;

/// 启动快照线程，返回只读快照的 IPC 旁路。
///
/// `handle` 由宿主提供：它就是主处理链的同一个入口，快照因此与 `axon daemon health`
/// 完全同源；`should_stop` 与接受循环共用同一套停机判据，停机请求与 fence 都能让
/// 这条线程收手。
pub(crate) fn spawn<H, S>(mut handle: H, should_stop: S) -> Result<IpcFastPath, String>
where
    H: FnMut(DaemonRequest) -> IpcDispatch + Send + 'static,
    S: Fn() -> bool + Send + 'static,
{
    let health_snapshot = Arc::new(RwLock::new(None::<serde_json::Value>));
    let snapshot_cell = Arc::clone(&health_snapshot);
    std::thread::Builder::new()
        .name("daemon-health-snapshot".to_owned())
        .spawn(move || {
            while !should_stop() {
                let probe = DaemonRequest {
                    message_type: "request".to_owned(),
                    request_id: "health-snapshot".to_owned(),
                    protocol_version: DAEMON_PROTOCOL_VERSION,
                    client_id: "daemon-health-snapshot".to_owned(),
                    session_id: None,
                    command: DaemonCommand::GetHealth,
                };
                // 探活只可能是单次响应；订阅型返回值与旁路无关，直接忽略。
                if let IpcDispatch::Complete(responses) = handle(probe)
                    && let Some(body) = responses.first().map(|response| response.body.clone())
                {
                    *snapshot_cell
                        .write()
                        .unwrap_or_else(PoisonError::into_inner) = Some(body);
                }
                std::thread::sleep(Duration::from_millis(HEALTH_SNAPSHOT_REFRESH_MS));
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Arc::new(move |request: &DaemonRequest| {
        if !matches!(&request.command, DaemonCommand::GetHealth) {
            return None;
        }
        // 版本不匹配交回主处理链：那里有统一的协议错误口径，旁路不另造一份。
        if request.protocol_version != DAEMON_PROTOCOL_VERSION {
            return None;
        }
        let snapshot = health_snapshot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        Some(IpcDispatch::complete(vec![DaemonResponse::success(
            request.request_id.clone(),
            snapshot,
        )]))
    }))
}
