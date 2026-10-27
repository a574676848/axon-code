//! 浏览器调试桥的命令分派表。
//!
//! 与 HTTP 服务本体分开：分派表逐个命令地长，与监听、路由、静态文件无关，
//! 各自成篇才不会互相挤到行数硬限之外。命令按领域分组，与桌面宿主注册的
//! `tauri::command` 一一对应：同一个名字在两条通道上调的是同一份实现。

use serde_json::Value;

use crate::catalog::{self, ModelInput, ProviderInput};
use crate::conversation;
use crate::ipc;
use crate::lane_source::GatewayLaneSource;
use crate::workspace;

/// 组合根构造的车道数据源：调试桥与桌面窗口共用同一份真实来源。
fn lane_source() -> Result<GatewayLaneSource, String> {
    Ok(GatewayLaneSource::new())
}

/// 把命令名与参数派发到真实实现。
pub(crate) fn dispatch(command: &str, args: &Value) -> Result<Value, String> {
    let source = lane_source()?;
    let as_str = |key: &str| -> Result<String, String> {
        args.get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| format!("参数 {key} 缺失或类型错误"))
    };
    let as_bool = |key: &str| -> Result<bool, String> {
        args.get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("参数 {key} 缺失或类型错误"))
    };
    let optional_string = |key: &str| -> Option<String> {
        args.get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    };

    match command {
        "config_read" => Ok(serde_json::to_value(ipc::config_read()?).map_err(stringify)?),
        "config_validate" => Ok(serde_json::to_value(ipc::config_validate(as_str(
            "toml_content",
        )?))
        .map_err(stringify)?),
        "config_write" => Ok(
            serde_json::to_value(ipc::config_write(as_str("toml_content")?)).map_err(stringify)?,
        ),
        "config_apply_values" => {
            let values: std::collections::HashMap<String, String> = args
                .get("values")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .ok_or_else(|| "参数 values 缺失或类型错误".to_string())?;
            Ok(serde_json::to_value(ipc::config_apply_values(values)?).map_err(stringify)?)
        }
        "config_reset_key" => {
            Ok(serde_json::to_value(ipc::config_reset_key(as_str("key")?)).map_err(stringify)?)
        }
        "config_validate_values" => {
            let values: std::collections::HashMap<String, String> = args
                .get("values")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .ok_or_else(|| "参数 values 缺失或类型错误".to_string())?;
            Ok(serde_json::to_value(ipc::config_validate_values(values)).map_err(stringify)?)
        }
        "voice_presets" => Ok(serde_json::to_value(ipc::voice_presets()).map_err(stringify)?),
        "voice_tts" => {
            let text = args
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned();
            let preset_id = args
                .get("presetId")
                .and_then(|value| value.as_str())
                .map(str::to_owned);
            Ok(serde_json::to_value(ipc::voice_tts(text, preset_id)).map_err(stringify)?)
        }
        "open_config_file" => {
            ipc::open_config_file()?;
            Ok(Value::Null)
        }
        "provider_profile_upsert" => {
            let input: ProviderInput = args
                .get("input")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .ok_or_else(|| "参数 input 缺失或类型错误".to_string())?;
            let models: Vec<ModelInput> = args
                .get("models")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .ok_or_else(|| "参数 models 缺失或类型错误".to_string())?;
            Ok(serde_json::to_value(catalog::provider_profile_upsert(
                input,
                models,
                optional_string("originalName"),
            )?)
            .map_err(stringify)?)
        }
        "provider_profile_remove" => Ok(serde_json::to_value(catalog::provider_profile_remove(
            as_str("name")?,
        )?)
        .map_err(stringify)?),
        "discover_models" => Ok(serde_json::to_value(crate::discovery::discover_models(
            optional_string("provider"),
            optional_string("baseUrl"),
            optional_string("protocol"),
            optional_string("apiKeyEnv"),
        )?)
        .map_err(stringify)?),
        "workspace_list" => {
            Ok(serde_json::to_value(workspace::workspace_list_at(&source)?).map_err(stringify)?)
        }
        "workspace_add" => Ok(serde_json::to_value(workspace::execute_workspace_add(
            &source,
            &as_str("path")?,
        )?)
        .map_err(stringify)?),
        "workspace_remove" => Ok(serde_json::to_value(workspace::execute_workspace_remove(
            &source,
            &as_str("id")?,
        )?)
        .map_err(stringify)?),
        "lane_create" => Ok(serde_json::to_value(workspace::execute_lane_create(
            &source,
            &as_str("workspaceId")?,
        )?)
        .map_err(stringify)?),
        "lane_set_pinned" => Ok(serde_json::to_value(workspace::execute_lane_set_pinned(
            &source,
            &as_str("laneId")?,
            as_bool("pinned")?,
        )?)
        .map_err(stringify)?),
        "lane_set_archived" => Ok(serde_json::to_value(workspace::execute_lane_set_archived(
            &source,
            &as_str("laneId")?,
            as_bool("archived")?,
        )?)
        .map_err(stringify)?),
        "lane_rename" => Ok(serde_json::to_value(workspace::execute_lane_rename(
            &source,
            &as_str("laneId")?,
            &as_str("title")?,
        )?)
        .map_err(stringify)?),
        "lane_reorder" => Ok(serde_json::to_value(workspace::execute_lane_reorder(
            &source,
            &as_str("workspaceId")?,
            args.get("orderedIds")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .ok_or_else(|| "参数 orderedIds 缺失或类型错误".to_string())?,
        )?)
        .map_err(stringify)?),
        "workspace_list_entries" => Ok(serde_json::to_value(crate::workspace_files::list_entries(
            &as_str("root")?,
            optional_string("relativePath")
                .as_deref()
                .unwrap_or_default(),
        )?)
        .map_err(stringify)?),
        "workspace_read_file" => Ok(serde_json::to_value(crate::workspace_files::read_text(
            &as_str("root")?,
            &as_str("relativePath")?,
        )?)
        .map_err(stringify)?),
        "workspace_read_bytes" => Ok(serde_json::to_value(crate::workspace_files::read_bytes(
            &as_str("root")?,
            &as_str("relativePath")?,
        )?)
        .map_err(stringify)?),
        "workspace_stat_path" => Ok(serde_json::to_value(crate::workspace_files::stat_path(
            &as_str("root")?,
            &as_str("relativePath")?,
        ))
        .map_err(stringify)?),
        "workspace_applications" => Ok(serde_json::to_value(crate::workspace_apps::applications(
            &as_str("root")?,
            &as_str("relativePath")?,
        )?)
        .map_err(stringify)?),
        "workspace_open_default" => {
            crate::workspace_files::open_default(&as_str("root")?, &as_str("relativePath")?)?;
            Ok(Value::Null)
        }
        "workspace_open_with" => {
            crate::workspace_apps::open_with(
                &as_str("root")?,
                &as_str("relativePath")?,
                &as_str("application")?,
            )?;
            Ok(Value::Null)
        }
        "workspace_reveal" => {
            crate::workspace_files::reveal(&as_str("root")?, &as_str("relativePath")?)?;
            Ok(Value::Null)
        }
        "desktop_preferences" => Ok(serde_json::to_value(
            crate::desktop_preferences::desktop_preferences()?,
        )
        .map_err(stringify)?),
        "desktop_set_notifications" => Ok(serde_json::to_value(
            crate::desktop_preferences::desktop_set_notifications(as_bool("enabled")?)?,
        )
        .map_err(stringify)?),
        "desktop_set_theme" => Ok(serde_json::to_value(
            crate::desktop_preferences::desktop_set_theme(as_str("theme")?)?,
        )
        .map_err(stringify)?),
        "pick_directory" => Ok(Value::Null),
        "document_catalog" => {
            Ok(serde_json::to_value(crate::documents::document_catalog()?).map_err(stringify)?)
        }
        "input_history_list" => Ok(serde_json::to_value(
            crate::input_history::list_input_history()?
        )
        .map_err(stringify)?),
        "input_history_append" => Ok(serde_json::to_value(
            crate::input_history::append_input_history(&as_str("text")?)?,
        )
        .map_err(stringify)?),
        "open_document" => {
            crate::documents::open_document(as_str("id")?)?;
            Ok(Value::Null)
        }
        "open_external_url" => {
            crate::process::open_external_url(as_str("url")?)?;
            Ok(Value::Null)
        }
        "mcp_list" => Ok(serde_json::to_value(crate::documents::mcp_list()?).map_err(stringify)?),
        "mcp_upsert" => {
            let input: crate::documents::McpServerInput = args
                .get("input")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .ok_or_else(|| "参数 input 缺失或类型错误".to_string())?;
            crate::documents::mcp_upsert(input, optional_string("originalName"))?;
            Ok(Value::Null)
        }
        "mcp_remove" => {
            crate::documents::mcp_remove(as_str("name")?)?;
            Ok(Value::Null)
        }
        "mcp_upsert_raw" => {
            crate::documents::mcp_upsert_raw(as_str("name")?, as_str("entryJson")?)?;
            Ok(Value::Null)
        }
        "mcp_runtime_status" => crate::mcp_runtime::load_status(&as_str("cwd")?),
        "mcp_runtime_refresh" => crate::mcp_runtime::reload(&as_str("cwd")?),
        "skills_list" => {
            Ok(serde_json::to_value(crate::documents::skills_list()?).map_err(stringify)?)
        }
        "document_read" => Ok(serde_json::to_value(crate::documents::document_read(as_str(
            "id",
        )?)?)
        .map_err(stringify)?),
        "document_write" => {
            crate::documents::document_write(as_str("id")?, as_str("content")?)?;
            Ok(Value::Null)
        }
        "secret_status_of" => Ok(
            serde_json::to_value(crate::secrets::secret_status_of(as_str("key")?)?)
                .map_err(stringify)?,
        ),
        "secret_set" => {
            crate::secrets::secret_set(as_str("key")?, as_str("value")?)?;
            Ok(Value::Null)
        }
        "secret_clear" => {
            crate::secrets::secret_clear(as_str("key")?)?;
            Ok(Value::Null)
        }
        "system_info" => {
            Ok(serde_json::to_value(crate::system_info::system_info()?).map_err(stringify)?)
        }
        "check_update" => {
            Ok(serde_json::to_value(crate::release_channel::check_update()?).map_err(stringify)?)
        }
        "gateway_health" => Ok(
            serde_json::to_value(crate::service_recovery::gateway_health_sync())
                .map_err(stringify)?,
        ),
        "gateway_start_service" => {
            let mode = args
                .get("mode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("start")
                .to_string();
            Ok(
                serde_json::to_value(crate::service_recovery::gateway_start_service_sync(mode))
                    .map_err(stringify)?,
            )
        }
        "cron_list" => Ok(crate::automation::cron_list(
            args.get("page")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            args.get("pageSize")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            optional_string("status"),
            optional_string("search"),
        )?),
        "cron_config" => Ok(crate::automation::cron_config()?),
        "cron_get" => Ok(crate::automation::cron_get(as_str("jobId")?)?),
        "cron_history" => Ok(crate::automation::cron_history(
            as_str("jobId")?,
            args.get("page")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            args.get("pageSize")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
        )?),
        "cron_create" => Ok(crate::automation::cron_create(
            args.get("input")
                .cloned()
                .ok_or_else(|| "参数 input 缺失或类型错误".to_string())?,
        )?),
        "cron_update" => Ok(crate::automation::cron_update(
            as_str("jobId")?,
            args.get("input")
                .cloned()
                .ok_or_else(|| "参数 input 缺失或类型错误".to_string())?,
        )?),
        "cron_pause" => Ok(crate::automation::cron_pause(as_str("jobId")?)?),
        "cron_resume" => Ok(crate::automation::cron_resume(as_str("jobId")?)?),
        "cron_run" => Ok(crate::automation::cron_run(as_str("jobId")?)?),
        "cron_remove" => Ok(crate::automation::cron_remove(as_str("jobId")?)?),
        "memory_list" => Ok(crate::automation::memory_list(
            as_str("memoryKind")?.to_owned(),
            optional_string("scope"),
            args.get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            args.get("cursorUpdatedAtMs").and_then(Value::as_u64),
            optional_string("cursorMemoryId"),
        )?),
        "memory_stats" => Ok(crate::automation::memory_stats(optional_string("scope"))?),
        "memory_scopes" => Ok(crate::automation::memory_scopes()?),
        "memory_get" => Ok(crate::automation::memory_get(
            as_str("memoryId")?.to_owned(),
        )?),
        "memory_write" => Ok(crate::automation::memory_write(
            args.get("input")
                .cloned()
                .ok_or_else(|| "参数 input 缺失".to_owned())?,
        )?),
        "memory_forget" => Ok(crate::automation::memory_forget(
            as_str("memoryId")?.to_owned(),
        )?),
        "memory_remove" => Ok(crate::automation::memory_remove(
            as_str("memoryId")?.to_owned(),
        )?),
        // 用量统计：浏览器调试桥与桌面宿主走同一批命令，只读。
        "usage_overview" => Ok(crate::usage::usage_overview(
            optional_string("period"),
            optional_string("provider"),
            optional_string("model"),
            optional_string("source"),
        )?),
        "usage_distribution" => Ok(crate::usage::usage_distribution(
            optional_string("dimension"),
            optional_string("period"),
        )?),
        "usage_diagnostics" => Ok(crate::usage::usage_diagnostics(optional_string("period"))?),
        "usage_realtime" => Ok(crate::usage::usage_realtime(optional_string("window"))?),
        "usage_events" => Ok(crate::usage::usage_events(
            optional_string("model"),
            optional_string("source"),
            optional_string("status"),
            optional_string("cursor"),
            args.get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
        )?),
        // 模型定价：规则读写与自动同步状态，同样只经网关。
        "pricing_rules" => Ok(crate::pricing::pricing_rules()?),
        "pricing_save_rule" => Ok(crate::pricing::pricing_save_rule(
            args.get("payload")
                .cloned()
                .ok_or_else(|| "参数 payload 缺失".to_owned())?,
        )?),
        "pricing_delete_rule" => Ok(crate::pricing::pricing_delete_rule(as_str("ruleId")?)?),
        "pricing_sync" => Ok(crate::pricing::pricing_sync()?),
        "pricing_recalculate" => Ok(crate::pricing::pricing_recalculate()?),
        "pricing_auto_sync" => Ok(crate::pricing::pricing_auto_sync()?),
        "pricing_set_auto_sync" => Ok(crate::pricing::pricing_set_auto_sync(as_bool("enabled")?)?),
        "conversation_submit" => Ok(serde_json::to_value(conversation::submit::submit_message(
            &as_str("workspaceId")?,
            &as_str("destination")?,
            &as_str("content")?,
            &args
                .get("attachments")
                .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
                .unwrap_or_default(),
        )?)
        .map_err(stringify)?),
        "conversation_answer_question" => {
            let answers = args
                .get("answers")
                .and_then(|value| {
                    serde_json::from_value::<Vec<conversation::wire::UserAnswerItem>>(value.clone())
                        .ok()
                })
                .unwrap_or_default();
            Ok(
                serde_json::to_value(conversation::answer_question::answer_question(
                    &as_str("laneId")?,
                    &as_str("callId")?,
                    &answers,
                )?)
                .map_err(stringify)?,
            )
        }
        "conversation_history" => Ok(serde_json::to_value(
            conversation::history::lane_interactions(
                &as_str("laneId")?,
                args.get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                args.get("beforeMs")
                    .and_then(Value::as_u64)
                    .zip(args.get("beforeId").and_then(Value::as_str))
                    .map(
                        |(received_at_ms, interaction_id)| conversation::history::HistoryCursor {
                            received_at_ms,
                            interaction_id: interaction_id.to_owned(),
                        },
                    )
                    .as_ref(),
            )?,
        )
        .map_err(stringify)?),
        "conversation_history_events" => Ok(serde_json::to_value(
            conversation::history::interaction_events(
                &as_str("interactionId")?,
                args.get("after").and_then(Value::as_u64),
                args.get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
            )?,
        )
        .map_err(stringify)?),
        "conversation_watch" => {
            conversation::stream::shared().watch(conversation::SubscribeTarget::Interaction {
                workspace_id: as_str("workspaceId")?,
                interaction_id: as_str("interactionId")?,
                resume_cursor: optional_string("resumeCursor"),
            });
            Ok(Value::Null)
        }
        "conversation_watch_session" => {
            conversation::stream::shared().watch(conversation::SubscribeTarget::Session {
                workspace_id: as_str("workspaceId")?,
                session_id: as_str("sessionId")?,
                resume_cursor: optional_string("resumeCursor"),
            });
            Ok(Value::Null)
        }
        "conversation_unsubscribe" => {
            conversation::stream::shared().unsubscribe(&as_str("subscribeId")?);
            Ok(Value::Null)
        }
        "conversation_interrupt" => Ok(serde_json::to_value(
            conversation::interrupt::interrupt_lane(&as_str("laneId")?)?,
        )
        .map_err(stringify)?),
        "conversation_stop_job" => Ok(serde_json::to_value(conversation::job_stop::stop_job(
            &as_str("laneId")?,
            &as_str("jobId")?,
        )?)
        .map_err(stringify)?),
        "conversation_send_subagent_message" => Ok(serde_json::to_value(
            conversation::subagent_message::send_subagent_message(
                &as_str("laneId")?,
                &as_str("agentId")?,
                &as_str("content")?,
            )?,
        )
        .map_err(stringify)?),
        "conversation_lane_jobs" => Ok(serde_json::to_value(conversation::lane_jobs::lane_jobs(
            &as_str("laneId")?,
        )?)
        .map_err(stringify)?),
        "conversation_lane_subagents" => Ok(serde_json::to_value(
            conversation::session_read::lane_subagents(&as_str("laneId")?)?,
        )
        .map_err(stringify)?),
        "attachment_stage" => Ok(serde_json::to_value(crate::conversation::attachment::stage(
            &as_str("name")?,
            &as_str("dataBase64")?,
        )?)
        .map_err(stringify)?),
        other => Err(format!("未知命令：{other}")),
    }
}

/// 把任意可显示错误转为字符串。
pub(crate) fn stringify(error: impl std::fmt::Display) -> String {
    error.to_string()
}
