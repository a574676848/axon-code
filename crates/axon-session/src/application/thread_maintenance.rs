//! Thread 维护型操作：全量投影扫描与子会话来源标注。
//!
//! 与主流程分开成文件，是因为它们不参与一轮对话的写入路径：扫描服务用量投影与
//! 保留策略，来源标注只决定界面名册列不列这条会话。

use std::path::Path;

use crate::{
    application::projection::project_threads,
    domain::{ThreadDetail, ThreadEvent, ThreadId},
    infrastructure::JsonlThreadEventStore,
    ports::ThreadEventStore,
};

use super::thread_service::ThreadService;

impl ThreadService<JsonlThreadEventStore> {
    /// 投影全部 thread 详情；单个 shard 损坏时跳过它，不影响其他会话的只读投影。
    ///
    /// 用量投影与保留策略需要遍历所有轮次：一次扫描 + 一次投影，避免逐会话重复读盘。
    pub fn scan_threads(&self, root: &Path) -> Result<Vec<ThreadDetail>, String> {
        let scan = self.store.scan_all(root)?;
        Ok(project_threads(&scan.records))
    }
}

impl<S> ThreadService<S>
where
    S: ThreadEventStore,
{
    /// 标注子会话来源；重复标注以最后一次为准。
    pub fn annotate_child_origin(
        &self,
        root: &Path,
        command: AnnotateChildOriginCommand,
    ) -> Result<(), String> {
        self.read_thread(root, &command.thread_id)?;
        self.append_simple(
            root,
            &command.thread_id,
            command.recorded_at_ms,
            ThreadEvent::ThreadAnnotated {
                child_origin: Some(command.origin),
            },
        )
    }
}

/// 标注子会话来源的命令。
#[derive(Debug, Clone)]
pub struct AnnotateChildOriginCommand {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 来源标识：用户可见的委派还是运行侧内务。
    pub origin: String,
}
