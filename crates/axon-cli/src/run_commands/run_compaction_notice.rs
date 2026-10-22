//! 空闲压缩结果的会话级持久通知。
//!
//! 空闲轮转在 turn 结束后的 debounce 里由独立线程执行，此时该 turn 的投影通道已经关闭，
//! 结果无法直接回写 stdout。因此结果先落一份会话级通知，由**下一次** run turn 投影：
//! 与后台完成通知「先入持久收件箱，再找可用渠道投递」的既有语义一致，不允许静默丢失。
//!
//! 单槽位设计：两次空闲轮转之间必然存在一次 run turn（排期点就是 turn 收尾），
//! 因此同一会话同时最多只有一条待投递通知。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use axon_protocol::wire::WireContextRollover;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use super::run_compaction_projection::{
    CompactionLifecycleView, CompactionTrigger, compaction_part_id, compaction_window_part_id,
    context_rollover_fact, emit_compaction_lifecycle, emit_context_rollover,
};
use super::run_progress::RunProgressSink;

const SESSIONS_DIR_NAME: &str = "sessions";
const NOTICE_FILE_NAME: &str = "compaction_notice.json";
pub(super) const NOTICE_LOG_PREFIX: &str = "compaction_notice";

/// 会话数据目录。
pub(super) fn session_data_dir(root: &Path, session_id: &str) -> PathBuf {
    root.join(SESSIONS_DIR_NAME).join(session_id)
}

/// 空闲压缩结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CompactionNoticeStatus {
    Completed,
    Failed,
}

/// 一条待补投的空闲压缩结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CompactionNotice {
    pub(super) session_id: String,
    pub(super) status: CompactionNoticeStatus,
    pub(super) summary: String,
    pub(super) window_number: Option<u64>,
    pub(super) window_id: Option<String>,
    pub(super) previous_window_id: Option<String>,
    pub(super) archived_messages_count: usize,
    pub(super) notes_injected: bool,
    pub(super) duration_ms: u64,
    pub(super) recorded_at_ms: u64,
}

impl CompactionNotice {
    /// 通知对应的生命周期投影视图；成功与失败共用通知里的 summary。
    pub(super) fn lifecycle_view(&self) -> CompactionLifecycleView {
        let part_id = match self.window_number {
            Some(window_number) => {
                compaction_window_part_id(CompactionTrigger::Idle, &self.session_id, window_number)
            }
            None => compaction_part_id(CompactionTrigger::Idle, &self.session_id),
        };
        match self.status {
            CompactionNoticeStatus::Completed => CompactionLifecycleView::completed(
                part_id,
                self.summary.clone(),
                None,
                Some(self.duration_ms),
            ),
            CompactionNoticeStatus::Failed => {
                CompactionLifecycleView::failed(part_id, self.summary.clone(), None, None)
            }
        }
    }

    /// 成功通知对应的窗口轮转协议事实；失败通知没有可发布的轮转事实。
    pub(super) fn context_rollover(&self) -> Option<WireContextRollover> {
        if self.status != CompactionNoticeStatus::Completed {
            return None;
        }
        let window_id = self.window_id.clone()?;
        Some(context_rollover_fact(
            self.window_number?,
            window_id,
            self.previous_window_id.clone(),
            self.archived_messages_count,
            self.notes_injected,
        ))
    }
}

fn notice_path(session_dir: &Path) -> PathBuf {
    session_dir.join(NOTICE_FILE_NAME)
}

/// 写入待补投通知；覆盖同一会话的旧通知（单槽位语义）。
pub(super) fn write_compaction_notice(
    session_dir: &Path,
    notice: &CompactionNotice,
) -> Result<(), String> {
    let path = notice_path(session_dir);
    let payload =
        serde_json::to_vec(notice).map_err(|error| format!("序列化空闲压缩通知失败：{error}"))?;
    atomic_write(&path, &payload)
}

/// 取出并清除待补投通知；内容损坏时返回错误并自愈删除，避免每次 turn 重复报错。
pub(super) fn take_compaction_notice(
    session_dir: &Path,
) -> Result<Option<CompactionNotice>, String> {
    let path = notice_path(session_dir);
    let Ok(payload) = fs::read(&path) else {
        return Ok(None);
    };
    let _ = fs::remove_file(&path);
    match serde_json::from_slice::<CompactionNotice>(&payload) {
        Ok(notice) => Ok(Some(notice)),
        Err(error) => Err(format!("空闲压缩通知解析失败，已丢弃：{error}")),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "空闲压缩通知路径缺少父目录".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建会话目录失败：{error}"))?;
    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|error| format!("创建通知临时文件失败：{error}"))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("写入通知临时文件失败：{error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("原子替换通知文件失败：{}", error.error))?;
    Ok(())
}

/// 投影并清除待补投的空闲轮转结果。
///
/// 只有在真正拿到投影通道时才消费通知：拿不到通道时保留通知，等下一次 turn 继续补投，
/// 与「没有可用渠道保留 pending」的完成通知语义一致，不允许静默丢弃。
pub(super) fn project_pending_compaction_notice(
    progress: Option<&RunProgressSink<'_>>,
    root: &Path,
    session_id: &str,
) {
    let Some(sink) = progress else {
        return;
    };
    match take_compaction_notice(&session_data_dir(root, session_id)) {
        Ok(Some(notice)) => {
            emit_compaction_lifecycle(Some(sink), &notice.lifecycle_view());
            if let Some(rollover) = notice.context_rollover() {
                emit_context_rollover(Some(sink), &rollover);
            }
        }
        Ok(None) => {}
        Err(error) => axon_logger::warning(&format!("{NOTICE_LOG_PREFIX} {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::run_compaction_projection::is_text_projectable_kind;
    use super::*;

    fn completed_notice() -> CompactionNotice {
        CompactionNotice {
            session_id: "session-1".to_owned(),
            status: CompactionNoticeStatus::Completed,
            summary: "空闲窗口轮转完成 · 消息 42 → 6 条".to_owned(),
            window_number: Some(2),
            window_id: Some("window-2".to_owned()),
            previous_window_id: Some("window-1".to_owned()),
            archived_messages_count: 36,
            notes_injected: true,
            duration_ms: 312,
            recorded_at_ms: 1_000,
        }
    }

    #[test]
    fn notice_round_trips_and_is_consumed_once() {
        let directory = axon_test_support::test_temp_dir();
        let session_dir = directory.path().join("session-1");
        write_compaction_notice(&session_dir, &completed_notice()).unwrap();

        assert_eq!(
            take_compaction_notice(&session_dir).unwrap(),
            Some(completed_notice())
        );
        assert_eq!(take_compaction_notice(&session_dir).unwrap(), None);
    }

    #[test]
    fn completed_notice_projects_rollover_fact_with_window_identity() {
        let notice = completed_notice();
        let rollover = notice.context_rollover().expect("完成通知应带轮转事实");

        assert_eq!(rollover.window_number, 2);
        assert_eq!(rollover.window_id, "window-2");
        assert_eq!(rollover.archived_messages_count, 36);
        assert!(rollover.notes_injected);
        assert!(
            notice
                .lifecycle_view()
                .part_id
                .ends_with(":idle:session-1:2")
        );
    }

    #[test]
    fn failed_notice_has_no_rollover_fact() {
        let notice = CompactionNotice {
            status: CompactionNoticeStatus::Failed,
            window_number: None,
            window_id: None,
            ..completed_notice()
        };

        assert!(notice.context_rollover().is_none());
        assert!(is_text_projectable_kind(notice.lifecycle_view().kind));
    }

    #[test]
    fn corrupted_notice_is_discarded_instead_of_blocking_every_turn() {
        let directory = axon_test_support::test_temp_dir();
        fs::write(notice_path(directory.path()), b"{ not json").unwrap();

        assert!(take_compaction_notice(directory.path()).is_err());
        assert_eq!(take_compaction_notice(directory.path()).unwrap(), None);
    }
}
