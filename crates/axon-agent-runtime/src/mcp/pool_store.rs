//! 插件 MCP 会话池持久化合同。

use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_domain::{AxonError, AxonResult};
use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

use crate::{
    McpPoolDoctorProjection, McpPoolDoctorStatus, McpPoolTelemetry,
    runtime_store_path::runtime_store_file_path,
};

/// MCP 会话池持久化文件名。
pub const MCP_POOL_STORE_FILE: &str = "mcp_pool.jsonl";
/// MCP 会话池持久化 schema 版本。
pub const MCP_POOL_STORE_SCHEMA_VERSION: u32 = 1;
/// MCP 会话池使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// MCP 会话池持久化记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPoolStoreRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// MCP server ID。
    pub server_id: String,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 活跃会话数量。
    pub active_session_count: usize,
    /// 正忙会话数量。
    pub busy_session_count: usize,
    /// 连续失败次数。
    pub failure_count: u32,
    /// 剩余退避毫秒。
    pub backoff_remaining_ms: Option<u64>,
    /// doctor 状态。
    pub doctor_status: String,
    /// doctor badge。
    pub doctor_badge: String,
    /// terminal 主循环是否继续。
    pub terminal_main_loop_continues: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
}

impl McpPoolStoreRecord {
    /// 从遥测与 doctor 投影生成持久化记录。
    #[must_use]
    pub fn from_projection(
        telemetry: &McpPoolTelemetry,
        doctor: &McpPoolDoctorProjection,
        recorded_at_ms: u64,
    ) -> Self {
        Self {
            schema_version: MCP_POOL_STORE_SCHEMA_VERSION,
            server_id: telemetry.server_id.clone(),
            recorded_at_ms,
            active_session_count: telemetry.active_session_count,
            busy_session_count: telemetry.busy_session_count,
            failure_count: telemetry.failure_count,
            backoff_remaining_ms: telemetry.backoff_remaining_ms,
            doctor_status: doctor_status_label(doctor.status).to_owned(),
            doctor_badge: doctor.badge.label.clone(),
            terminal_main_loop_continues: doctor.terminal_main_loop_continues,
            remote_trigger_allowed: false,
        }
    }
}

/// MCP 会话池持久化 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpPoolStoreRuntime;

impl McpPoolStoreRuntime {
    /// 解析默认持久化文件路径。
    #[must_use]
    pub fn resolve_store_path(root: &Path) -> PathBuf {
        runtime_store_file_path(root, MCP_POOL_STORE_FILE)
    }

    /// 追加写入 JSONL 记录。
    pub fn append_record(path: &Path, record: &McpPoolStoreRecord) -> AxonResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| AxonError::runtime_failure("无法创建 MCP 会话池持久化目录"))?;
        }
        let line = serde_json::to_string(record)
            .map_err(|_| AxonError::runtime_failure("MCP 会话池记录序列化失败"))?;
        SHARD_STORE
            .append_line(path, line.as_bytes())
            .map_err(|_| AxonError::runtime_failure("无法写入 MCP 会话池持久化记录"))
    }

    /// 读取全部 JSONL 记录。
    pub fn read_records(path: &Path) -> AxonResult<Vec<McpPoolStoreRecord>> {
        SHARD_STORE
            .read_lines(path)
            .map_err(|_| AxonError::runtime_failure("无法读取 MCP 会话池持久化记录"))?
            .iter()
            .map(|line| {
                serde_json::from_slice::<McpPoolStoreRecord>(line)
                    .map_err(|_| AxonError::runtime_failure("MCP 会话池持久化记录 JSONL 无效"))
            })
            .collect()
    }

    /// 读取指定 MCP server 的最新记录。
    pub fn latest_record(path: &Path, server_id: &str) -> AxonResult<Option<McpPoolStoreRecord>> {
        Ok(Self::read_records(path)?
            .into_iter()
            .filter(|record| record.server_id == server_id)
            .max_by_key(|record| record.recorded_at_ms))
    }
}

fn doctor_status_label(status: McpPoolDoctorStatus) -> &'static str {
    match status {
        McpPoolDoctorStatus::Ok => "ok",
        McpPoolDoctorStatus::Degraded => "degraded",
    }
}
