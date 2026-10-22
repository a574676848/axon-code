//! Feishu OAPI WebSocket ping 保活。

use std::time::Duration;

use tokio::time::{Instant, interval_at};

use crate::remote_feishu_oapi_ws_sdk::RemoteFeishuOapiWsClientConfig;

pub(crate) const DEFAULT_PING_INTERVAL_SECONDS: u64 = 120;

pub(crate) fn feishu_oapi_ws_ping_interval(
    config: RemoteFeishuOapiWsClientConfig,
) -> tokio::time::Interval {
    let interval = Duration::from_secs(feishu_oapi_ws_ping_interval_seconds(config));
    interval_at(Instant::now() + interval, interval)
}

pub(crate) fn feishu_oapi_ws_ping_interval_seconds(config: RemoteFeishuOapiWsClientConfig) -> u64 {
    if config.ping_interval == 0 {
        return DEFAULT_PING_INTERVAL_SECONDS;
    }
    config.ping_interval
}
