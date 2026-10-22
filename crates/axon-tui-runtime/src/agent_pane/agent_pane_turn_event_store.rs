use std::path::{Path, PathBuf};

use axon_agent_runtime::AgentTurnEvent;
use axon_config::{USER_RUNTIME_DATA_DIR, USER_RUNTIME_SUBDIR};
use axon_domain::AxonError;
use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

use crate::CellId;

/// optional UI turn 事件文件名。
pub const AGENT_PANE_TURN_EVENTS_FILE: &str = "terminal_agent_pane_turn_events.jsonl";

/// optional UI turn 事件记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPaneTurnEventRecord {
    /// pane ID。
    pub pane_id: u64,
    /// RuntimeCell ID。
    pub cell_id: CellId,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// turn 事件。
    pub turn_events: Vec<AgentTurnEvent>,
}

/// optional UI turn 事件存储。
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentPaneTurnEventStoreRuntime;
/// optional UI turn 事件使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

impl AgentPaneTurnEventStoreRuntime {
    /// 解析默认持久化文件路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref()
            .join(USER_RUNTIME_DATA_DIR)
            .join(USER_RUNTIME_SUBDIR)
            .join(AGENT_PANE_TURN_EVENTS_FILE)
    }

    /// 追加写入 turn 事件记录。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &AgentPaneTurnEventRecord,
    ) -> Result<(), AxonError> {
        if record.turn_events.is_empty() {
            return Ok(());
        }
        let path = path.as_ref();
        let line = serde_json::to_string(record)
            .map_err(|_| AxonError::runtime_failure("optional UI turn 事件序列化失败"))?;
        SHARD_STORE
            .append_line(path, line.as_bytes())
            .map_err(|_| AxonError::runtime_failure("无法写入 optional UI turn 事件"))
    }

    /// 读取全部 turn 事件记录。
    pub fn read_records(
        path: impl AsRef<Path>,
    ) -> Result<Vec<AgentPaneTurnEventRecord>, AxonError> {
        let path = path.as_ref();
        SHARD_STORE
            .read_lines(path)
            .map_err(|_| AxonError::runtime_failure("无法读取 optional UI turn 事件"))?
            .iter()
            .map(|line| {
                serde_json::from_slice(line)
                    .map_err(|_| AxonError::runtime_failure("optional UI turn 事件 JSONL 无效"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_agent_runtime::{AgentTurnEvent, AgentTurnEventKind};
    use axon_domain::TurnId;
    use std::num::NonZeroU64;

    #[test]
    fn agent_pane_turn_event_store_roundtrips_jsonl() {
        let dir = axon_test_support::test_temp_root()
            .join(format!("agent-pane-turn-events-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("测试目录应可创建");
        let path = AgentPaneTurnEventStoreRuntime::resolve_store_path(&dir);

        AgentPaneTurnEventStoreRuntime::append_record(
            &path,
            &AgentPaneTurnEventRecord {
                pane_id: 7,
                cell_id: CellId::new(NonZeroU64::new(9).expect("cell id must be non-zero")),
                recorded_at_ms: 100,
                turn_events: vec![AgentTurnEvent::new(
                    TurnId(1),
                    AgentTurnEventKind::TurnStarted,
                    "turn started",
                )],
            },
        )
        .expect("写入 turn 事件应成功");

        let loaded =
            AgentPaneTurnEventStoreRuntime::read_records(&path).expect("读取 turn 事件应成功");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].pane_id, 7);
        assert_eq!(loaded[0].cell_id.get().get(), 9);
        assert_eq!(loaded[0].turn_events.len(), 1);
        assert_eq!(loaded[0].turn_events[0].turn_id, TurnId(1));
    }
}
