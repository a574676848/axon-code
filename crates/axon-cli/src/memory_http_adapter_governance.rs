//! 记忆管理的治理写路径：遗忘、归档、取消归档、物理删除与投影清理。
//!
//! 与读路径（列表、统计、详情、scope 面）分开维护：只有这一组会写治理记录并触碰
//! 检索投影，读写生命周期不同，因此单独成文件。路由仍由父模块统一分发。

use axon_domain::{MemoryId, MemoryState, MemoryTombstone};
use serde_json::{Value, json};

use super::{MemoryHttpAdapter, MemoryHttpError, parse_memory_id};

impl MemoryHttpAdapter {
    /// 遗忘：写入 tombstone 并清空正文引用，同时清理检索投影。
    pub(super) fn forget(&self, id: &str) -> Result<Value, MemoryHttpError> {
        let memory_id = parse_memory_id(id)?;
        let store = self.store()?;
        let mut record = store
            .get_record(memory_id)
            .map_err(|error| MemoryHttpError::internal(format!("读取 Memory 详情失败：{error:?}")))?
            .ok_or_else(|| MemoryHttpError::not_found("未找到 Memory 记录"))?;
        self.ensure_visible(&record)?;
        let vector_ids = record.vector_id.into_iter().collect::<Vec<_>>();
        let blocked_ref = record
            .record
            .content_ref
            .clone()
            .or_else(|| {
                record
                    .record
                    .tombstone
                    .as_ref()
                    .map(|item| item.blocked_ref.clone())
            })
            .ok_or_else(|| MemoryHttpError::bad_request("目标 Memory 缺少可遗忘引用"))?;
        record.record.state = MemoryState::Tombstone;
        record.record.content_ref = None;
        record.record.tombstone = Some(MemoryTombstone {
            blocked_ref,
            reason: "管理页显式遗忘".to_owned(),
        });
        record.updated_at_ms = axon_time::unix_now_ms();
        record.last_active_at_ms = record.updated_at_ms;
        record.archived_at_ms = Some(record.updated_at_ms);
        store
            .update_governance_records(&[record])
            .map_err(|error| {
                MemoryHttpError::internal(format!("写入遗忘 tombstone 失败：{error:?}"))
            })?;
        let needs_repair = !self.prune_projections(&[memory_id], &vector_ids);
        Ok(
            json!({ "status": "memory_forgotten", "memoryId": memory_id.0.to_string(), "needsRepair": needs_repair }),
        )
    }

    /// 物理删除：级联清理 SQLite 行与检索投影。
    pub(super) fn remove(&self, id: &str) -> Result<Value, MemoryHttpError> {
        let memory_id = parse_memory_id(id)?;
        let store = self.store()?;
        let record = store
            .get_record(memory_id)
            .map_err(|error| MemoryHttpError::internal(format!("读取 Memory 详情失败：{error:?}")))?
            .ok_or_else(|| MemoryHttpError::not_found("未找到 Memory 记录"))?;
        self.ensure_visible(&record)?;
        let vector_ids = record.vector_id.into_iter().collect::<Vec<_>>();
        let deleted = store
            .delete_records(&[memory_id])
            .map_err(|error| MemoryHttpError::internal(format!("删除 Memory 失败：{error:?}")))?;
        if deleted == 0 {
            return Err(MemoryHttpError::not_found("未找到 Memory 记录"));
        }
        let needs_repair = !self.prune_projections(&[memory_id], &vector_ids);
        Ok(
            json!({ "status": "memory_deleted", "memoryId": memory_id.0.to_string(), "needsRepair": needs_repair }),
        )
    }

    /// 归档：只移出召回与时间轴，保留正文，可再次取消归档。
    ///
    /// 投影不在这里清理：召回链路在 SQLite 侧就以 `archived_at_ms IS NULL` 过滤，
    /// 多删索引会让「取消归档」多付一次重建代价且可能留下不可恢复的空档。
    pub(super) fn archive(&self, id: &str) -> Result<Value, MemoryHttpError> {
        let memory_id = parse_memory_id(id)?;
        let store = self.store()?;
        let mut record = store
            .get_record(memory_id)
            .map_err(|error| MemoryHttpError::internal(format!("读取 Memory 详情失败：{error:?}")))?
            .ok_or_else(|| MemoryHttpError::not_found("未找到 Memory 记录"))?;
        self.ensure_visible(&record)?;
        if record.record.state == MemoryState::Tombstone {
            return Err(MemoryHttpError::bad_request(
                "已遗忘的记忆不能归档，只能永久删除",
            ));
        }
        if record.archived_at_ms.is_some() {
            return Err(MemoryHttpError::bad_request("这条记忆已经归档"));
        }
        let now_ms = axon_time::unix_now_ms();
        record.archived_at_ms = Some(now_ms);
        record.updated_at_ms = now_ms;
        store
            .update_governance_records(&[record])
            .map_err(|error| MemoryHttpError::internal(format!("归档 Memory 失败：{error:?}")))?;
        Ok(json!({
            "status": "memory_archived",
            "memoryId": memory_id.0.to_string(),
            "archivedAtMs": now_ms,
            "needsRepair": false,
        }))
    }

    /// 取消归档：恢复为活跃记录，重新进入召回与时间轴。
    pub(super) fn unarchive(&self, id: &str) -> Result<Value, MemoryHttpError> {
        let memory_id = parse_memory_id(id)?;
        let store = self.store()?;
        let mut record = store
            .get_record(memory_id)
            .map_err(|error| MemoryHttpError::internal(format!("读取 Memory 详情失败：{error:?}")))?
            .ok_or_else(|| MemoryHttpError::not_found("未找到 Memory 记录"))?;
        self.ensure_visible(&record)?;
        if record.record.state == MemoryState::Tombstone {
            return Err(MemoryHttpError::bad_request(
                "已遗忘的记忆正文已清除，不能取消归档；如需留用请重新写入一条",
            ));
        }
        if record.archived_at_ms.is_none() {
            return Err(MemoryHttpError::bad_request("这条记忆尚未归档"));
        }
        let now_ms = axon_time::unix_now_ms();
        record.archived_at_ms = None;
        record.updated_at_ms = now_ms;
        record.last_active_at_ms = now_ms;
        store
            .update_governance_records(&[record])
            .map_err(|error| MemoryHttpError::internal(format!("取消归档失败：{error:?}")))?;
        Ok(json!({
            "status": "memory_unarchived",
            "memoryId": memory_id.0.to_string(),
            "needsRepair": false,
        }))
    }

    /// 清理文本索引、向量投影与 HNSW；任一失败即要求修复，绝不伪报成功。
    fn prune_projections(&self, memory_ids: &[MemoryId], vector_ids: &[i64]) -> bool {
        let Ok(orchestrator) = self.projection_runtime() else {
            return false;
        };
        let text_ok = orchestrator
            .text_index()
            .and_then(|index| index.delete_documents(memory_ids))
            .is_ok();
        let projection_ok = orchestrator
            .vector_projection_store()
            .and_then(|store| store.delete_projections_by_memory_ids(memory_ids))
            .is_ok();
        let vector_ok = if vector_ids.is_empty() {
            true
        } else {
            orchestrator
                .vector_index()
                .and_then(|index| {
                    index.ok_or_else(|| {
                        axon_domain::AxonError::memory_persistence("Memory HNSW 索引不可用")
                    })
                })
                .and_then(|index| index.remove_vectors(vector_ids))
                .is_ok()
        };
        text_ok && projection_ok && vector_ok
    }
}
