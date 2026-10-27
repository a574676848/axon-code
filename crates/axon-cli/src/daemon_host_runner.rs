use crate::runtime_paths::daemon_endpoint_for_root;
use axon_config::ConfigLoader;
use axon_daemon::{
    ActiveTurnSteerSink, DaemonGatewayBridge, DaemonHost, EventProviderRegistry, GatewayAuth,
    GatewayEgress, GatewayEventOptions, GatewayIngress, GatewayKernel, HttpGatewayAdapter, LogSink,
    WebSocketGatewayAdapter,
    infrastructure::ipc::{IpcDispatch, JsonlIpcClient, JsonlIpcServer},
};
use axon_domain::{InteractionId, InteractionRecord, RuntimeHostId, SYSTEM_SOURCE_CHANNEL};
use axon_remote_runtime::{DeliveryStore, RemoteNotificationOutboxRuntime};
use axon_session::{
    SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore, interaction_session_lane_id,
};
use axon_session_runtime::AgentTurnExecutor;
#[cfg(test)]
use axon_session_runtime::EventPublisher;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use super::DaemonRunCommand;
use super::daemon_delivery_health::CliDeliveryHealthStore;
use super::{DEFAULT_ENDPOINT, RUNTIME_DATA_DIR, now_ms};
use crate::run_commands::run_steer::ActivePrimaryTurnRegistry;

#[path = "daemon_host_runner_support.rs"]
mod support;
pub(super) use support::{
    CliProcessIdentity, DaemonEventPublisher, DaemonMaintenanceSignal, DaemonPidFileGuard,
    SystemClock,
};

#[path = "daemon_host_runner_workers.rs"]
mod workers;

#[path = "daemon_host_runner_steer_input.rs"]
mod steer_input;
use steer_input::steer_message;

/// 把网关访问日志接到 Axon 统一日志上。
///
/// 访问日志的可见级别由 `[agent] log_level` 决定：想事后归因偶发卡顿，把它设为
/// `info`（只看慢请求与失败则 `warning`）。网关自身不做任何级别过滤。
struct GatewayAccessLog;

impl LogSink for GatewayAccessLog {
    fn append(&self, level: &str, message: &str) {
        match level {
            "ERROR" => axon_logger::error(message),
            "WARNING" => axon_logger::warning(message),
            "DEBUG" => axon_logger::debug(message),
            _ => axon_logger::info(message),
        }
    }
}

#[path = "daemon_host_runner_answer_ipc.rs"]
mod answer_ipc;

#[path = "daemon_host_runner_question_durable.rs"]
mod question_durable;

#[path = "daemon_host_runner_question_wiring.rs"]
mod question_wiring;

use answer_ipc::{DaemonIpcAnswerEntry, answer_question_ipc_response, ipc_error_response};
use question_wiring::QuestionWiring;

pub(super) const HOST_LEASE_DURATION_MS: u64 = 30_000;
pub(super) const HOST_RENEW_INTERVAL_MS: u64 = HOST_LEASE_DURATION_MS / 3;
pub(super) const ACTION_LEASE_DURATION_MS: u64 = 120_000;
pub(super) const SCHEDULE_LEASE_DURATION_MS: u64 = 30_000;
pub(super) const SUPERVISOR_BATCH_LIMIT: u32 = 32;
pub(super) const MAINTENANCE_TIMER_INTERVAL_MS: u64 = 1_000;
/// Cron tick 间隔：Cron 任务体时长不可控（用户脚本、清理、Agent turn），
/// 必须运行在独立线程，避免拖住调度与监督 tick。
pub(super) const CRON_TICK_INTERVAL_MS: u64 = 1_000;
pub(super) const FEISHU_DELIVERY_RECOVERY_INTERVAL_MS: u64 = 30_000;
pub(super) const RELEASE_GC_INTERVAL_MS: u64 = 30_000;
/// 服务停机唤醒线程的轮询间隔。
pub(super) const SERVICE_STOP_WAKEUP_POLL_MS: u64 = 50;

#[derive(Clone)]
struct DaemonPrimaryTurnSteerSink {
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
}

impl ActiveTurnSteerSink for DaemonPrimaryTurnSteerSink {
    fn try_steer(&self, interaction: &InteractionRecord) -> Result<bool, String> {
        let lane_id = interaction_session_lane_id(interaction)
            .ok_or_else(|| "Primary Turn steer 必须绑定 Workspace".to_owned())?;
        self.active_primary_turns
            .try_send_message(
                &ActivePrimaryTurnRegistry::affinity_key(&lane_id),
                steer_message(interaction),
            )
            .map(|result| result.is_some())
    }

    fn append_turn_input(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<Option<InteractionId>, String> {
        if interaction.source.channel() == SYSTEM_SOURCE_CHANNEL {
            return Ok(None);
        }
        let lane_id = interaction_session_lane_id(interaction)
            .ok_or_else(|| "Primary Turn append 必须绑定 Workspace".to_owned())?;
        self.active_primary_turns.try_send_message(
            &ActivePrimaryTurnRegistry::affinity_key(&lane_id),
            steer_message(interaction),
        )
    }
}

pub(super) fn run_daemon_host(command: &DaemonRunCommand) -> crate::CliRunResult {
    // 停止请求标志必须由生产路径提供：守护进程可能以更高完整性级别运行，
    // 外部无法结束它的进程，只能请求它自行退出（见 daemon_stop_request）。
    run_daemon_host_with_mode(command, Some(Arc::new(AtomicBool::new(false))))
}

pub(super) fn run_daemon_host_with_mode(
    command: &DaemonRunCommand,
    stop_requested: Option<Arc<AtomicBool>>,
) -> crate::CliRunResult {
    match try_run_daemon_host(command, stop_requested) {
        Ok(()) => daemon_result(0, String::new()),
        Err(error) => daemon_result(1, error),
    }
}

fn try_run_daemon_host(
    command: &DaemonRunCommand,
    stop_requested: Option<Arc<AtomicBool>>,
) -> Result<(), String> {
    // Daemon endpoint 必须在声明运行态 root override 前解析；否则非默认 root 会被误判为默认根。
    let endpoint = resolved_daemon_endpoint(command);
    // Daemon 的运行态根目录由 `--root` 决定，必须先于任何运行态路径解析完成声明。
    axon_config::set_runtime_root_override(&command.root)
        .map_err(|error| format!("声明 Daemon 运行态根目录失败: {error:?}"))?;
    let _ = crate::init_logger_for_config_path(&command.config_path);
    std::env::set_current_dir(&command.root)
        .map_err(|error| format!("设置 Daemon 工作目录失败: {error}"))?;
    // Daemon 是独立进程，必须自行加载配置相邻 `.env`，否则 Provider 凭据不可用。
    crate::run_commands::load_run_env_file(&crate::setup_commands::env_path_for_config(
        &command.config_path,
    ))
    .map_err(|error| format!("加载 Daemon 凭据失败: {error}"))?;
    let loaded_config = ConfigLoader::load_from_path(&command.config_path)
        .map_err(|error| format!("加载 Daemon 配置失败: {error:?}"))?;
    // 飞书问答卡片要用同一份远端配置；在 gateway 字段被取出前先留一份。
    let feishu_websocket_config = loaded_config.config.remote.feishu_websocket.clone();
    crate::run_commands::cleanup_stale_run_controls(&command.config_path)
        .map_err(|error| format!("清理 stale run 控制文件失败: {error}"))?;
    let gateway_config = loaded_config.config.remote.gateway;
    let gateway_auth = axon_config::provider_credential(&gateway_config.api_token_env)
        .or_else(|| std::env::var(&gateway_config.api_token_env).ok())
        .and_then(|token| GatewayAuth::from_token(&token).ok())
        .map(Arc::new);
    ensure_system_crons(command)?;
    let runtime_root = daemon_runtime_root(command);
    // 上一轮停止可能留下请求文件；必须先清掉，否则新进程会被立刻要求退出。
    crate::daemon_commands::daemon_stop_request::clear(&runtime_root);
    let store = SqliteRuntimeStore::from_root(&runtime_root);
    // Gateway 与 CLI、cron 一样是 Interaction 提交方，但它没有调用方 cwd，事件只能落在
    // Daemon 自己的工作目录上；提交方必须先注册 Workspace，否则主 turn 打开运行时会被拒。
    register_daemon_workspace(&store);
    let delivery_signal = Arc::new(DaemonMaintenanceSignal::default());
    // 显式 MCP 刷新请求要在 IPC 侧清除健康负缓存并重建快照，因此缓存句柄由运行函数
    // 持有，再分别交给 Worker 与 IPC handler。
    let startup_snapshots = crate::run_commands::StartupSnapshotCache::default();
    startup_snapshots.spawn_prewarm(command.config_path.clone(), command.root.clone());
    // 活跃主 turn 注册表由运行函数创建：问答答案注入、网关中断与后台 Worker 必须共用同一份。
    let active_primary_turns = crate::run_commands::run_steer::ActivePrimaryTurnRegistry::default();
    // 主 turn 唤醒信号必须先于问答装配创建：迟到作答落库后由答案注入器直接唤醒一次领取。
    let primary_turn_signal = Arc::new(DaemonMaintenanceSignal::default());
    // 问答宿主单例与答案服务在此装配；两者共用同一个宿主实例，见 QuestionWiring。
    // 飞书卡片 sink 在后台预热凭据：提问事件当场出卡，未就绪时由网关后台 tick 兜底。
    let feishu_question_sink = crate::remote_commands::warm_feishu_question_card_sink(
        command.config_path.clone(),
        command.root.clone(),
        feishu_websocket_config,
    );
    let questions = QuestionWiring::assemble(
        store.clone(),
        active_primary_turns.clone(),
        feishu_question_sink,
        primary_turn_wake_hook(Arc::clone(&primary_turn_signal)),
    );
    // durable 落点要用同一份运行态存储：问答 sink 建在宿主之前，安装在其后。
    let question_store = store.clone();
    // 关停标志先于宿主构建：后台 Worker 启动后立即依赖它判断是否需要收口。
    let workers_running = Arc::new(AtomicBool::new(true));
    let mut host = build_daemon_host(
        command,
        store,
        Arc::clone(&delivery_signal),
        startup_snapshots.clone(),
        active_primary_turns.clone(),
        questions.primary_turn_port(),
    )?;
    host.start().map_err(|error| error.to_string())?;
    // 宿主起来之后才有 host 记录与订阅桥；迟到作答的问答帧靠它落进会话事件流。
    questions.install_durable_question_events(&host, question_store);
    let _pid_file = DaemonPidFileGuard::acquire(&runtime_root)?;
    let delivery_owner = host.owner_identity().map_err(|error| error.to_string())?;
    let server = bind_daemon_server(&endpoint, command)?;
    let cron_adapter = load_cron_adapter(command)?;
    let maintenance_signal = Arc::new(DaemonMaintenanceSignal::default());
    // provider 内容资产与事件正文归档根都从运行态根目录派生；仓库内不落任何 provider 脚本。
    let event_providers = Arc::new(EventProviderRegistry::new(
        command.root.join(axon_config::AXON_EVENT_PROVIDERS_DIR),
    ));
    let gateway_bridge = Arc::new(DaemonGatewayBridge::new_with_events(
        host.gateway_dependencies(),
        // HTTP/WS 的 Gateway Adapter 跑在独立线程上，事件入账后必须显式唤醒主 turn；
        // 否则外部事件只能等 1s 定时兜底才发现。
        GatewayEventOptions::new(Arc::clone(&event_providers), runtime_root.clone())
            .with_waker(primary_turn_waker(Arc::clone(&primary_turn_signal))),
    ));
    let gateway_ingress: Arc<dyn GatewayIngress> = gateway_bridge.clone();
    let gateway_egress: Arc<dyn GatewayEgress> = gateway_bridge.clone();
    let gateway_kernel = Arc::new(GatewayKernel::with_default_connection_limit_and_auth(
        gateway_ingress,
        gateway_egress,
        gateway_auth,
    ));
    gateway_kernel
        .register_adapter(Arc::new(
            crate::remote_commands::remote_feishu_gateway_adapter::FeishuGatewayAdapter::new(
                command.config_path.clone(),
            ),
        ))
        .map_err(|error| format!("注册 Feishu Gateway Adapter 失败: {error}"))?;
    let observability_http_handler = Arc::new(
        crate::usage_statistics::http_adapter::ObservabilityHttpAdapter::new(
            command.config_path.clone(),
        ),
    );
    // 网关只接受一个扩展处理器，各路由族在此按顺序组合：
    // 可观测性在前，工作台其后，互不命中的路径由网关统一回 404。
    let workbench_root = axon_config::runtime_axon_root_path(&command.config_path)
        .map_err(|error| format!("解析 Axon 运行时根目录失败: {error:?}"))?;
    let route_handler = Arc::new(crate::workbench_http_adapter::CompositeHttpAdapter::new(
        vec![
            // Cron 必须先于可观测性适配器：可观测性对未知子路径也返回统一 404，
            // 若排在前面会截断后续 Cron 路由。
            Arc::new(crate::cron_http_adapter::CronHttpAdapter::new(
                workbench_root.clone(),
                command.config_path.clone(),
            )),
            Arc::new(crate::memory_http_adapter::MemoryHttpAdapter::new(
                workbench_root.clone(),
                command.config_path.clone(),
            )),
            // 人格状态只读路由：桌面设置页的人设分区据此展示已持久化的状态。
            Arc::new(crate::persona_http_adapter::PersonaHttpAdapter::new(
                workbench_root.clone(),
            )),
            observability_http_handler,
            // MCP 运行态要先于工作台：工作台适配器对 /api/v1 前缀最宽，排在它后面才轮得到。
            Arc::new(crate::mcp_http_adapter::McpHttpAdapter::new(
                workbench_root.clone(),
                command.config_path.clone(),
                command.root.clone(),
                startup_snapshots.clone(),
            )),
            Arc::new(
                crate::workbench_http_adapter::WorkbenchHttpAdapter::from_root(workbench_root)
                    // 中断入口要触达正在运行的主 turn，因此共享同一份注册表。
                    .with_active_primary_turns(active_primary_turns.clone())
                    // 作答入口复用同一份宿主单例，答案才能投给等待中的提问工具。
                    .with_answer_service(questions.service.clone()),
            ),
        ],
    ));
    gateway_kernel
        .register_adapter(Arc::new(
            HttpGatewayAdapter::new(gateway_config.http_bind, Arc::clone(&event_providers))
                .with_route_handler(route_handler)
                // 网关访问日志接入统一日志：慢请求与失败按 WARNING 落盘。
                .with_log_sink(Arc::new(GatewayAccessLog)),
        ))
        .map_err(|error| format!("注册 HTTP Gateway Adapter 失败: {error}"))?;
    gateway_kernel
        .register_adapter(Arc::new(WebSocketGatewayAdapter::new(
            gateway_config.websocket_bind,
        )))
        .map_err(|error| format!("注册 WebSocket Gateway Adapter 失败: {error}"))?;
    gateway_kernel
        .start()
        .map_err(|error| format!("启动 Gateway Adapter 失败: {error}"))?;
    if let Some(stop_requested) = stop_requested.clone() {
        // 停止请求通道：调用方只写文件，守护进程自行 drain 退出，不要求同级权限。
        crate::daemon_commands::daemon_stop_request::watch(
            runtime_root.clone(),
            Arc::clone(&stop_requested),
            Duration::from_millis(SERVICE_STOP_WAKEUP_POLL_MS),
        );
        start_gateway_shutdown_watcher(
            &endpoint,
            host.clone(),
            Arc::clone(&gateway_kernel),
            stop_requested,
        );
    }
    workers::start_background_workers(
        command,
        &host,
        cron_adapter,
        delivery_owner,
        Arc::clone(&workers_running),
        Arc::clone(&maintenance_signal),
        Arc::clone(&primary_turn_signal),
        delivery_signal,
        active_primary_turns,
    );
    let result = serve_daemon_ipc(
        server,
        host,
        Arc::clone(&gateway_kernel),
        Arc::clone(&maintenance_signal),
        DaemonIpcAnswerEntry {
            service: questions.service.clone(),
            primary_turn_signal: Arc::clone(&primary_turn_signal),
        },
        McpRefreshContext {
            config_path: command.config_path.clone(),
            snapshots: startup_snapshots,
        },
        stop_requested,
    );
    // 守护进程收口：唤醒仍在等待用户作答的提问工具，避免它们空等到预算耗尽。
    questions.shutdown.cancel();
    workers_running.store(false, Ordering::Release);
    maintenance_signal.notify();
    primary_turn_signal.notify();
    let _ = gateway_kernel.begin_drain();
    let _ = gateway_kernel.stop();
    result
}

/// 注册 Daemon 自身工作目录对应的 Workspace。
///
/// 外部事件没有调用方 cwd，默认落在 Daemon 的 `--root` 工作目录上；该 Workspace 必须先注册，
/// 主 turn 才能为事件打开运行时。注册失败只留告警，不阻塞 Daemon 启动。
fn register_daemon_workspace(store: &SqliteRuntimeStore) {
    let Ok(cwd) = std::env::current_dir() else {
        axon_logger::warning("Daemon 工作目录不可访问，外部事件会因 Workspace 未注册被拒绝");
        return;
    };
    match WorkspaceResolver.resolve(&cwd, now_ms()) {
        Ok(workspace) => {
            if let Err(error) = store.register_workspace(&workspace) {
                axon_logger::warning(&format!("注册 Daemon Workspace 失败: {error}"));
            }
        }
        Err(error) => axon_logger::warning(&format!("解析 Daemon Workspace 失败: {error}")),
    }
}

/// 外部事件 durable 入账后唤醒 primary turn；唤醒只是提示，事实源仍是 durable inbox。
fn primary_turn_waker(signal: Arc<DaemonMaintenanceSignal>) -> Arc<dyn Fn() + Send + Sync> {
    let hook = primary_turn_wake_hook(signal);
    Arc::new(move || {
        let _ = hook();
    })
}

/// 主 turn 领取唤醒端口：与网关外部事件共用同一个信号。
///
/// 置位唤醒标记不会失败，因此这里始终返回 `Ok`；`Err` 分支留给未装配唤醒端口的
/// 实现，调用方据此留痕而不是静默跳过。
fn primary_turn_wake_hook(
    signal: Arc<DaemonMaintenanceSignal>,
) -> crate::user_question::lane_steerer::PrimaryTurnWakeHook {
    Arc::new(move || {
        signal.notify();
        Ok(())
    })
}

fn daemon_result(exit_code: u8, stderr: String) -> crate::CliRunResult {
    crate::CliRunResult {
        exit_code,
        stdout: String::new(),
        stderr,
    }
}

fn daemon_runtime_root(command: &DaemonRunCommand) -> PathBuf {
    if command
        .root
        .file_name()
        .is_some_and(|name| name == RUNTIME_DATA_DIR)
    {
        command.root.clone()
    } else {
        command.root.join(RUNTIME_DATA_DIR)
    }
}

// 系统 Cron（记忆巩固、usage 汇总、会话清理）由守护进程自身调度执行，
// 服务模式与前台模式都必须注册，否则服务形态下这些维护任务永不运行。
fn ensure_system_crons(command: &DaemonRunCommand) -> Result<(), String> {
    let cron_root = crate::runtime_paths::runtime_axon_root_path(&command.config_path)
        .map_err(|error| format!("解析 Cron 运行态根目录失败: {error}"))?;
    crate::cron_commands::ensure_memory_consolidation_cron(&cron_root)
        .map_err(|error| format!("初始化 Daemon 系统 Cron 失败: {error}"))?;
    crate::cron_commands::ensure_usage_rollup_cron(&cron_root)
        .map_err(|error| format!("初始化 Daemon usage Cron 失败: {error}"))?;
    crate::cron_commands::ensure_session_cleanup_cron(&cron_root)
        .map(|_| ())
        .map_err(|error| format!("初始化 Daemon session cleanup Cron 失败: {error}"))
}

pub(super) type CliDaemonHost = DaemonHost<
    SqliteRuntimeStore,
    AgentTurnExecutor<crate::run_commands::run_remote_execution::CliProviderTurnRunner>,
    DaemonEventPublisher,
    SystemClock,
    CliProcessIdentity,
>;

fn build_daemon_host(
    command: &DaemonRunCommand,
    store: SqliteRuntimeStore,
    delivery_signal: Arc<DaemonMaintenanceSignal>,
    startup_snapshots: crate::run_commands::StartupSnapshotCache,
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
    user_question_port: Option<Arc<dyn axon_agent_runtime::UserQuestionPort>>,
) -> Result<CliDaemonHost, String> {
    store
        .reap_dead_hosts(now_ms())
        .map_err(|error| format!("回收已退出 Daemon Host lease 失败: {error}"))?;
    let runner = crate::run_commands::run_remote_execution::CliProviderTurnRunner {
        config_path: command.config_path.clone(),
        active_primary_turns: active_primary_turns.clone(),
        startup_snapshots,
        // 只有守护进程主 turn 装配用户提问端口；子代理与后台执行保持 None。
        user_question_port,
    };
    let delivery_waker = Arc::clone(&delivery_signal);
    crate::remote_commands::register_delivery_waker(Arc::new(move || {
        delivery_waker.notify();
    }));
    let host_id =
        RuntimeHostId::new(format!("axon-daemon-{}", std::process::id())).expect("稳定 Host ID");
    let host = DaemonHost::new(
        store.clone(),
        AgentTurnExecutor::new(runner),
        DaemonEventPublisher::new(
            command.root.clone(),
            command.config_path.clone(),
            delivery_signal,
        ),
        SystemClock,
        CliProcessIdentity,
        host_id,
        HOST_LEASE_DURATION_MS,
        ACTION_LEASE_DURATION_MS,
    )
    .with_active_turn_steer_sink(DaemonPrimaryTurnSteerSink {
        active_primary_turns: active_primary_turns.clone(),
    })
    .with_delivery_health_store(CliDeliveryHealthStore::new(DeliveryStore::new(
        RemoteNotificationOutboxRuntime::resolve_store_path(
            axon_remote_feishu::remote_feishu_runtime_root(&command.root),
        ),
    )));
    let notification_config = ConfigLoader::load_from_path(&command.config_path)
        .map_err(|error| format!("读取飞书通知配置失败：{error:?}"))?
        .config
        .remote
        .feishu_websocket;
    host.set_feishu_notification_chat_id(
        (notification_config.enabled && notification_config.enable_notification_sending)
            .then_some(notification_config.notification_chat_id.as_str()),
    )
    .map_err(|error| error.to_string())?;
    Ok(host)
}

fn bind_daemon_server(
    endpoint: &str,
    _command: &DaemonRunCommand,
) -> Result<JsonlIpcServer, String> {
    #[cfg(windows)]
    {
        let sid = super::daemon_service_manager::current_windows_user_sid()?;
        JsonlIpcServer::bind_with_user_sid(endpoint, Some(&sid)).map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    JsonlIpcServer::bind(endpoint).map_err(|error| error.to_string())
}

fn resolved_daemon_endpoint(command: &DaemonRunCommand) -> String {
    if command.endpoint == DEFAULT_ENDPOINT {
        daemon_endpoint_for_root(&command.root)
    } else {
        command.endpoint.clone()
    }
}

fn load_cron_adapter(
    command: &DaemonRunCommand,
) -> Result<crate::remote_commands::FeishuCronAdapter, String> {
    crate::remote_commands::load_remote_context(Some(&command.config_path))
        .map(|(_, context)| crate::remote_commands::FeishuCronAdapter::new(context))
        .map_err(|error| format!("加载 Daemon Cron 上下文失败: {error}"))
}

/// MCP 实时刷新所需的宿主资源。
struct McpRefreshContext {
    config_path: PathBuf,
    snapshots: crate::run_commands::StartupSnapshotCache,
}

fn serve_daemon_ipc(
    server: JsonlIpcServer,
    host: CliDaemonHost,
    gateway_kernel: Arc<GatewayKernel>,
    maintenance_signal: Arc<DaemonMaintenanceSignal>,
    answer_entry: DaemonIpcAnswerEntry,
    mcp_refresh: McpRefreshContext,
    stop_requested: Option<Arc<AtomicBool>>,
) -> Result<(), String> {
    let mut handler_host = host.clone();
    let handler_gateway = Arc::clone(&gateway_kernel);
    let DaemonIpcAnswerEntry {
        service: answer_service,
        primary_turn_signal,
    } = answer_entry;
    let handler = move |request: axon_protocol::DaemonRequest| {
        if let axon_protocol::DaemonCommand::RefreshMcpCapabilities { cwd } = &request.command {
            if request.protocol_version != axon_protocol::DAEMON_PROTOCOL_VERSION {
                return IpcDispatch::complete(vec![ipc_error_response(
                    &request.request_id,
                    "Daemon IPC 协议版本不匹配",
                )]);
            }
            return IpcDispatch::complete(handle_mcp_refresh_request(
                &mcp_refresh,
                &request.request_id,
                cwd,
            ));
        }
        // 作答必须走统一答案入口：校验、CAS 结算、二选一投递与落库后的唤醒全部收敛在
        // UserQuestionAnswerService 内。IPC 不允许存在「只结算、不投递」的第二条路径，
        // 否则答案会落库却送不到等待中的提问工具，也不会开启续跑的一轮。
        if let axon_protocol::DaemonCommand::AnswerQuestion {
            lane_id,
            call_id,
            answers,
            channel,
        } = &request.command
        {
            if request.protocol_version != axon_protocol::DAEMON_PROTOCOL_VERSION {
                return IpcDispatch::complete(vec![ipc_error_response(
                    &request.request_id,
                    "Daemon IPC 协议版本不匹配",
                )]);
            }
            return IpcDispatch::complete(vec![answer_question_ipc_response(
                &answer_service,
                &request.request_id,
                lane_id,
                call_id,
                answers.clone(),
                channel,
            )]);
        }
        let drain_requested =
            matches!(&request.command, axon_protocol::DaemonCommand::Drain { .. });
        if drain_requested {
            let _ = handler_host.request_drain();
            let _ = handler_gateway.begin_drain();
        }
        let wake_primary_turn = matches!(
            &request.command,
            axon_protocol::DaemonCommand::SubmitInteraction(_)
        );
        let wake_maintenance = matches!(
            &request.command,
            axon_protocol::DaemonCommand::UpsertSchedule { .. }
                | axon_protocol::DaemonCommand::Drain { .. }
                | axon_protocol::DaemonCommand::GoalTool(_)
        );
        let responses = handler_host.handle_ipc_request(request);
        if wake_primary_turn {
            primary_turn_signal.notify();
        }
        if wake_maintenance {
            maintenance_signal.notify();
        }
        responses
    };
    let served = match stop_requested {
        Some(stop_requested) => server
            .serve_forever_streaming_until(handler, move || stop_requested.load(Ordering::Acquire)),
        None => server.serve_forever_streaming(handler),
    };
    let _ = host.request_drain();
    let _ = gateway_kernel.begin_drain();
    served.map_err(|error| error.to_string())
}

/// 处理显式 MCP 刷新请求：清除健康负缓存，作废并在后台重建该 Workspace 的启动快照。
///
/// 探测本身（握手 + `tools/list`）在后台线程执行，IPC 不等待：下一次 turn 的
/// `acquire` 会等这次构建结束后直接复用结果。
fn handle_mcp_refresh_request(
    context: &McpRefreshContext,
    request_id: &str,
    cwd: &str,
) -> Vec<axon_protocol::DaemonResponse> {
    let workspace = cwd.trim();
    if workspace.is_empty() {
        return vec![ipc_error_response(request_id, "cwd 不能为空")];
    }
    let cleared = axon_config::runtime_axon_root_path(&context.config_path)
        .map(|root| axon_mcp_runtime::clear_runtime_mcp_health_cache(&root))
        .unwrap_or(0);
    context
        .snapshots
        .refresh_in_background(context.config_path.clone(), PathBuf::from(workspace));
    vec![axon_protocol::DaemonResponse::success(
        request_id,
        serde_json::json!({
            "refreshed": true,
            "cwd": workspace,
            "mcpHealthCacheCleared": cleared,
        }),
    )]
}

fn start_gateway_shutdown_watcher(
    endpoint: &str,
    host: CliDaemonHost,
    gateway_kernel: Arc<GatewayKernel>,
    stop_requested: Arc<AtomicBool>,
) {
    let endpoint = endpoint.to_owned();
    thread::spawn(move || {
        while !stop_requested.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(SERVICE_STOP_WAKEUP_POLL_MS));
        }
        let _ = host.request_drain();
        let _ = gateway_kernel.begin_drain();
        let _ = JsonlIpcClient::connect(&endpoint);
    });
}

#[cfg(test)]
#[path = "daemon_host_runner_tests.rs"]
mod tests;
