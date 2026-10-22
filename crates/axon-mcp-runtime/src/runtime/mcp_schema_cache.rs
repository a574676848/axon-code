//! MCP tool schema 跨进程缓存。

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use axon_agent_runtime::{McpServerConfig, McpToolAnnotations, McpToolSchema};
use axon_crypto::sha256_hex;
use rusqlite::{Connection, OptionalExtension as _, params};

const MCP_SCHEMA_CACHE_VERSION: i64 = 1;
const MCP_SCHEMA_CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
const MCP_SCHEMA_CACHE_FILE: &str = "mcp_tool_schemas.sqlite3";
const MCP_SCHEMA_CACHE_BUSY_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpSchemaCacheDisposition {
    Hit,
    Stale,
    Miss,
    Empty,
    Refreshed,
}

impl McpSchemaCacheDisposition {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Stale => "stale",
            Self::Miss => "miss",
            Self::Empty => "empty",
            Self::Refreshed => "refreshed",
        }
    }
}

pub(crate) enum McpSchemaCacheLookup {
    Hit(Vec<McpToolSchema>),
    Stale(Vec<McpToolSchema>),
    Empty,
    Miss,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedMcpToolSchema {
    name: String,
    mcp_name: String,
    description: String,
    input_schema: serde_json::Value,
    output_schema: Option<serde_json::Value>,
    read_only_hint: Option<bool>,
    destructive_hint: Option<bool>,
    open_world_hint: Option<bool>,
}

impl From<&McpToolSchema> for CachedMcpToolSchema {
    fn from(schema: &McpToolSchema) -> Self {
        Self {
            name: schema.name.clone(),
            mcp_name: schema.mcp_name.clone(),
            description: schema.description.clone(),
            input_schema: schema.input_schema.clone(),
            output_schema: schema.output_schema.clone(),
            read_only_hint: schema.annotations.read_only_hint,
            destructive_hint: schema.annotations.destructive_hint,
            open_world_hint: schema.annotations.open_world_hint,
        }
    }
}

impl From<CachedMcpToolSchema> for McpToolSchema {
    fn from(schema: CachedMcpToolSchema) -> Self {
        Self {
            name: schema.name,
            mcp_name: schema.mcp_name,
            description: schema.description,
            input_schema: schema.input_schema,
            output_schema: schema.output_schema,
            annotations: McpToolAnnotations {
                read_only_hint: schema.read_only_hint,
                destructive_hint: schema.destructive_hint,
                open_world_hint: schema.open_world_hint,
            },
        }
    }
}

pub(crate) fn mcp_config_fingerprint(config: &McpServerConfig) -> String {
    let value = match config {
        McpServerConfig::Stdio(command) => serde_json::json!({
            "type": "stdio",
            "program": command.program,
            "args": command.args,
            "workingDir": command.working_dir,
            "env": command.env,
        }),
        McpServerConfig::StreamableHttp(http) => serde_json::json!({
            "type": "streamable_http",
            "url": http.url,
            "headers": http.headers.iter().map(|header| (&header.name, &header.value)).collect::<Vec<_>>(),
            "bearerTokenEnvVar": http.bearer_token_env_var,
        }),
    };
    sha256_hex(value.to_string().as_bytes())
}

pub(crate) fn lookup_mcp_schemas(
    root: &Path,
    server_id: &str,
    config_ref: &str,
    fingerprint: &str,
    now_ms: u64,
) -> McpSchemaCacheLookup {
    let Ok(connection) = open_cache(root) else {
        return McpSchemaCacheLookup::Miss;
    };
    let entry = connection
        .query_row(
            "SELECT config_ref, config_fingerprint, refreshed_at_ms, schemas_json
             FROM mcp_schema_cache WHERE server_id = ?1",
            params![server_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten();
    let Some((cached_ref, cached_fingerprint, refreshed_at_ms, schemas_json)) = entry else {
        return McpSchemaCacheLookup::Miss;
    };
    if cached_ref != config_ref || cached_fingerprint != fingerprint {
        return McpSchemaCacheLookup::Miss;
    }
    let Ok(schemas) = serde_json::from_str::<Vec<CachedMcpToolSchema>>(&schemas_json) else {
        return McpSchemaCacheLookup::Miss;
    };
    let schemas: Vec<McpToolSchema> = schemas.into_iter().map(McpToolSchema::from).collect();
    if schemas.is_empty() {
        return McpSchemaCacheLookup::Empty;
    }
    let refreshed_at_ms = u64::try_from(refreshed_at_ms).unwrap_or(0);
    if now_ms.saturating_sub(refreshed_at_ms) < MCP_SCHEMA_CACHE_TTL_MS {
        McpSchemaCacheLookup::Hit(schemas)
    } else {
        McpSchemaCacheLookup::Stale(schemas)
    }
}

pub(crate) fn store_mcp_schemas(
    root: &Path,
    server_id: &str,
    config_ref: &str,
    fingerprint: &str,
    refreshed_at_ms: u64,
    schemas: &[McpToolSchema],
) -> Result<(), String> {
    if schemas.is_empty() {
        return Err("MCP schema 缓存拒绝空工具列表".to_owned());
    }
    let connection = open_cache(root)?;
    let schemas = schemas
        .iter()
        .map(CachedMcpToolSchema::from)
        .collect::<Vec<_>>();
    let schemas_json =
        serde_json::to_string(&schemas).map_err(|_| "MCP schema 缓存序列化失败".to_owned())?;
    connection
        .execute(
            "INSERT INTO mcp_schema_cache (
                server_id, config_ref, config_fingerprint, refreshed_at_ms, schemas_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(server_id) DO UPDATE SET
                config_ref = excluded.config_ref,
                config_fingerprint = excluded.config_fingerprint,
                refreshed_at_ms = excluded.refreshed_at_ms,
                schemas_json = excluded.schemas_json",
            params![
                server_id,
                config_ref,
                fingerprint,
                timestamp(refreshed_at_ms),
                schemas_json,
            ],
        )
        .map_err(|_| "MCP schema 缓存事务写入失败".to_owned())?;
    Ok(())
}

fn open_cache(root: &Path) -> Result<Connection, String> {
    let path = cache_path(root);
    let parent = path
        .parent()
        .ok_or_else(|| "MCP schema 缓存路径无父目录".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|_| "MCP schema 缓存目录创建失败".to_owned())?;
    let connection = Connection::open(path).map_err(|_| "MCP schema SQLite 打开失败".to_owned())?;
    connection
        .busy_timeout(Duration::from_millis(MCP_SCHEMA_CACHE_BUSY_TIMEOUT_MS))
        .map_err(|_| "MCP schema SQLite busy timeout 配置失败".to_owned())?;
    ensure_schema(&connection)?;
    Ok(connection)
}

fn ensure_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS mcp_schema_cache (
                server_id TEXT PRIMARY KEY NOT NULL,
                config_ref TEXT NOT NULL,
                config_fingerprint TEXT NOT NULL,
                refreshed_at_ms INTEGER NOT NULL,
                schemas_json TEXT NOT NULL
             );",
        )
        .map_err(|_| "MCP schema SQLite schema 初始化失败".to_owned())?;
    connection
        .pragma_update(None, "user_version", MCP_SCHEMA_CACHE_VERSION)
        .map_err(|_| "MCP schema SQLite 版本写入失败".to_owned())
}

fn cache_path(root: &Path) -> PathBuf {
    root.join("cache").join(MCP_SCHEMA_CACHE_FILE)
}

fn timestamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_agent_runtime::{McpServerCommand, McpToolAnnotations};
    use std::{collections::BTreeMap, fs};

    fn schema(name: &str) -> McpToolSchema {
        McpToolSchema {
            name: name.to_owned(),
            mcp_name: name.to_owned(),
            description: "测试工具".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "目标路径" },
                    "options": {
                        "type": "object",
                        "properties": {
                            "mode": { "type": "string", "enum": ["fast", "safe"] }
                        }
                    },
                    "tags": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["path"]
            }),
            output_schema: Some(serde_json::json!({
                "type": "object",
                "properties": { "result": { "type": "string" } },
                "required": ["result"]
            })),
            annotations: McpToolAnnotations {
                read_only_hint: Some(true),
                destructive_hint: Some(false),
                open_world_hint: Some(false),
            },
        }
    }

    #[test]
    fn cache_requires_exact_config_fingerprint_and_marks_stale_entries() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-mcp-schema-cache-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        store_mcp_schemas(
            &root,
            "mcp_demo",
            "config://demo",
            "fingerprint-a",
            10,
            &[schema("read")],
        )
        .unwrap();

        assert!(matches!(
            lookup_mcp_schemas(&root, "mcp_demo", "config://demo", "fingerprint-a", 11),
            McpSchemaCacheLookup::Hit(value) if value == vec![schema("read")]
        ));
        assert!(matches!(
            lookup_mcp_schemas(&root, "mcp_demo", "config://demo", "fingerprint-b", 11),
            McpSchemaCacheLookup::Miss
        ));
        assert!(matches!(
            lookup_mcp_schemas(&root, "mcp_demo", "config://demo", "fingerprint-a", MCP_SCHEMA_CACHE_TTL_MS + 10),
            McpSchemaCacheLookup::Stale(value) if value == vec![schema("read")]
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fingerprint_changes_with_stdio_execution_config() {
        let config = |program: &str| {
            McpServerConfig::Stdio(McpServerCommand {
                program: program.to_owned(),
                args: vec!["serve".to_owned()],
                working_dir: None,
                env: BTreeMap::new(),
                tool_timeout_ms: None,
            })
        };

        assert_ne!(
            mcp_config_fingerprint(&config("one")),
            mcp_config_fingerprint(&config("two"))
        );
    }

    #[test]
    fn empty_schema_cache_is_not_a_hit_and_cannot_be_stored() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-mcp-empty-schema-cache-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        assert!(
            store_mcp_schemas(
                &root,
                "mcp_empty",
                "config://empty",
                "fingerprint-empty",
                10,
                &[],
            )
            .is_err()
        );

        let connection = open_cache(&root).expect("测试缓存应可打开");
        connection
            .execute(
                "INSERT INTO mcp_schema_cache (
                    server_id, config_ref, config_fingerprint, refreshed_at_ms, schemas_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    "mcp_empty",
                    "config://empty",
                    "fingerprint-empty",
                    timestamp(10),
                    "[]",
                ],
            )
            .expect("测试应能写入历史空缓存");

        assert!(matches!(
            lookup_mcp_schemas(
                &root,
                "mcp_empty",
                "config://empty",
                "fingerprint-empty",
                11
            ),
            McpSchemaCacheLookup::Empty
        ));
        let _ = fs::remove_dir_all(root);
    }
}
