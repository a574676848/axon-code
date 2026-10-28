//! Axon Workbench 桌面端库。
//!
//! 提供基于 Tauri 2 的图形化窗口集成，以及对 Axon 配置系统与工作区、
//! 车道数据的权威 IPC 读写通道。

/// 自动化任务：Cron 管理命令，经 Gateway 访问 Axon 运行态。
pub mod automation;
/// 桌面设置弹窗读取与写入的配置项目录。
pub mod catalog;
/// 剪贴板：原生通道读写，不经网页剪贴板权限。
pub mod clipboard;
/// 会话：提交、订阅与历史索引。
pub mod conversation;
/// 桌面级偏好的 IPC：目前只有通知开关。
pub mod desktop_preferences;
/// 桌面外壳：系统托盘、关闭最小化与系统通知。
pub mod desktop_shell;
/// 开发期本地调试桥。
pub mod dev_server;
/// 调试桥的命令分派表。
mod dev_server_dispatch;
/// 上游供应商模型列表发现。
pub mod discovery;
/// 设置内的文档、目录与 MCP 配置管理。
pub mod documents;
/// 正文导出：另存为 Markdown 文件。
pub mod export;
/// 网关客户端：桌面端访问运行态的唯一入口。
pub mod gateway;
/// 目标看板：只读目标列表与详情的 IPC 命令。
pub mod goals;
/// 输入历史：与 TUI 共用 composer draft 历史。
pub mod input_history;
/// 基础配置权威 IPC 命令与数据模型。
pub mod ipc;
/// 当前会话上下文：通知与界面共用的会话归属。
pub mod lane_context;
/// 车道数据源与表现层端口。
pub mod lane_source;
/// MCP 运行态转发：真正的加载结论由 daemon 的启动快照给出。
pub mod mcp_runtime;
pub mod persona;
/// 原生目录选择器。
pub mod picker;
/// 模型定价：经网关读写价格规则。
pub mod pricing;
/// 子进程启动：统一隐藏控制台窗体。
pub mod process;
/// GitHub 发布渠道：仓库坐标与更新检测。
pub mod release_channel;
/// 密钥的可视化管理。
pub mod secrets;
/// 端到端往返自检逻辑。
pub mod self_check;
/// 网关可用性探测与服务修复引导。
pub mod service_recovery;
/// 桌面工作台自身的持久化状态。
pub mod state;
/// 系统信息。
pub mod system_info;
/// 会话工作区内的交互式终端。
pub mod terminal;
/// 界面进程失败时的兜底引导页。
pub mod ui_failure;
/// 用量统计：经网关读取可观测性接口。
pub mod usage;
/// 语音：音色试听与麦克风上行。
pub mod voice;
/// 操作系统级麦克风隐私探测与系统设置跳转。
pub mod voice_permission;
/// 窗口级系统材质、原生深浅色联动与最大化状态翻转。
pub mod window_effects;
/// 窗口几何记忆与静默呈现。
pub mod window_state;
/// 工作区与车道的真实扫描与状态维护。
pub mod workspace;
/// 「用其他应用打开」：按系统关联枚举候选并回放选中的一条。
pub mod workspace_apps;
/// 工作区文件的只读浏览。
pub mod workspace_files;

use automation::{
    cron_config, cron_create, cron_get, cron_history, cron_list, cron_pause, cron_remove,
    cron_resume, cron_run, cron_update, memory_forget, memory_get, memory_list, memory_remove,
    memory_scopes, memory_stats, memory_write,
};
use catalog::{provider_profile_remove, provider_profile_upsert};
use clipboard::{clipboard_read_text, clipboard_write_text};
use conversation::{
    attachment_pick, attachment_preview, attachment_stage, conversation_answer_question,
    conversation_history, conversation_history_events, conversation_interrupt,
    conversation_lane_jobs, conversation_lane_questions, conversation_lane_subagents,
    conversation_send_subagent_message, conversation_set_active_lane, conversation_stop_job,
    conversation_submit, conversation_unsubscribe, conversation_watch, conversation_watch_session,
};
use desktop_preferences::{desktop_preferences, desktop_set_notifications, desktop_set_theme};
use discovery::discover_models;
use documents::{
    document_catalog, document_read, document_write, mcp_list, mcp_remove, mcp_upsert,
    mcp_upsert_raw, open_document, skills_list,
};
use export::export_markdown;
use goals::{goal_get, goal_list};
use input_history::{input_history_append, input_history_list};
use ipc::{
    config_apply_values, config_read, config_reset_key, config_validate, config_validate_values,
    config_write, open_config_file, voice_presets, voice_runtime_status, voice_tts,
};
use lane_source::GatewayLaneSource;
use mcp_runtime::{mcp_runtime_refresh, mcp_runtime_status};
use persona::{persona_portrait, persona_summary};
use picker::pick_directory;
use pricing::{
    pricing_auto_sync, pricing_delete_rule, pricing_recalculate, pricing_rules, pricing_save_rule,
    pricing_set_auto_sync, pricing_sync,
};
use process::open_external_url;
use release_channel::check_update;
use secrets::{secret_clear, secret_set, secret_status_of};
use service_recovery::{gateway_health, gateway_start_service};
use std::sync::Arc;
use system_info::system_info;
use tauri::Emitter;
use terminal::{terminal_close, terminal_open, terminal_resize, terminal_shells, terminal_write};
use usage::{usage_diagnostics, usage_distribution, usage_events, usage_overview, usage_realtime};
use voice::{
    voice_audio, voice_cancel, voice_playback_drained, voice_preview, voice_start, voice_stop,
};
use voice_permission::{voice_microphone_status, voice_open_microphone_settings};
use workspace::{
    lane_create, lane_rename, lane_reorder, lane_set_archived, lane_set_pinned, workspace_add,
    workspace_list, workspace_remove,
};
use workspace_apps::{workspace_applications, workspace_open_with};
use workspace_files::{
    workspace_list_entries, workspace_open_default, workspace_read_bytes, workspace_read_file,
    workspace_reveal, workspace_stat_path,
};

/// 桌面应用共享状态：组合根装配好的车道数据源。
pub struct AppState {
    /// 表现层消费的车道数据端口。
    pub lanes: Arc<dyn lane_source::LaneSource>,
    /// 会话事件订阅器：一条连接并行服务多路订阅（进程内唯一实例）。
    pub conversation: Arc<conversation::ConversationStream>,
}

/// 启动 Axon 桌面应用。
pub fn run() {
    let state = AppState {
        lanes: Arc::new(GatewayLaneSource::new()),
        conversation: conversation::stream::shared().clone(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // 窗口状态插件只恢复尺寸、位置、最大化与全屏四项。
        // 可见性必须排除在外：插件默认会在恢复几何时顺手 show 一次，
        // 那一刻前端还没画完第一帧，用户看到的就是白底加一次位置跳动。
        .plugin(window_state::plugin())
        // 单实例必须排在最前：第二次启动时它负责唤回既有窗口后立即退出，
        // 否则用户每点一次图标就会多出一个托盘图标与一份后台进程。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            desktop_shell::show_main_window(app);
        }))
        // 附件预览走自定义协议：字节由 webview 直接取，不再经 IPC 传 base64，
        // 省掉三分之一的体积膨胀，也让浏览器能自己缓存。
        .register_uri_scheme_protocol(
            conversation::attachment::ATTACHMENT_SCHEME,
            |_ctx, request| {
                let path =
                    conversation::attachment::path_from_scheme_uri(&request.uri().to_string())
                        .and_then(|path| conversation::attachment::read_preview(&path));
                match path {
                    Ok((bytes, mime)) => tauri::http::Response::builder()
                        .status(200)
                        .header(tauri::http::header::CONTENT_TYPE, mime)
                        .header(tauri::http::header::CACHE_CONTROL, "no-store")
                        .body(bytes)
                        .unwrap_or_else(|_| tauri::http::Response::new(Vec::new())),
                    Err(_) => tauri::http::Response::builder()
                        .status(404)
                        .body(Vec::new())
                        .unwrap_or_else(|_| tauri::http::Response::new(Vec::new())),
                }
            },
        )
        .setup(|app| {
            // 注入应用句柄，使「打开文件/目录」走官方插件而不是自行拼系统命令。
            process::install_opener(app.handle().clone());
            // 会话事件经 Tauri 事件通道下发；订阅器是进程内唯一实例，
            // 并且不引用任何界面类型，界面只是它的一个消费者。
            // 车道流与子代理会话流共用这一条通道，帧内自带 subscribeId 由前端分发。
            // 投递按窗口攒批：历史回放一次上万帧，逐帧 emit 会让 WebView 跟不上，
            // 丢掉的历史帧不会重来，界面就一直停在「生成中」。
            let emit_handle = app.handle().clone();
            let batcher =
                conversation::stream_batch::StreamBatcher::start(Arc::new(move |batch| {
                    let _ = emit_handle.emit(conversation::stream::CONVERSATION_EVENT, batch);
                }));
            let notify_handle = app.handle().clone();
            conversation::stream::shared().install_emitter(Arc::new(move |event| {
                // 完成通知是逐条事实，与投递频率无关，仍然按帧判定。
                desktop_shell::notify_turn_finished(&notify_handle, &event);
                batcher.push(event);
            }));
            // 托盘与关闭行为在窗口建好之后安装，此时主窗口已可解析。
            desktop_shell::install(app.handle())?;
            // 材质底座在建窗之后、呈现之前建立：前端的首帧就绪信号要能问到结论。
            window_effects::install(app.handle());
            window_effects::init_window_material(app.handle());
            // 兜底呈现：前端首帧就绪信号与 350ms 超时，两条路只会显示一次。
            window_state::install_show_gate(app.handle());
            Ok(())
        })
        .manage(state)
        // 终端注册表由读取线程共享（线程退出时要回来摘掉自己那一格），因此挂在应用上。
        .manage(Arc::new(terminal::TerminalRegistry::default()))
        .invoke_handler(tauri::generate_handler![
            config_read,
            config_validate,
            config_write,
            config_apply_values,
            config_reset_key,
            config_validate_values,
            open_config_file,
            open_external_url,
            voice_presets,
            voice_runtime_status,
            voice_tts,
            voice_preview,
            voice_start,
            voice_audio,
            voice_stop,
            voice_cancel,
            voice_playback_drained,
            voice_microphone_status,
            voice_open_microphone_settings,
            input_history_list,
            input_history_append,
            provider_profile_upsert,
            provider_profile_remove,
            discover_models,
            document_catalog,
            open_document,
            mcp_list,
            mcp_upsert,
            mcp_remove,
            mcp_upsert_raw,
            mcp_runtime_status,
            mcp_runtime_refresh,
            skills_list,
            document_read,
            document_write,
            persona_summary,
            persona_portrait,
            secret_status_of,
            secret_set,
            secret_clear,
            system_info,
            check_update,
            gateway_health,
            gateway_start_service,
            cron_list,
            cron_config,
            cron_get,
            cron_history,
            cron_create,
            cron_update,
            cron_pause,
            cron_resume,
            cron_run,
            cron_remove,
            memory_list,
            memory_stats,
            memory_scopes,
            memory_get,
            memory_write,
            memory_forget,
            memory_remove,
            goal_list,
            goal_get,
            workspace_list,
            workspace_add,
            workspace_remove,
            pick_directory,
            lane_create,
            lane_set_pinned,
            lane_set_archived,
            lane_rename,
            lane_reorder,
            workspace_list_entries,
            workspace_stat_path,
            workspace_read_file,
            workspace_read_bytes,
            workspace_open_default,
            workspace_applications,
            workspace_open_with,
            workspace_reveal,
            terminal_shells,
            terminal_open,
            terminal_write,
            terminal_resize,
            terminal_close,
            conversation_submit,
            conversation_answer_question,
            conversation_set_active_lane,
            conversation_history,
            conversation_history_events,
            conversation_lane_questions,
            conversation_watch,
            conversation_watch_session,
            conversation_unsubscribe,
            conversation_interrupt,
            conversation_stop_job,
            conversation_send_subagent_message,
            conversation_lane_jobs,
            conversation_lane_subagents,
            attachment_stage,
            attachment_pick,
            attachment_preview,
            export_markdown,
            clipboard_read_text,
            clipboard_write_text,
            desktop_preferences,
            desktop_set_notifications,
            desktop_set_theme,
            window_state::desktop_notify_ready,
            usage_overview,
            usage_distribution,
            usage_diagnostics,
            usage_realtime,
            usage_events,
            pricing_rules,
            pricing_save_rule,
            pricing_delete_rule,
            pricing_sync,
            pricing_recalculate,
            pricing_auto_sync,
            pricing_set_auto_sync,
        ])
        .run(tauri::generate_context!())
        .expect("运行 Axon 桌面工作台失败");
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
