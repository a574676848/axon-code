//! 网关扩展路由的装配：把各路由族按顺序组合成一个处理器。
//!
//! 网关只接受一个扩展处理器，因此所有 `/api/v1/*` 适配器都在这里按序拼装。排布
//! 规则只有一条：**精确路径的族必须排在工作台适配器之前**，因为工作台对 `/api/v1`
//! 前缀最宽，排在它后面的族永远轮不到。各族只认自己的精确路径，未命中一律返回
//! `None`，所以族与族之间没有顺序耦合。
//!
//! 这个文件是**唯一**的装配处：新增路由族只改这里，组合根本身不再跟着变长。

use std::path::Path;
use std::sync::Arc;

use axon_daemon::GatewayHttpRouteHandler;
use axon_daemon::infrastructure::gateway_auth::GatewayKeyStore;

use crate::workbench_http_adapter::{CompositeHttpAdapter, WorkbenchHttpAdapter};

/// 装配所需的运行态依赖。
pub(crate) struct AdapterDeps<'a> {
    /// 运行时根目录：多数族的数据都落在它下面。
    pub runtime_root: &'a Path,
    /// 配置文件路径：配置、密钥、供应商发现与系统状态都读它。
    pub config_path: &'a Path,
    /// 守护进程声明的工作区目录。
    pub command_root: &'a Path,
    /// 可观测性处理器：由调用方注入，属既有实现。
    pub observability: Arc<dyn GatewayHttpRouteHandler>,
    /// MCP 启动快照缓存。
    pub startup_snapshots: crate::run_commands::StartupSnapshotCache,
    /// 已就绪的工作台适配器（中断与作答入口已接好）。
    pub workbench: WorkbenchHttpAdapter,
    /// 多 Key 存储；打不开时为 None，管理面整族不注册（按 404 呈现而不是 500）。
    pub key_store: Option<Arc<GatewayKeyStore>>,
}

/// 按既定顺序装配全部路由族。
pub(crate) fn compose(deps: AdapterDeps<'_>) -> Arc<dyn GatewayHttpRouteHandler> {
    let runtime_root = deps.runtime_root;
    let config_path = deps.config_path;
    let mut handlers: Vec<Arc<dyn GatewayHttpRouteHandler>> = vec![
        // Cron 必须先于可观测性：可观测性对未知子路径也返回统一 404，
        // 排在前面会截断后续 Cron 路由。
        Arc::new(crate::cron_http_adapter::CronHttpAdapter::new(
            runtime_root.to_path_buf(),
            config_path.to_path_buf(),
        )),
        Arc::new(crate::memory_http_adapter::MemoryHttpAdapter::new(
            runtime_root.to_path_buf(),
            config_path.to_path_buf(),
        )),
        // 人格状态只读路由：设置 › 人设 据此展示已持久化的状态。
        Arc::new(crate::persona_http_adapter::PersonaHttpAdapter::new(
            runtime_root.to_path_buf(),
        )),
        // 配置类六族：配置、密钥、供应商发现、MCP 配置、技能清单、人设与准则。
        Arc::new(crate::config_http_adapter::ConfigHttpAdapter::new(
            config_path.to_path_buf(),
        )),
        Arc::new(crate::secrets_http_adapter::SecretsHttpAdapter::new(
            config_path.to_path_buf(),
        )),
        Arc::new(crate::providers_http_adapter::ProvidersHttpAdapter::new(
            config_path.to_path_buf(),
        )),
        Arc::new(crate::mcp_config_http_adapter::McpConfigHttpAdapter::new(
            runtime_root.to_path_buf(),
        )),
        Arc::new(crate::skills_http_adapter::SkillsHttpAdapter::new(
            runtime_root.to_path_buf(),
        )),
        Arc::new(crate::documents_http_adapter::DocumentsHttpAdapter::new(
            runtime_root.to_path_buf(),
        )),
        deps.observability,
        // MCP 运行态与目标看板同样要在工作台之前。
        Arc::new(crate::mcp_http_adapter::McpHttpAdapter::new(
            runtime_root.to_path_buf(),
            config_path.to_path_buf(),
            deps.command_root.to_path_buf(),
            deps.startup_snapshots,
        )),
        Arc::new(crate::goal_http_adapter::GoalHttpAdapter::new(
            runtime_root.to_path_buf(),
        )),
        // 后台完成通知收件箱：桌面端据此拉取 Goal 与定时任务的完成结论。
        Arc::new(
            crate::notifications_http_adapter::NotificationsHttpAdapter::new(
                runtime_root.to_path_buf(),
            ),
        ),
        // 系统元信息与运行态快照。
        Arc::new(crate::system_http_adapter::SystemHttpAdapter::new(
            runtime_root.to_path_buf(),
            config_path.to_path_buf(),
        )),
        // 文件、目录与附件三族。
        Arc::new(crate::files_http_adapter::FilesHttpAdapter::new(
            runtime_root.to_path_buf(),
        )),
        Arc::new(crate::fs_dirs_http_adapter::FsDirsHttpAdapter::new(
            runtime_root.to_path_buf(),
        )),
        Arc::new(
            crate::attachments_http_adapter::AttachmentsHttpAdapter::new(
                runtime_root.to_path_buf(),
            ),
        ),
    ];
    // 多 Key 管理面：同样必须在工作台之前；存储缺失时整族不注册。
    if let Some(store) = deps.key_store {
        handlers.push(Arc::new(crate::keys_http_adapter::KeysHttpAdapter::new(
            store,
        )));
    }
    // 工作台最后：它对 `/api/v1` 前缀最宽，必须兜在其余各族之后。
    handlers.push(Arc::new(deps.workbench));
    Arc::new(CompositeHttpAdapter::new(handlers))
}
