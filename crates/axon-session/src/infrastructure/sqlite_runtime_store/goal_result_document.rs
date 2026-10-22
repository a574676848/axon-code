//! Goal 完整结果的不可变 UTF-8 文档归档。

use super::SqliteRuntimeStore;
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, ExecutionId};
use std::io::Write;

const GOAL_RESULT_DIRECTORY: &str = "goal-results";
const GOAL_RESULT_EXTENSION: &str = "md";

pub(super) fn save(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    execution_id: &ExecutionId,
    content: &str,
) -> Result<String, RuntimeStoreError> {
    // 归档与数据库位于同一用户运行目录，避免写入不可信 Workspace 路径。
    // 文件名由身份和正文散列生成，不接受 Execution ID 中的路径分隔符。
    store.get_session(session_id)?;
    let directory = store
        .path
        .parent()
        .ok_or_else(|| RuntimeStoreError::Storage("运行目录缺少父目录".to_owned()))?
        .join(GOAL_RESULT_DIRECTORY);
    std::fs::create_dir_all(&directory).map_err(storage)?;
    let name = axon_crypto::sha256_hex(format!("{}\n{content}", execution_id.as_str()).as_bytes());
    let destination = directory.join(format!("{name}.{GOAL_RESULT_EXTENSION}"));
    if destination.exists() {
        let existing = std::fs::read(&destination).map_err(storage)?;
        if existing != content.as_bytes() {
            return Err(RuntimeStoreError::Conflict("结果文档内容不一致".to_owned()));
        }
    } else {
        let temporary = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let saved = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &destination)
        })();
        if let Err(error) = saved {
            let _ = std::fs::remove_file(&temporary);
            return Err(storage(error));
        }
    }
    std::fs::canonicalize(destination)
        .map(|path| path.display().to_string())
        .map_err(storage)
}

fn storage(error: std::io::Error) -> RuntimeStoreError {
    RuntimeStoreError::Storage(format!("保存 Goal 结果文档失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_is_complete_utf8_and_idempotent_with_safe_filename() {
        let home = tempfile::tempdir().expect("临时目录");
        let store = SqliteRuntimeStore::new(home.path().join("runtime.sqlite3"));
        let session = DaemonSessionId::new("document-session").expect("会话 ID");
        store.create_session(&session, ".", 1).expect("会话");
        let execution = ExecutionId::new("../nested-id").expect("执行 ID");
        let content = "完整结果😀\n".repeat(1_000);
        let path = save(&store, &session, &execution, &content).expect("保存");
        assert_eq!(std::fs::read_to_string(&path).expect("读取"), content);
        assert_eq!(
            save(&store, &session, &execution, &content).expect("重试"),
            path
        );
        assert_eq!(
            std::fs::read_dir(home.path().join(GOAL_RESULT_DIRECTORY))
                .expect("目录")
                .count(),
            1
        );
    }
}
