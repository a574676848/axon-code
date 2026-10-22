//! Workspace registry 的 SQLite repository。

use super::{SqliteRuntimeStore, helpers::i64_value};
use crate::{
    WorkspaceRecord,
    ports::{RuntimeStore, WorkspaceStore, WorkspaceStoreError},
};
use axon_domain::WorkspaceId;
use rusqlite::{OptionalExtension, params};

impl WorkspaceStore for SqliteRuntimeStore {
    fn register_workspace(&self, workspace: &WorkspaceRecord) -> Result<(), WorkspaceStoreError> {
        self.initialize().map_err(storage_error)?;
        let aliases_json = serde_json::to_string(&workspace.aliases).map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .execute(
                "INSERT INTO jarvis_workspaces(workspace_id,display_name,workspace_root,git_remote,default_branch,aliases_json,memory_namespace,pinned,last_seen_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(workspace_id) DO UPDATE SET display_name=excluded.display_name,workspace_root=excluded.workspace_root,git_remote=excluded.git_remote,default_branch=excluded.default_branch,aliases_json=excluded.aliases_json,memory_namespace=excluded.memory_namespace,pinned=excluded.pinned,last_seen_at_ms=MAX(jarvis_workspaces.last_seen_at_ms,excluded.last_seen_at_ms)",
                params![
                    workspace.workspace_id,
                    workspace.display_name,
                    workspace.workspace_root,
                    workspace.git_remote,
                    workspace.default_branch,
                    aliases_json,
                    workspace.memory_namespace,
                    workspace.pinned,
                    i64_value(workspace.last_seen_at_ms).map_err(storage_error)?,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn get_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<WorkspaceRecord>, WorkspaceStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                "SELECT workspace_id,display_name,workspace_root,git_remote,default_branch,aliases_json,memory_namespace,pinned,last_seen_at_ms FROM jarvis_workspaces WHERE workspace_id=?1",
                params![workspace_id.as_str()],
                read_workspace,
            )
            .optional()
            .map_err(storage_error)
    }

    fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, WorkspaceStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(
                "SELECT workspace_id,display_name,workspace_root,git_remote,default_branch,aliases_json,memory_namespace,pinned,last_seen_at_ms FROM jarvis_workspaces ORDER BY pinned DESC, last_seen_at_ms DESC, workspace_id ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], read_workspace)
            .map_err(storage_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)?;
        Ok(rows)
    }
}

/// 读取一行 Workspace 注册记录。
fn read_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceRecord> {
    let aliases_json = row.get::<_, String>(5)?;
    let last_seen_at_ms = row.get::<_, i64>(8)?;
    Ok(WorkspaceRecord {
        workspace_id: row.get(0)?,
        display_name: row.get(1)?,
        workspace_root: row.get(2)?,
        git_remote: row.get(3)?,
        default_branch: row.get(4)?,
        aliases: serde_json::from_str(&aliases_json).map_err(|_| rusqlite::Error::InvalidQuery)?,
        memory_namespace: row.get(6)?,
        pinned: row.get(7)?,
        last_seen_at_ms: u64::try_from(last_seen_at_ms)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn storage_error(error: impl std::fmt::Display) -> WorkspaceStoreError {
    WorkspaceStoreError::Storage(error.to_string())
}
