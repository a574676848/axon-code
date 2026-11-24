//! MCP tool schema 跨进程缓存。

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use axon_agent_runtime::{
    McpServerConfig, McpServerInfoProjection, McpToolAnnotations, McpToolSchema,
};
use axon_crypto::sha256_hex;
use rusqlite::{Connection, OptionalExtension as _, params};

const MCP_SCHEMA_CACHE_VERSION: i64 = 2;
const MCP_SCHEMA_CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
/// 声明 `capabilities.tools.listChanged` 的 server 目录随时会变，TTL 缩短到 5 分钟。
///
/// 这类 server 自己声明了工具集会变，缓存再长就等于把「新工具已经上线」这件事挡住。
const MCP_SCHEMA_CACHE_LIST_CHANGED_TTL_MS: u64 = 5 * 60 * 1_000;
const MCP_SCHEMA_CACHE_FILE: &str = "mcp_tool_schemas.sqlite3";
const MCP_SCHEMA_CACHE_BUSY_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpSchemaCacheDisposition {
    Hit,
    Stale,
    Miss,
    Empty,
    Refreshed,
    /// 发现期熔断命中，沿用上次成功目录：工具仍在清单里，但调用会被快速拒绝。
    Degraded,
}

impl McpSchemaCacheDisposition {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Stale => "stale",
            Self::Miss => "miss",
            Self::Empty => "empty",
            Self::Refreshed => "refreshed",
            Self::Degraded => "degraded",
        }
    }
}

pub(crate) enum McpSchemaCacheLookup {
    Hit(Vec<McpToolSchema>),
    Stale(Vec<McpToolSchema>),
    Empty,
    Miss,
}

/// 写入缓存所需的事实。
///
/// 参数合在一个结构里而不是继续摊成 8 个位置参数：`metadata_fingerprint` 与
/// `tools_list_changed` 都是「server 版本一变就该失效」的同一类事实，分开传最容易错位。
pub(crate) struct McpSchemaCacheEntry<'a> {
    pub(crate) server_id: &'a str,
    pub(crate) config_ref: &'a str,
    pub(crate) config_fingerprint: &'a str,
    /// initialize 结果（`serverInfo.version` + `instructions` 投影 + `tools.listChanged`）的指纹。
    pub(crate) metadata_fingerprint: &'a str,
    /// server 是否声明 `capabilities.tools.listChanged`。
    pub(crate) tools_list_changed: Option<bool>,
    pub(crate) refreshed_at_ms: u64,
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

/// 缓存行的完整读侧投影。
struct CachedMcpSchemaEntry {
    config_ref: String,
    config_fingerprint: String,
    metadata_fingerprint: String,
    tools_list_changed: Option<bool>,
    refreshed_at_ms: u64,
    schemas: Vec<McpToolSchema>,
}

impl CachedMcpSchemaEntry {
    /// 该 server 的目录有效期：声明 listChanged 的一律短 TTL。
    const fn ttl_ms(&self) -> u64 {
        if matches!(self.tools_list_changed, Some(true)) {
            MCP_SCHEMA_CACHE_LIST_CHANGED_TTL_MS
        } else {
            MCP_SCHEMA_CACHE_TTL_MS
        }
    }
}

/// initialize 结果指纹：`serverInfo.version` + instructions 投影 + `tools.listChanged`。
///
/// server 自升级通常不动配置文件，只动版本号与工具集；只按配置指纹判活会让目录一直
/// 停在升级前。instructions 用投影侧同一份有界前缀参与计算，不为了一个哈希去读全量正文。
pub(crate) fn mcp_initialize_fingerprint(metadata: &McpServerInfoProjection) -> String {
    let value = serde_json::json!({
        "version": metadata.version,
        "instructionsDeclared": metadata.instructions_declared,
        "instructionsPreview": metadata.instructions_preview,
        "toolsListChanged": metadata.capabilities.tools_list_changed,
    });
    sha256_hex(value.to_string().as_bytes())
}

pub(crate) fn lookup_mcp_schemas(
    root: &Path,
    server_id: &str,
    config_ref: &str,
    fingerprint: &str,
    now_ms: u64,
) -> McpSchemaCacheLookup {
    let Some(entry) = read_cached_entry(root, server_id) else {
        return McpSchemaCacheLookup::Miss;
    };
    if entry.config_ref != config_ref || entry.config_fingerprint != fingerprint {
        return McpSchemaCacheLookup::Miss;
    }
    if entry.schemas.is_empty() {
        return McpSchemaCacheLookup::Empty;
    }
    if now_ms.saturating_sub(entry.refreshed_at_ms) < entry.ttl_ms() {
        McpSchemaCacheLookup::Hit(entry.schemas)
    } else {
        McpSchemaCacheLookup::Stale(entry.schemas)
    }
}

/// 忽略 TTL、只校验配置指纹，读取上一次成功目录。
///
/// 专供发现期熔断使用：此时不能拉起 server，但把工具整批从模型可见清单里拿掉，模型会
/// 判定该能力不存在。沿用旧目录并标注「暂时不可用」比静默消失更接近事实。
pub(crate) fn lookup_mcp_schemas_ignoring_ttl(
    root: &Path,
    server_id: &str,
    config_ref: &str,
    fingerprint: &str,
) -> Option<Vec<McpToolSchema>> {
    let entry = read_cached_entry(root, server_id)?;
    if entry.config_ref != config_ref
        || entry.config_fingerprint != fingerprint
        || entry.schemas.is_empty()
    {
        return None;
    }
    Some(entry.schemas)
}

/// 读取该 server 缓存行里记录的 initialize 指纹；命中不到返回 `None`。
pub(crate) fn cached_metadata_fingerprint(root: &Path, server_id: &str) -> Option<(String, u64)> {
    let entry = read_cached_entry(root, server_id)?;
    Some((entry.metadata_fingerprint, entry.refreshed_at_ms))
}

fn read_cached_entry(root: &Path, server_id: &str) -> Option<CachedMcpSchemaEntry> {
    let connection = open_cache(root).ok()?;
    let entry = connection
        .query_row(
            "SELECT config_ref, config_fingerprint, metadata_fingerprint, tools_list_changed,
                    refreshed_at_ms, schemas_json
             FROM mcp_schema_cache WHERE server_id = ?1",
            params![server_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<bool>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()?;
    let (
        config_ref,
        config_fingerprint,
        metadata_fingerprint,
        tools_list_changed,
        refreshed_at_ms,
        schemas_json,
    ) = entry;
    let schemas = serde_json::from_str::<Vec<CachedMcpToolSchema>>(&schemas_json).ok()?;
    Some(CachedMcpSchemaEntry {
        config_ref,
        config_fingerprint,
        metadata_fingerprint,
        tools_list_changed,
        refreshed_at_ms: u64::try_from(refreshed_at_ms).unwrap_or(0),
        schemas: schemas.into_iter().map(McpToolSchema::from).collect(),
    })
}

pub(crate) fn store_mcp_schemas(
    root: &Path,
    entry: &McpSchemaCacheEntry<'_>,
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
                server_id, config_ref, config_fingerprint, metadata_fingerprint,
                tools_list_changed, refreshed_at_ms, schemas_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(server_id) DO UPDATE SET
                config_ref = excluded.config_ref,
                config_fingerprint = excluded.config_fingerprint,
                metadata_fingerprint = excluded.metadata_fingerprint,
                tools_list_changed = excluded.tools_list_changed,
                refreshed_at_ms = excluded.refreshed_at_ms,
                schemas_json = excluded.schemas_json",
            params![
                entry.server_id,
                entry.config_ref,
                entry.config_fingerprint,
                entry.metadata_fingerprint,
                entry.tools_list_changed,
                timestamp(entry.refreshed_at_ms),
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

/// 建表并按版本重置。
///
/// 这份库是纯缓存：版本对不上就整表重建，不做列迁移。旧版本没有 initialize 指纹，
/// 沿用它会继续用一份不知道 server 版本的目录，重建一次的代价远小于这个风险。
fn ensure_schema(connection: &Connection) -> Result<(), String> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| "MCP schema SQLite 版本读取失败".to_owned())?;
    if version != MCP_SCHEMA_CACHE_VERSION {
        connection
            .execute_batch("DROP TABLE IF EXISTS mcp_schema_cache;")
            .map_err(|_| "MCP schema SQLite 旧缓存清理失败".to_owned())?;
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS mcp_schema_cache (
                server_id TEXT PRIMARY KEY NOT NULL,
                config_ref TEXT NOT NULL,
                config_fingerprint TEXT NOT NULL,
                metadata_fingerprint TEXT NOT NULL DEFAULT '',
                tools_list_changed INTEGER,
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
    use axon_agent_runtime::{
        McpServerCapabilitiesProjection, McpServerCommand, McpToolAnnotations,
    };
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

    fn entry<'a>(
        server_id: &'a str,
        metadata_fingerprint: &'a str,
        tools_list_changed: Option<bool>,
        refreshed_at_ms: u64,
    ) -> McpSchemaCacheEntry<'a> {
        McpSchemaCacheEntry {
            server_id,
            config_ref: "config://demo",
            config_fingerprint: "fingerprint-a",
            metadata_fingerprint,
            tools_list_changed,
            refreshed_at_ms,
        }
    }

    fn test_root(tag: &str) -> PathBuf {
        axon_test_support::test_temp_root().join(format!(
            "axon-mcp-schema-cache-{tag}-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ))
    }

    fn metadata(version: Option<&str>, instructions: Option<&str>) -> McpServerInfoProjection {
        McpServerInfoProjection {
            name: Some("demo".to_owned()),
            title: None,
            version: version.map(ToOwned::to_owned),
            website_url: None,
            instructions_declared: instructions.is_some(),
            instructions_preview: instructions.map(ToOwned::to_owned),
            capabilities: McpServerCapabilitiesProjection {
                tools: true,
                tools_list_changed: Some(false),
                ..McpServerCapabilitiesProjection::default()
            },
            raw_payload_exposed: false,
        }
    }

    #[test]
    fn cache_requires_exact_config_fingerprint_and_marks_stale_entries() {
        let root = test_root("ttl");
        store_mcp_schemas(
            &root,
            &entry("mcp_demo", "meta-a", None, 10),
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
            lookup_mcp_schemas(
                &root,
                "mcp_demo",
                "config://demo",
                "fingerprint-a",
                MCP_SCHEMA_CACHE_TTL_MS + 10
            ),
            McpSchemaCacheLookup::Stale(value) if value == vec![schema("read")]
        ));
        let _ = fs::remove_dir_all(root);
    }

    /// `tools.listChanged=true` 的 server 目录随时会变：同一时刻必须已过期，而不是沿用一整天。
    #[test]
    fn list_changed_servers_get_a_short_ttl() {
        let root = test_root("list-changed");
        store_mcp_schemas(
            &root,
            &entry("mcp_dynamic", "meta-a", Some(true), 10),
            &[schema("list")],
        )
        .unwrap();
        store_mcp_schemas(
            &root,
            &entry("mcp_static", "meta-a", None, 10),
            &[schema("list")],
        )
        .unwrap();

        let after_short_ttl = 10 + MCP_SCHEMA_CACHE_LIST_CHANGED_TTL_MS + 1;
        assert!(matches!(
            lookup_mcp_schemas(
                &root,
                "mcp_dynamic",
                "config://demo",
                "fingerprint-a",
                after_short_ttl
            ),
            McpSchemaCacheLookup::Stale(_)
        ));
        assert!(matches!(
            lookup_mcp_schemas(
                &root,
                "mcp_static",
                "config://demo",
                "fingerprint-a",
                after_short_ttl
            ),
            McpSchemaCacheLookup::Hit(_)
        ));
        let _ = fs::remove_dir_all(root);
    }

    /// initialize 指纹必须随 server 版本与 instructions 变化：只按配置判活会让目录永远停在升级前。
    #[test]
    fn initialize_fingerprint_tracks_version_and_instructions() {
        let base = mcp_initialize_fingerprint(&metadata(Some("1.0.0"), Some("使用稳定工具名")));
        assert_eq!(
            base,
            mcp_initialize_fingerprint(&metadata(Some("1.0.0"), Some("使用稳定工具名")))
        );
        assert_ne!(
            base,
            mcp_initialize_fingerprint(&metadata(Some("2.0.0"), Some("使用稳定工具名")))
        );
        assert_ne!(
            base,
            mcp_initialize_fingerprint(&metadata(Some("1.0.0"), Some("新的工具约定")))
        );
        assert_ne!(base, mcp_initialize_fingerprint(&metadata(None, None)));
    }

    /// 指纹随缓存行一起落盘，供诊断与「server 版本变过」的核对。
    #[test]
    fn metadata_fingerprint_is_persisted_with_the_catalog_entry() {
        let root = test_root("metadata");
        store_mcp_schemas(
            &root,
            &entry("mcp_demo", "meta-server-1", Some(false), 10),
            &[schema("read")],
        )
        .unwrap();

        assert_eq!(
            cached_metadata_fingerprint(&root, "mcp_demo"),
            Some(("meta-server-1".to_owned(), 10))
        );
        assert_eq!(cached_metadata_fingerprint(&root, "mcp_missing"), None);
        let _ = fs::remove_dir_all(root);
    }

    /// 熔断期沿用目录时不受 TTL 限制，但配置指纹对不上仍然拒绝。
    #[test]
    fn degraded_lookup_ignores_ttl_but_not_config_fingerprint() {
        let root = test_root("degraded");
        store_mcp_schemas(
            &root,
            &entry("mcp_demo", "meta-a", None, 10),
            &[schema("read")],
        )
        .unwrap();

        assert_eq!(
            lookup_mcp_schemas_ignoring_ttl(&root, "mcp_demo", "config://demo", "fingerprint-a"),
            Some(vec![schema("read")])
        );
        assert_eq!(
            lookup_mcp_schemas_ignoring_ttl(&root, "mcp_demo", "config://demo", "fingerprint-b"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 旧版本缓存没有 initialize 指纹：整表重建一次，而不是继续用来源不明的目录。
    #[test]
    fn legacy_cache_table_is_rebuilt_on_version_bump() {
        let root = test_root("legacy");
        let path = cache_path(&root);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE mcp_schema_cache (
                     server_id TEXT PRIMARY KEY NOT NULL,
                     config_ref TEXT NOT NULL,
                     config_fingerprint TEXT NOT NULL,
                     refreshed_at_ms INTEGER NOT NULL,
                     schemas_json TEXT NOT NULL
                 );
                 INSERT INTO mcp_schema_cache VALUES('mcp_old','config://old','fp',1,'[]');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            lookup_mcp_schemas(&root, "mcp_old", "config://old", "fp", 2),
            McpSchemaCacheLookup::Miss
        ));
        let connection = open_cache(&root).expect("重建后的缓存应可打开");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM mcp_schema_cache", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "版本升级必须清空旧行");
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
        let root = test_root("empty");
        assert!(store_mcp_schemas(&root, &entry("mcp_empty", "meta-a", None, 10), &[]).is_err());

        let connection = open_cache(&root).expect("测试缓存应可打开");
        connection
            .execute(
                "INSERT INTO mcp_schema_cache (
                    server_id, config_ref, config_fingerprint, metadata_fingerprint,
                    tools_list_changed, refreshed_at_ms, schemas_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    "mcp_empty",
                    "config://empty",
                    "fingerprint-empty",
                    "meta-empty",
                    Option::<bool>::None,
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
