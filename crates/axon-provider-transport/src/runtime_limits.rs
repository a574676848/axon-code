//! Provider transport 内部运行限制。

/// Provider 流式响应默认空闲超时，对齐 Codex。
pub const PROVIDER_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
/// Provider HTTP 建连超时时间。
pub const PROVIDER_HTTP_CONNECT_TIMEOUT_MS: u64 = 10_000;
/// Provider HTTP 空闲连接在池中的保留时间。
pub const PROVIDER_HTTP_POOL_IDLE_TIMEOUT_MS: u64 = 1_800_000;
/// Provider HTTP TCP keepalive 探测间隔。
pub const PROVIDER_HTTP_TCP_KEEPALIVE_MS: u64 = 60_000;
/// Provider WebSocket 建连超时时间，对齐 Codex。
pub const PROVIDER_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
/// Provider WebSocket 逻辑会话空闲保留时间。
pub const PROVIDER_WEBSOCKET_SESSION_IDLE_TIMEOUT_MS: u64 = 1_800_000;
/// Provider 限流桶的空闲回收周期。
pub const PROVIDER_RATE_BUCKET_IDLE_TIMEOUT_MS: u64 = 1_800_000;
/// Provider WebSocket 单条物理连接的最大复用寿命。
pub const PROVIDER_WEBSOCKET_CONNECTION_MAX_AGE_MS: u64 = 3_300_000;
/// Provider transport 取消轮询间隔。
/// Provider HTTP non-stream fallback 最大超时时间。
pub const PROVIDER_HTTP_NON_STREAM_FALLBACK_TIMEOUT_MS: u64 = 30_000;
