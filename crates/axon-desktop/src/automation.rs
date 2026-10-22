use serde_json::{Value, json};

use crate::gateway;

fn path(job_id: &str, suffix: &str) -> String {
    format!("/crons/{job_id}{suffix}")
}

const DEFAULT_PAGE_SIZE: usize = 20;
const MAX_JOB_PAGE_SIZE: usize = 500;
const MAX_HISTORY_PAGE_SIZE: usize = 200;

/// 列出可见 Cron 任务。
#[tauri::command]
pub fn cron_list(
    page: Option<usize>,
    page_size: Option<usize>,
    status: Option<String>,
    search: Option<String>,
) -> Result<Value, String> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("page", &page.unwrap_or(1).max(1).to_string())
        .append_pair(
            "pageSize",
            &page_size
                .unwrap_or(DEFAULT_PAGE_SIZE)
                .clamp(1, MAX_JOB_PAGE_SIZE)
                .to_string(),
        )
        .append_pair("status", status.as_deref().unwrap_or_default())
        .append_pair("search", search.as_deref().unwrap_or_default())
        .finish();
    gateway::get_json(&format!("/crons?{query}"))
}

/// 读取 Cron 运行配置。
#[tauri::command]
pub fn cron_config() -> Result<Value, String> {
    gateway::get_json("/crons/config")
}

/// 读取单个 Cron 任务。
#[tauri::command]
pub fn cron_get(job_id: String) -> Result<Value, String> {
    gateway::get_json(&path(&job_id, ""))
}

/// 读取单个 Cron 的执行历史。
#[tauri::command]
pub fn cron_history(
    job_id: String,
    page: Option<usize>,
    page_size: Option<usize>,
) -> Result<Value, String> {
    let page = page.unwrap_or(1).max(1);
    let page_size = page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_HISTORY_PAGE_SIZE);
    gateway::get_json(&format!(
        "{}?page={page}&pageSize={page_size}",
        path(&job_id, "/runs")
    ))
}

/// 创建 Cron 任务。
#[tauri::command]
pub fn cron_create(input: Value) -> Result<Value, String> {
    gateway::post_json("/crons", &input)
}

/// 更新 Cron 任务。
#[tauri::command]
pub fn cron_update(job_id: String, input: Value) -> Result<Value, String> {
    gateway::post_json(&path(&job_id, ""), &input)
}

/// 暂停 Cron 任务。
#[tauri::command]
pub fn cron_pause(job_id: String) -> Result<Value, String> {
    gateway::post_json(&path(&job_id, "/pause"), &json!({}))
}

/// 恢复 Cron 任务。
#[tauri::command]
pub fn cron_resume(job_id: String) -> Result<Value, String> {
    gateway::post_json(&path(&job_id, "/resume"), &json!({}))
}

/// 立即执行 Cron 任务。
#[tauri::command]
pub fn cron_run(job_id: String) -> Result<Value, String> {
    gateway::post_json(&path(&job_id, "/run"), &json!({}))
}

/// 移除用户 Cron 任务；系统 Cron 会由网关拒绝。
#[tauri::command]
pub fn cron_remove(job_id: String) -> Result<Value, String> {
    gateway::delete_json(&path(&job_id, ""))
}

/// 列出当前可见面的记忆时间轴页面。
#[tauri::command]
pub fn memory_list(
    memory_kind: String,
    scope: Option<String>,
    limit: Option<usize>,
    cursor_updated_at_ms: Option<u64>,
    cursor_memory_id: Option<String>,
) -> Result<Value, String> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("memoryKind", memory_kind.trim());
    if let Some(scope) = scope
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.append_pair("scope", scope);
    }
    let page_size = limit.unwrap_or(50).clamp(1, 100).to_string();
    query.append_pair("limit", &page_size);
    if let Some(value) = cursor_updated_at_ms {
        query.append_pair("cursorUpdatedAtMs", &value.to_string());
    }
    if let Some(value) = cursor_memory_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.append_pair("cursorMemoryId", value);
    }
    gateway::get_json(&format!("/memory?{}", query.finish()))
}

/// 读取当前可见面的记忆统计。
#[tauri::command]
pub fn memory_stats(scope: Option<String>) -> Result<Value, String> {
    match scope
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(scope) => gateway::get_json(&format!(
            "/memory/stats?scope={}",
            url::form_urlencoded::byte_serialize(scope.as_bytes()).collect::<String>()
        )),
        None => gateway::get_json("/memory/stats"),
    }
}

/// 列出当前可见面实际存在的记忆 scope。
#[tauri::command]
pub fn memory_scopes() -> Result<Value, String> {
    gateway::get_json("/memory/scopes")
}

/// 读取单条记忆详情。
#[tauri::command]
pub fn memory_get(memory_id: String) -> Result<Value, String> {
    gateway::get_json(&format!("/memory/{memory_id}"))
}

/// 创建或通过 supersedes 修改记忆；`input` 原样透传网关校验。
#[tauri::command]
pub fn memory_write(input: Value) -> Result<Value, String> {
    gateway::post_json("/memory", &input)
}

/// 遗忘记忆并写入 tombstone。
#[tauri::command]
pub fn memory_forget(memory_id: String) -> Result<Value, String> {
    gateway::post_json(&format!("/memory/{memory_id}/forget"), &json!({}))
}

/// 物理删除记忆。
#[tauri::command]
pub fn memory_remove(memory_id: String) -> Result<Value, String> {
    gateway::delete_json(&format!("/memory/{memory_id}"))
}
