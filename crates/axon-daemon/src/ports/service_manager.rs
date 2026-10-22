//! 操作系统用户服务端口。

/// Daemon 用户服务管理器。
pub trait ServiceManager: Send + Sync {
    /// 注册服务定义。
    fn install(&self) -> Result<(), String>;
    /// 删除服务定义。
    fn uninstall(&self) -> Result<(), String>;
    /// 启动服务。
    fn start(&self) -> Result<(), String>;
    /// 停止服务。
    fn stop(&self) -> Result<(), String>;
    /// 查询是否运行。
    fn is_running(&self) -> bool;
}
