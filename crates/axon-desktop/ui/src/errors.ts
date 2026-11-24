/** 两种 IPC 通道统一保留服务端的中文错误说明。 */
export function backendError(failure: unknown): Error {
    if (failure instanceof Error) return failure;
    if (typeof failure === 'string') return new Error(failure);
    if (typeof failure === 'object' && failure !== null && 'message' in failure &&
        typeof failure.message === 'string') return new Error(failure.message);
    return new Error('后端调用失败，请重试');
}

/**
 * 传输层「后端服务未启用」的判据。
 *
 * 事实源只有两处断连说明：Rust 侧 `gateway.rs` 的传输层失败统一带
 * `网关不可达` 前缀（网关进程没起来、端口未监听、连接被拒），以及本地调试桥
 * 找不到桌面宿主时的说明。业务拒绝（凭据不符、状态码、归属不符、版本过旧）
 * 都不带这些前缀，因此不会被误判成「服务未启用」。
 */
const BACKEND_UNAVAILABLE_MARKERS = ['网关不可达', '未连接到桌面宿主'] as const;

/** 该失败是否属于「后端服务未启用」这一类传输层故障。 */
export function isBackendUnavailable(failure: unknown): boolean {
  const message = backendError(failure).message;
  return BACKEND_UNAVAILABLE_MARKERS.some((marker) => message.includes(marker));
}
