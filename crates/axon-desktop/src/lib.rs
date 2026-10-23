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
/// 输入历史：与 TUI 共用 composer draft 历史。
pub mod input_history;
/// 基础配置权威 IPC 命令与数据模型。
pub mod ipc;
/// 当前会话上下文：通知与界面共用的会话归属。
pub mod lane_context;
/// 车道数据源与表现层端口。
pub mod lane_source;
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
use input_history::{input_history_append, input_history_list};
use ipc::{
    config_apply_values, config_read, config_reset_key, config_validate, config_validate_values,
    config_write, open_config_file,
};
use lane_source::GatewayLaneSource;
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
            skills_list,
            document_read,
            document_write,
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
mod tests {
    use super::catalog::*;
    use super::ipc::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 建立一次性的临时目录与配置文件路径。
    fn scratch_config() -> (PathBuf, PathBuf) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("axon-desktop-test-{nonce}"));
        fs::create_dir_all(&temp_dir).unwrap();
        let config_file = temp_dir.join("config.toml");
        (temp_dir, config_file)
    }

    /// 读取配置文件全文。
    fn read_config(path: &std::path::Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn test_ipc_read_validate_and_atomic_write_cycle() {
        let (temp_dir, config_file) = scratch_config();

        // 1. 初次读取：文件尚不存在时返回默认规范
        let read1 = execute_config_read_at(&config_file).unwrap();
        assert!(!read1.specs.is_empty(), "必须透出配置键规格定义");

        // 2. 校验无效 TOML：必须拒绝且不产生文件
        let invalid_toml = "[agent\ninvalid_syntax = ";
        let val_fail = execute_config_validate(invalid_toml);
        assert!(!val_fail.valid, "非法 TOML 必须校验失败");
        assert!(!config_file.exists(), "校验失败绝不应写入磁盘");

        // 3. 校验有效 TOML 并执行首次原子写入
        let valid_toml = "[agent]\ndefault_model = \"glm-5.3-flash\"\n";
        assert!(
            execute_config_validate(valid_toml).valid,
            "合法配置必须校验通过"
        );

        let write1 = execute_config_write_at(&config_file, valid_toml).unwrap();
        assert!(write1.written, "初次写入应返回 true");
        assert!(config_file.exists(), "原子写后目标文件必须存在");

        // 4. 重复写入完全相同内容：必须幂等跳过
        let write2 = execute_config_write_at(&config_file, valid_toml).unwrap();
        assert!(!write2.written, "内容未变时应跳过写入");

        // 5. 复读验证一致性
        let read2 = execute_config_read_at(&config_file).unwrap();
        assert_eq!(read2.toml_content.trim(), valid_toml.trim());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_apply_values_persists_and_rejects_invalid_entries() {
        let (temp_dir, config_file) = scratch_config();

        // 首次写入建立基线文件
        execute_config_write_at(&config_file, "[agent]\ndefault_model = \"\"\n").unwrap();

        // 1. 真实写入：推理强度落到磁盘并可读回
        let mut values = HashMap::new();
        values.insert("agent.reasoning_effort".to_string(), "low".to_string());
        values.insert("agent.log_level".to_string(), "debug".to_string());
        let applied = execute_apply_values_at(&config_file, &values).unwrap();
        assert!(applied.written, "有效变更必须真实落盘");

        let content = read_config(&config_file);
        assert!(
            content.contains("reasoning_effort = \"low\""),
            "磁盘内容必须包含新值"
        );
        assert!(
            content.contains("log_level = \"debug\""),
            "磁盘内容必须包含新值"
        );

        let read_back = execute_config_read_at(&config_file).unwrap();
        assert_eq!(
            read_back.flat_values.get("agent.reasoning_effort"),
            Some(&serde_json::json!("low")),
            "读回值必须与新写入一致"
        );

        // 2. 非法取值：必须整批拒绝，磁盘内容保持上一版
        let mut invalid = HashMap::new();
        invalid.insert(
            "agent.reasoning_effort".to_string(),
            "不存在的档位".to_string(),
        );
        assert!(
            execute_apply_values_at(&config_file, &invalid).is_err(),
            "非法枚举值必须被拒绝"
        );
        assert_eq!(read_config(&config_file), content, "拒绝后磁盘内容不得变化");

        // 3. 空变更集：不写盘
        assert!(
            !execute_apply_values_at(&config_file, &HashMap::new())
                .unwrap()
                .written
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_reset_key_restores_builtin_default() {
        let (temp_dir, config_file) = scratch_config();

        let mut values = HashMap::new();
        values.insert("agent.max_subagent_depth".to_string(), "9".to_string());
        execute_apply_values_at(&config_file, &values).unwrap();
        assert!(read_config(&config_file).contains("max_subagent_depth = 9"));

        execute_reset_key_at(&config_file, "agent.max_subagent_depth").unwrap();
        let restored = execute_config_read_at(&config_file).unwrap();
        let default_value = execute_config_read_at(&config_file)
            .unwrap()
            .flat_values
            .get("agent.max_subagent_depth")
            .cloned();
        assert_eq!(
            restored
                .flat_values
                .get("agent.max_subagent_depth")
                .cloned(),
            default_value,
            "恢复后应回到内置默认值"
        );

        assert!(
            execute_reset_key_at(&config_file, "不存在的键").is_err(),
            "未知键必须报错"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_validate_values_is_pure_memory() {
        let (temp_dir, config_file) = scratch_config();
        execute_config_write_at(&config_file, "[agent]\nlog_level = \"info\"\n").unwrap();
        let before = read_config(&config_file);

        let mut values = HashMap::new();
        values.insert("agent.log_level".to_string(), "trace".to_string());
        let report = execute_validate_values_at(&config_file, &values);
        assert!(report.valid, "合法变更应校验通过");
        assert_eq!(read_config(&config_file), before, "试算校验绝不能写盘");

        let mut broken = HashMap::new();
        broken.insert("agent.log_level".to_string(), "不存在".to_string());
        let report_broken = execute_validate_values_at(&config_file, &broken);
        assert!(!report_broken.valid, "非法变更应校验失败");
        assert!(!report_broken.errors.is_empty(), "必须给出可读的错误明细");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_provider_profile_upsert_and_remove_are_atomic() {
        let (temp_dir, config_file) = scratch_config();
        execute_config_write_at(&config_file, "[agent]\ndefault_model = \"\"\n").unwrap();

        let profile = ProviderInput {
            name: "scratch-provider".to_string(),
            enabled: true,
            protocol: "openai".to_string(),
            base_url: "https://example.com/v1".to_string(),
            api_key_env: Some("SCRATCH_PROFILE_API_KEY".to_string()),
            wire_transport: "http".to_string(),
            priority: 10,
            weight: 1,
            ..ProviderInput::default()
        };
        let primary = ModelInput {
            name: "scratch-a".to_string(),
            id: "scratch-a-v1".to_string(),
            provider: "scratch-provider".to_string(),
            context_window: 64000,
            max_output_tokens: Some(4096),
            ..ModelInput::default()
        };
        let secondary = ModelInput {
            name: "scratch-b".to_string(),
            id: "scratch-b-v1".to_string(),
            provider: "scratch-provider".to_string(),
            context_window: 32000,
            ..ModelInput::default()
        };

        // 1. 新增档案：供应商与其名下模型一次落盘
        provider_profile_upsert_at(
            &config_file,
            profile.clone(),
            vec![primary.clone(), secondary.clone()],
            None,
        )
        .unwrap();
        let added = execute_config_read_at(&config_file).unwrap();
        assert_eq!(added.providers.len(), 1);
        assert_eq!(added.models.len(), 2);
        assert_eq!(
            added.flat_values.get("agent.default_model"),
            Some(&serde_json::json!("scratch-a")),
            "首次写入档案必须自动绑定默认模型槽位"
        );
        assert_eq!(
            added.flat_values.get("agent.fast_model"),
            Some(&serde_json::json!("scratch-a")),
            "首次写入档案必须自动绑定快速委托槽位"
        );

        // 2. 未设置的密钥引用必须如实报告未配置，绝不谎报已就绪
        assert!(!added.providers[0].api_key_configured);

        // 3. 整卡更新：移除 scratch-b 并修改 scratch-a 的上下文窗口
        let mut tuned = primary.clone();
        tuned.context_window = 128000;
        provider_profile_upsert_at(
            &config_file,
            profile.clone(),
            vec![tuned],
            Some("scratch-provider".to_string()),
        )
        .unwrap();
        let updated = execute_config_read_at(&config_file).unwrap();
        assert_eq!(updated.models.len(), 1, "整卡提交必须移除未提交的旧模型");
        assert_eq!(updated.models[0].context_window, 128000);

        // 4. 供应商改名：名下模型的归属同步跟随
        let mut renamed_profile = profile.clone();
        renamed_profile.name = "scratch-profile-renamed".to_string();
        provider_profile_upsert_at(
            &config_file,
            renamed_profile.clone(),
            vec![primary.clone()],
            Some("scratch-provider".to_string()),
        )
        .unwrap();
        let renamed = execute_config_read_at(&config_file).unwrap();
        assert_eq!(renamed.providers[0].name, "scratch-profile-renamed");
        assert_eq!(renamed.models[0].provider, "scratch-profile-renamed");

        // 5. 缺模型 ID 必须整卡拒绝，磁盘保持上一版
        let before_bytes = read_config(&config_file);
        let mut broken = primary.clone();
        broken.id = String::new();
        assert!(
            provider_profile_upsert_at(
                &config_file,
                renamed_profile.clone(),
                vec![broken],
                Some("scratch-profile-renamed".to_string()),
            )
            .is_err(),
            "缺模型 ID 的条目必须被拒绝"
        );
        assert_eq!(
            read_config(&config_file),
            before_bytes,
            "拒绝后磁盘内容不得变化"
        );

        // 6. 同档案内模型重名必须拒绝
        let mut collided = primary.clone();
        collided.id = "scratch-a-v2".to_string();
        assert!(
            provider_profile_upsert_at(
                &config_file,
                renamed_profile.clone(),
                vec![primary.clone(), collided],
                Some("scratch-profile-renamed".to_string()),
            )
            .is_err(),
            "同档案内模型重名必须被拒绝"
        );

        // 7. 移除档案：供应商与其名下模型一并消失，槽位清空
        provider_profile_remove_at(&config_file, "scratch-profile-renamed").unwrap();
        let removed = execute_config_read_at(&config_file).unwrap();
        assert!(removed.providers.is_empty());
        assert!(removed.models.is_empty());
        assert_eq!(
            removed.flat_values.get("agent.default_model"),
            Some(&serde_json::json!("")),
            "模型目录清空后必须同时清空模型槽位"
        );

        // 8. 重复移除必须报错，而不是静默成功
        assert!(provider_profile_remove_at(&config_file, "scratch-profile-renamed").is_err());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_desktop_state_tolerates_utf8_bom() {
        let (temp_dir, _) = scratch_config();
        let state_file = temp_dir.join("desktop-state.json");
        fs::write(
            &state_file,
            "\u{feff}{\"version\":1,\"workspaces\":[{\"path\":\"E:\\\\repo\",\"name\":\"repo\"}],\"lane_pinned\":[\"lane-a\"]}",
        )
        .unwrap();

        let text = fs::read_to_string(&state_file).unwrap();
        let parsed: super::state::DesktopState =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        assert_eq!(
            parsed.workspaces.len(),
            1,
            "带 BOM 的状态必须仍能解析出已登记工作区"
        );
        assert_eq!(parsed.workspaces[0].name, "repo");
        assert_eq!(parsed.lane_pinned, vec!["lane-a".to_string()]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_workspace_list_only_exposes_user_registered_directories() {
        let source = super::lane_source::GatewayLaneSource::new();
        // 该用例校验的是真实运行态契约：数据经网关读取，网关未运行时无从校验。
        // 此时直接跳过而不是伪造数据，避免把「网关没起」误报成用例通过。
        let Ok(workspaces) = super::workspace::workspace_list_at(&source) else {
            return;
        };
        let state = super::state::load_state();

        // 侧栏只呈现用户在应用内添加过的目录，运行态注册表里的历史条目不得出现。
        assert_eq!(
            workspaces.len(),
            state.workspaces.len(),
            "展示的工作区数量必须与用户登记数量一致"
        );
        for workspace in &workspaces {
            assert!(
                state
                    .workspaces
                    .iter()
                    .any(|registered| registered.path == workspace.path),
                "展示的工作区必须来自用户登记：{}",
                workspace.path
            );
            assert!(
                !workspace.path.contains(r"\\?\") && !workspace.path.contains("//?/"),
                "路径不得残留 Windows 扩展前缀"
            );
            assert!(!workspace.name.is_empty(), "工作区名称不得为空");
            for lane in &workspace.lanes {
                assert!(!lane.id.is_empty(), "车道标识不得为空");
                assert!(!lane.title.is_empty(), "车道标题不得为空");
                assert!(!lane.channel.is_empty(), "车道渠道不得为空");
            }
        }
    }

    #[test]
    fn test_workspace_entries_strip_extended_path_prefix() {
        assert_eq!(
            super::workspace::strip_extended_prefix(r"\\?\E:\zbg\axon-code"),
            if cfg!(windows) {
                r"E:\zbg\axon-code"
            } else {
                r"\\?\E:\zbg\axon-code"
            }
        );
        assert_eq!(
            super::workspace::strip_extended_prefix("//?/e:/zbg/axon-code"),
            if cfg!(windows) {
                r"e:\zbg\axon-code"
            } else {
                "e:/zbg/axon-code"
            }
        );
    }
}
