//! Feishu OAPI WebSocket 服务端重连配置与策略。

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 服务端下发的 client config。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteFeishuOapiWsClientConfig {
    /// 重连次数，负数表示无限次。
    pub reconnect_count: i32,
    /// 重连间隔秒。
    pub reconnect_interval: u64,
    /// 首次重连抖动秒。
    pub reconnect_nonce: u64,
    /// ping 间隔秒。
    pub ping_interval: u64,
}

impl Default for RemoteFeishuOapiWsClientConfig {
    fn default() -> Self {
        Self {
            reconnect_count: -1,
            reconnect_interval: 120,
            reconnect_nonce: 30,
            ping_interval: 120,
        }
    }
}

/// Feishu 服务端指定的 WebSocket 重连策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteFeishuOapiWsReconnectPolicy {
    retry_limit: Option<usize>,
    retry_interval: Duration,
    first_retry_jitter: Duration,
}

impl RemoteFeishuOapiWsClientConfig {
    /// 将服务端配置转换为重连策略。
    #[must_use]
    pub fn reconnect_policy(self) -> RemoteFeishuOapiWsReconnectPolicy {
        RemoteFeishuOapiWsReconnectPolicy {
            retry_limit: usize::try_from(self.reconnect_count).ok(),
            retry_interval: Duration::from_secs(self.reconnect_interval),
            first_retry_jitter: Duration::from_secs(self.reconnect_nonce),
        }
    }
}

impl RemoteFeishuOapiWsReconnectPolicy {
    /// 当前重连周期是否允许进行指定序号的重试，序号从 1 开始。
    #[must_use]
    pub fn permits_retry(self, retry_number: usize) -> bool {
        self.retry_limit.is_none_or(|limit| retry_number <= limit)
    }

    /// 计算指定序号重试前的等待时间；首个重试使用服务端指定抖动窗口。
    #[must_use]
    pub fn delay_before_retry(self, retry_number: usize, entropy_millis: u64) -> Duration {
        if retry_number != 1 || self.first_retry_jitter.is_zero() {
            return self.retry_interval;
        }
        let jitter_millis = u64::try_from(self.first_retry_jitter.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(entropy_millis % jitter_millis)
    }
}
