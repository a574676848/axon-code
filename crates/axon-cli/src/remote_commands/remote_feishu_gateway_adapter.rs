//! Feishu Gateway Adapter：复用既有生产 SDK loop 与后台闭环。

use axon_daemon::{GatewayAdapter, GatewayContext};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use super::{
    RemoteFeishuSdkLoopCommand, load_remote_context,
    run_remote_execute_feishu_sdk_loop_with_cancellation,
};

const FEISHU_GATEWAY_ADAPTER_ID: &str = "feishu";

struct AdapterState {
    cancellation: Option<Arc<AtomicBool>>,
    thread: Option<JoinHandle<()>>,
}

/// Feishu 私有 WebSocket Gateway Adapter。
///
/// Adapter 不创建 Task、Execution 或 Delivery 存储，只包装现有 SDK loop；
/// 事件规范化、JSONL durable append、治理和反馈仍由既有远控链路完成。
pub(crate) struct FeishuGatewayAdapter {
    config_path: PathBuf,
    state: Mutex<AdapterState>,
}

impl FeishuGatewayAdapter {
    /// 创建尚未启动的 Feishu Adapter。
    #[must_use]
    pub(crate) fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            state: Mutex::new(AdapterState {
                cancellation: None,
                thread: None,
            }),
        }
    }

    fn request_cancellation(&self) {
        let cancellation = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancellation
            .clone();
        if let Some(cancellation) = cancellation {
            cancellation.store(true, Ordering::Release);
        }
    }
}

impl GatewayAdapter for FeishuGatewayAdapter {
    fn adapter_id(&self) -> &str {
        FEISHU_GATEWAY_ADAPTER_ID
    }

    fn start(&self, context: GatewayContext) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(thread) = state.thread.take() {
            if thread.is_finished() {
                let _ = thread.join();
                state.cancellation = None;
            } else {
                state.thread = Some(thread);
                return Err("Feishu Gateway Adapter 正在运行或退出中".to_owned());
            }
        }
        let permit = context
            .try_acquire_connection()
            .map_err(|error| error.to_string())?;
        let cancellation = Arc::new(AtomicBool::new(false));
        let thread_cancellation = Arc::clone(&cancellation);
        let config_path = self.config_path.clone();
        let thread = thread::spawn(move || {
            let _permit = permit;
            let Ok((_, loaded_context)) = load_remote_context(Some(&config_path)) else {
                axon_logger::warning("Feishu Gateway Adapter 加载运行时配置失败，Adapter 已退出");
                return;
            };
            let command = RemoteFeishuSdkLoopCommand {
                session_id: loaded_context.gateway_session_id(),
                payload_file: None,
                event_source_file: None,
                cursor_file: None,
                interval_ms: None,
                max_ticks: None,
                max_events: None,
                stop_after_first_event: false,
                wait_ms: None,
                connect_sdk: true,
                app_secret: None,
            };
            let feishu_context = loaded_context.feishu_runtime();
            let result = run_remote_execute_feishu_sdk_loop_with_cancellation(
                crate::OutputFormat::Json,
                &feishu_context,
                &command,
                thread_cancellation,
            );
            if result.exit_code != 0 {
                axon_logger::warning(&format!("Feishu Gateway Adapter 已退出: {}", result.stderr));
            }
        });
        state.cancellation = Some(cancellation);
        state.thread = Some(thread);
        Ok(())
    }

    fn drain(&self) {
        self.request_cancellation();
    }

    fn stop(&self) {
        self.request_cancellation();
        let thread = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .thread
            .take();
        if let Some(thread) = thread {
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                axon_logger::warning(
                    "Feishu Gateway Adapter 仍在退出，跳过阻塞式 join；取消信号已发送",
                );
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.thread = Some(thread);
            }
        }
    }
}

impl Drop for FeishuGatewayAdapter {
    fn drop(&mut self) {
        self.request_cancellation();
        if let Some(thread) = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .thread
            .take()
            && thread.is_finished()
        {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_has_stable_id_and_drain_is_idempotent_before_start() {
        let adapter = FeishuGatewayAdapter::new(PathBuf::from("config.toml"));
        assert_eq!(adapter.adapter_id(), FEISHU_GATEWAY_ADAPTER_ID);
        adapter.drain();
        adapter.stop();
    }
}
