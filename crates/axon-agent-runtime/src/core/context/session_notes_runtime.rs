//! 当前会话私有便签与窗口交接状态的原子更新运行时。
//!
//! 便签内容由 notes 命名空间的文本文件承载；本运行时负责在窗口轮转锁内
//! 完成写入、读取与检索，并把“当前窗口已写入便签”这一事实固化到
//! `SessionNotes.last_saved_state_window_number`，作为换窗 fail-closed 门禁。

use std::path::Path;

use axon_domain::{AxonError, AxonResult};

use crate::session_notes::SessionNotes;
use crate::session_notes_store::{
    NotesFileEntry, NotesFileOrder, NotesFileOrderBy, NotesFileReceipt, NotesReadReceipt,
    NotesSearchReceipt, NotesStore,
};
use crate::window_rollover::WINDOW_ROLLOVER_LOCK_FILE_NAME;

/// 当前会话私有便签更新入口。
#[derive(Debug, Default, Clone, Copy)]
pub struct SessionNotesRuntime;

impl SessionNotesRuntime {
    /// 整文件写入私有便签；成功后把当前窗口标记为已保存状态。
    pub fn write_note_file(
        session_dir: &Path,
        path: &str,
        text: &str,
        now_ms: i64,
    ) -> AxonResult<NotesFileReceipt> {
        with_session_notes_lock(session_dir, || {
            let receipt = NotesStore::write_file(session_dir, path, text, now_ms)?;
            mark_state_saved(session_dir, now_ms)?;
            Ok(receipt)
        })
    }

    /// 追加写入私有便签；成功后把当前窗口标记为已保存状态。
    pub fn append_note_file(
        session_dir: &Path,
        path: &str,
        text: &str,
        now_ms: i64,
    ) -> AxonResult<NotesFileReceipt> {
        with_session_notes_lock(session_dir, || {
            let receipt = NotesStore::append_to_file(session_dir, path, text, now_ms)?;
            mark_state_saved(session_dir, now_ms)?;
            Ok(receipt)
        })
    }

    /// 按 1-based 行范围读取私有便签；负值表示从末尾回数。
    pub fn read_note_file(
        session_dir: &Path,
        path: &str,
        start_line: Option<i64>,
        stop_line: Option<i64>,
    ) -> AxonResult<NotesReadReceipt> {
        with_session_notes_lock(session_dir, || {
            NotesStore::read_file(session_dir, path, start_line, stop_line)
        })
    }

    /// 按虚拟路径前缀列举私有便签文件。
    pub fn list_note_files(
        session_dir: &Path,
        prefix: Option<&str>,
        max_results: Option<usize>,
        order_by: NotesFileOrderBy,
        order: NotesFileOrder,
    ) -> AxonResult<Vec<NotesFileEntry>> {
        with_session_notes_lock(session_dir, || {
            NotesStore::list_files_by_prefix(session_dir, prefix, max_results, order_by, order)
        })
    }

    /// 区分大小写的字面量检索私有便签内容。
    pub fn search_note_files(
        session_dir: &Path,
        query: &str,
        max_matches_per_file: Option<usize>,
        max_files: Option<usize>,
        path_prefix: Option<&str>,
        recent_file_first: bool,
    ) -> AxonResult<NotesSearchReceipt> {
        with_session_notes_lock(session_dir, || {
            NotesStore::search_contents(
                session_dir,
                query,
                max_matches_per_file,
                max_files,
                path_prefix,
                recent_file_first,
            )
        })
    }

    /// 仅在 GoalAnchor 为空时，以当前请求目标初始化；不会覆盖已存在目标。
    pub fn initialize_goal_anchor(
        session_dir: &Path,
        user_objective: &str,
        now_ms: i64,
    ) -> AxonResult<()> {
        let user_objective = user_objective.trim();
        if user_objective.is_empty() {
            return Ok(());
        }
        with_session_notes_lock(session_dir, || {
            let mut notes = SessionNotes::load_from_session(session_dir)?
                .unwrap_or_else(|| SessionNotes::new(Default::default(), now_ms));
            if !notes.goal_anchor.user_objective.trim().is_empty() {
                return Ok(());
            }
            notes.goal_anchor.user_objective = user_objective.to_owned();
            notes.revision = notes.revision.saturating_add(1);
            notes.updated_at_ms = now_ms;
            notes.save_to_session(session_dir)?;
            Ok(())
        })
    }

    /// 校验指定窗口是否已经成功保存过任务状态。
    pub fn ensure_state_saved_for_window(session_dir: &Path, window_number: u64) -> AxonResult<()> {
        with_session_notes_lock(session_dir, || {
            let notes = SessionNotes::load_from_session(session_dir)?
                .ok_or_else(|| AxonError::runtime_failure("会话便签尚未初始化"))?;
            ensure_notes_window_saved(&notes, window_number)
        })
    }

    /// 校验会话当前窗口是否已经成功保存过任务状态。
    ///
    /// 供换窗工具在调用边界做前置门禁：失败只拒绝本次工具请求，
    /// 模型可在同一 turn 内先写入便签再重试换窗。
    pub fn ensure_current_window_state_saved(session_dir: &Path) -> AxonResult<()> {
        with_session_notes_lock(session_dir, || {
            let notes = SessionNotes::load_from_session(session_dir)?
                .ok_or_else(|| AxonError::runtime_failure("会话便签尚未初始化"))?;
            ensure_notes_window_saved(&notes, notes.window_number)
        })
    }
}

fn ensure_notes_window_saved(notes: &SessionNotes, window_number: u64) -> AxonResult<()> {
    if notes.last_saved_state_window_number == Some(window_number) {
        return Ok(());
    }
    Err(AxonError::runtime_failure(format!(
        "窗口 #{window_number} 尚未成功写入任务便签，拒绝换窗"
    )))
}

fn mark_state_saved(session_dir: &Path, now_ms: i64) -> AxonResult<()> {
    let mut notes = SessionNotes::load_from_session(session_dir)?
        .unwrap_or_else(|| SessionNotes::new(Default::default(), now_ms));
    notes.last_saved_state_window_number = Some(notes.window_number);
    notes.revision = notes.revision.saturating_add(1);
    notes.updated_at_ms = now_ms;
    notes.save_to_session(session_dir)?;
    Ok(())
}

pub(crate) fn with_session_notes_lock<T>(
    session_dir: &Path,
    operation: impl FnOnce() -> AxonResult<T>,
) -> AxonResult<T> {
    std::fs::create_dir_all(session_dir)
        .map_err(|error| AxonError::runtime_failure(format!("创建会话目录失败: {error:?}")))?;
    let lock_path = session_dir.join(WINDOW_ROLLOVER_LOCK_FILE_NAME);
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| AxonError::runtime_failure(format!("打开窗口状态锁失败: {error:?}")))?;
    lock_file
        .lock()
        .map_err(|error| AxonError::runtime_failure(format!("获取窗口状态锁失败: {error:?}")))?;
    let result = operation();
    drop(lock_file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_rollover_requires_state_saved_in_current_window() {
        let session_dir = axon_test_support::test_temp_root()
            .join(format!("axon_notes_handoff_test_{}", std::process::id()));
        let notes = SessionNotes::new(Default::default(), 1);
        notes
            .save_to_session(&session_dir)
            .expect("initialize notes");
        assert!(SessionNotesRuntime::ensure_state_saved_for_window(&session_dir, 1).is_err());

        SessionNotesRuntime::write_note_file(&session_dir, "checkpoint.md", "当前进度已记录", 2)
            .expect("write notes");
        assert!(SessionNotesRuntime::ensure_state_saved_for_window(&session_dir, 1).is_ok());
        assert!(
            SessionNotesRuntime::ensure_state_saved_for_window(&session_dir, 2).is_err(),
            "新窗口必须重新写入便签"
        );
        let _ = std::fs::remove_dir_all(session_dir);
    }
}
