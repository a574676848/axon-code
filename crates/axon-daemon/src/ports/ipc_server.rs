//! 本地 IPC 服务端口。

/// 跨平台 IPC 服务生命周期端口。
pub trait IpcServer: Send {
    /// 绑定并开始接收请求。
    fn start(&mut self) -> Result<(), String>;
    /// 停止接收新连接并释放监听器。
    fn stop(&mut self) -> Result<(), String>;
}
