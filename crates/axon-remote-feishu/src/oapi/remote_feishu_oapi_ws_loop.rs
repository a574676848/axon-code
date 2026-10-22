use std::{
    sync::{Arc, Mutex, atomic::AtomicBool, mpsc::Sender},
    time::Duration,
};

use crate::remote_feishu_oapi_ws_reconnect::RemoteFeishuOapiWsClientConfig;

/// 服务端 ClientConfig 的跨连接快照。
#[derive(Debug, Clone)]
pub struct RemoteFeishuOapiWsClientConfigState {
    inner: Arc<Mutex<RemoteFeishuOapiWsClientConfig>>,
}

impl Default for RemoteFeishuOapiWsClientConfigState {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RemoteFeishuOapiWsClientConfig::default())),
        }
    }
}

impl RemoteFeishuOapiWsClientConfigState {
    /// 读取最新服务端 ClientConfig。
    #[must_use]
    pub fn snapshot(&self) -> RemoteFeishuOapiWsClientConfig {
        *self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn replace(&self, config: RemoteFeishuOapiWsClientConfig) {
        *self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
    }
}

/// WS loop 可控退出条件。
#[derive(Debug, Clone, Default)]
pub struct RemoteFeishuOapiWsLoopControl {
    /// 最大处理入站业务事件数；为空时持续运行至连接关闭。
    pub max_events: Option<usize>,
    /// 最大等待时长；到期后优雅退出并返回已采集遥测。
    pub max_duration: Option<Duration>,
    pub(crate) cancellation: Option<Arc<AtomicBool>>,
    pub(crate) connected_signal: Option<Sender<()>>,
    pub(crate) client_config_state: Option<RemoteFeishuOapiWsClientConfigState>,
}

impl RemoteFeishuOapiWsLoopControl {
    /// 创建受控退出配置。
    #[must_use]
    pub fn new(max_events: Option<usize>) -> Self {
        Self {
            max_events,
            max_duration: None,
            cancellation: None,
            connected_signal: None,
            client_config_state: None,
        }
    }

    /// 设置最大等待时长。
    #[must_use]
    pub fn with_max_duration(mut self, max_duration: Option<Duration>) -> Self {
        self.max_duration = max_duration;
        self
    }

    /// 设置由宿主控制的取消信号。
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    /// 设置 WebSocket 握手成功信号。
    #[must_use]
    pub fn with_connected_signal(mut self, signal: Sender<()>) -> Self {
        self.connected_signal = Some(signal);
        self
    }

    /// 设置供宿主重连循环读取的服务端 ClientConfig 快照。
    #[must_use]
    pub fn with_client_config_state(mut self, state: RemoteFeishuOapiWsClientConfigState) -> Self {
        self.client_config_state = Some(state);
        self
    }

    pub(crate) fn replace_client_config(&self, config: RemoteFeishuOapiWsClientConfig) {
        if let Some(state) = &self.client_config_state {
            state.replace(config);
        }
    }

    pub(crate) fn signal_connected(&self) {
        if let Some(signal) = &self.connected_signal {
            let _ = signal.send(());
        }
    }
}

/// WS loop 遥测报告。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoteFeishuOapiWsLoopReport {
    /// 是否完成 WS 连接。
    pub connected: bool,
    /// 已收到 binary frame 数。
    pub received_binary_frames: usize,
    /// 已收到 data frame 数。
    pub received_data_frames: usize,
    /// 已处理入站业务事件数。
    pub handled_events: usize,
    /// 已跳过的不支持 data frame 数。
    pub skipped_data_frames: usize,
    /// 已发送 ping 数。
    pub sent_pings: usize,
    /// 已收到 pong 数。
    pub received_pongs: usize,
    /// 上层 callback 请求停止后的退出次数。
    pub requested_stops: usize,
}
