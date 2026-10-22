//! MCP runtime 健康负缓存。
//!
//! 负缓存分两个作用域，粒度与窗口都不同：
//!
//! - **server 级**（`tool` 缺省）：只由发现期故障写入：配置加载、握手、`tools/list`、
//!   空工具列表与 schema 注册失败；命中即整台 server 在窗口内不注册任何工具。
//! - **tool 级**（`tool` 有值）：只由 `tools/call` 失败写入，窗口更短，只挡住同一个工具，
//!   既不摘除同 server 的其他工具，也不改变任何工具注册表。
//!
//! 调用期失败不得写 server 级故障：把一次工具调用抖动放大成整台 server 不可用，
//! 会让模型可见工具表无谓收缩，也会让其他健康工具连带失败。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use axon_time::unix_now_ms;

/// server 级发现故障的负缓存窗口。
const MCP_SERVER_HEALTH_TTL_MS: u64 = 30_000;
/// 单个工具的调用失败冷却窗口。
///
/// 比 server 级更短：调用期失败是工具级信号，不能让一次超时长期挡住该工具修正后的重试。
const MCP_TOOL_COOLDOWN_TTL_MS: u64 = 10_000;
const MCP_REGISTRY_HEALTH_CACHE_FILE: &str = "mcp_registry_health_negative.json";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct McpRegistryHealthCache {
    failures: Vec<McpRegistryHealthFailure>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpRegistryHealthFailure {
    config_ref: String,
    failed_at_ms: u64,
    reason: String,
    /// 工具级冷却的工具名；缺省表示 server 级故障。
    ///
    /// 旧版本缓存文件没有该字段，反序列化后按 server 级故障读取，不需要迁移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
}

/// 记录 server 级发现故障；只有发现期失败可以调用本函数。
pub(crate) fn record_mcp_health_failure(root: &Path, config_ref: &str, reason: &str) {
    record_failure(root, config_ref, None, reason);
}

/// 记录单个工具的调用失败冷却；调用期失败必须走本函数。
pub(crate) fn record_mcp_tool_cooldown(root: &Path, config_ref: &str, tool: &str, reason: &str) {
    record_failure(root, config_ref, Some(tool), reason);
}

fn record_failure(root: &Path, config_ref: &str, tool: Option<&str>, reason: &str) {
    let _guard = cache_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut cache = read_cache(root);
    let now = unix_now_ms();
    cache.failures.retain(|failure| {
        !matches_scope(failure, config_ref, tool)
            && now.saturating_sub(failure.failed_at_ms) < ttl_ms(failure.tool.as_deref())
    });
    cache.failures.push(McpRegistryHealthFailure {
        config_ref: config_ref.to_owned(),
        failed_at_ms: now,
        reason: reason.to_owned(),
        tool: tool.map(ToOwned::to_owned),
    });
    write_cache(root, &cache);
}

/// 清除 server 级故障记录；tool 级冷却按各自窗口独立过期。
pub(crate) fn clear_mcp_health_failure(root: &Path, config_ref: &str) {
    clear_scope(root, config_ref, None);
}

/// 清除单个工具的冷却记录；该工具调用成功后立即恢复。
pub(crate) fn clear_mcp_tool_cooldown(root: &Path, config_ref: &str, tool: &str) {
    clear_scope(root, config_ref, Some(tool));
}

fn clear_scope(root: &Path, config_ref: &str, tool: Option<&str>) {
    let _guard = cache_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut cache = read_cache(root);
    let before = cache.failures.len();
    cache
        .failures
        .retain(|failure| !matches_scope(failure, config_ref, tool));
    if cache.failures.len() != before {
        write_cache(root, &cache);
    }
}

/// 清空全部作用域的负缓存，供管理面强制重新探测。
pub(crate) fn clear_mcp_health_cache(root: &Path) -> usize {
    let _guard = cache_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut cache = read_cache(root);
    let removed = cache.failures.len();
    if removed > 0 {
        cache.failures.clear();
        write_cache(root, &cache);
    }
    removed
}

pub(crate) fn cached_mcp_health_failure(
    root: &Path,
    config_ref: &str,
    refresh: bool,
) -> Option<String> {
    if refresh {
        clear_mcp_health_failure(root, config_ref);
        return None;
    }
    cached_reason(root, config_ref, None)
}

/// 读取单个工具的冷却原因；命中即应快速失败，不发起真实调用。
pub(crate) fn cached_mcp_tool_cooldown(
    root: &Path,
    config_ref: &str,
    tool: &str,
) -> Option<String> {
    cached_reason(root, config_ref, Some(tool))
}

fn cached_reason(root: &Path, config_ref: &str, tool: Option<&str>) -> Option<String> {
    let now = unix_now_ms();
    read_cache(root)
        .failures
        .into_iter()
        .find(|failure| {
            matches_scope(failure, config_ref, tool)
                && now.saturating_sub(failure.failed_at_ms) < ttl_ms(tool)
        })
        .map(|failure| failure.reason)
}

fn matches_scope(failure: &McpRegistryHealthFailure, config_ref: &str, tool: Option<&str>) -> bool {
    failure.config_ref == config_ref && failure.tool.as_deref() == tool
}

const fn ttl_ms(tool: Option<&str>) -> u64 {
    if tool.is_some() {
        MCP_TOOL_COOLDOWN_TTL_MS
    } else {
        MCP_SERVER_HEALTH_TTL_MS
    }
}

fn read_cache(root: &Path) -> McpRegistryHealthCache {
    fs::read_to_string(cache_path(root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_cache(root: &Path, cache: &McpRegistryHealthCache) {
    let path = cache_path(root);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(payload) = serde_json::to_string(cache) {
        let _ = fs::write(path, payload);
    }
}

fn cache_path(root: &Path) -> PathBuf {
    root.join("cache").join(MCP_REGISTRY_HEALTH_CACHE_FILE)
}

fn cache_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        axon_test_support::test_temp_root().join(format!(
            "axon-mcp-health-{label}-{}-{}",
            std::process::id(),
            unix_now_ms()
        ))
    }

    #[test]
    fn negative_cache_returns_reason_and_clears_exactly() {
        let root = temp_root("lifecycle");
        let first = "mcp://one/.mcp.json";
        let second = "mcp://two/.mcp.json";
        record_mcp_health_failure(root.as_path(), first, "MCP tools/list 超时");
        record_mcp_health_failure(root.as_path(), second, "MCP server 启动失败");

        assert_eq!(
            cached_mcp_health_failure(root.as_path(), first, false).as_deref(),
            Some("MCP tools/list 超时")
        );
        assert_eq!(cached_mcp_health_failure(root.as_path(), first, true), None);
        assert_eq!(
            cached_mcp_health_failure(root.as_path(), first, false),
            None
        );
        assert_eq!(clear_mcp_health_cache(root.as_path()), 1);
        assert_eq!(clear_mcp_health_cache(root.as_path()), 0);
        let _ = fs::remove_dir_all(root);
    }

    /// 调用期失败只允许落在 tool 作用域：既不产生 server 级熔断，也不影响同 server 的其他工具。
    #[test]
    fn tool_cooldown_and_server_failure_are_independent_scopes() {
        let root = temp_root("scope-isolation");
        let config_ref = "mcp://one/.mcp.json";
        let cooled = "mcp__one__query_db";
        let healthy_sibling = "mcp__one__explain_db";

        record_mcp_tool_cooldown(
            root.as_path(),
            config_ref,
            cooled,
            "MCP persistent 响应超时",
        );

        assert_eq!(
            cached_mcp_tool_cooldown(root.as_path(), config_ref, cooled).as_deref(),
            Some("MCP persistent 响应超时")
        );
        assert_eq!(
            cached_mcp_health_failure(root.as_path(), config_ref, false),
            None,
            "工具级冷却不得升级为 server 级熔断"
        );
        assert_eq!(
            cached_mcp_tool_cooldown(root.as_path(), config_ref, healthy_sibling),
            None,
            "同 server 的其他工具不得被连带冷却"
        );

        record_mcp_health_failure(root.as_path(), config_ref, "MCP tools/list 超时");

        assert_eq!(
            cached_mcp_health_failure(root.as_path(), config_ref, false).as_deref(),
            Some("MCP tools/list 超时")
        );
        assert_eq!(
            cached_mcp_tool_cooldown(root.as_path(), config_ref, cooled).as_deref(),
            Some("MCP persistent 响应超时"),
            "server 级故障不得覆盖既有 tool 级冷却记录"
        );

        clear_mcp_health_failure(root.as_path(), config_ref);
        assert_eq!(
            cached_mcp_health_failure(root.as_path(), config_ref, false),
            None
        );
        assert_eq!(
            cached_mcp_tool_cooldown(root.as_path(), config_ref, cooled).as_deref(),
            Some("MCP persistent 响应超时"),
            "清除 server 级故障不得连带清除 tool 级冷却"
        );

        clear_mcp_tool_cooldown(root.as_path(), config_ref, cooled);
        assert_eq!(
            cached_mcp_tool_cooldown(root.as_path(), config_ref, cooled),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 旧版本缓存文件缺少 `tool` 字段时，必须按 server 级故障读取而不是整体失效。
    #[test]
    fn legacy_cache_entry_without_tool_scope_reads_as_server_failure() {
        let root = temp_root("legacy-format");
        let config_ref = "mcp://legacy/.mcp.json";
        let path = cache_path(root.as_path());
        fs::create_dir_all(path.parent().expect("缓存路径应有父目录")).expect("缓存目录应可创建");
        fs::write(
            &path,
            format!(
                r#"{{"failures":[{{"configRef":"{config_ref}","failedAtMs":{},"reason":"MCP server 启动失败"}}]}}"#,
                unix_now_ms()
            ),
        )
        .expect("旧格式缓存应可写入");

        assert_eq!(
            cached_mcp_health_failure(root.as_path(), config_ref, false).as_deref(),
            Some("MCP server 启动失败")
        );
        assert_eq!(
            cached_mcp_tool_cooldown(root.as_path(), config_ref, "mcp__legacy__query_db"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }
}
