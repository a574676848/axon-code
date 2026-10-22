//! 工作区候选搜索索引。
//!
//! 分工：walker 提供文件全集，`builder` 负责读取与 trigram 提取，`manager` 按 workspace
//! 维护已发布快照与增量覆盖层，`candidate_filter` 把本轮 entries 收敛成候选集合。
//! 任何不可信状态一律 fail-open 回到完整扫描：索引“没有命中”绝不能被解释成“文件不匹配”。

mod builder;
mod candidate_filter;
#[cfg(test)]
mod differential_tests;
mod manager;
mod model;
mod overlay;
mod planner;
#[cfg(test)]
mod roi_probe;
mod storage;
#[cfg(test)]
mod tests;
mod trigram;

use std::sync::{Arc, OnceLock};

pub(crate) use manager::{IndexQuery, SearchIndexManager};

static GLOBAL_MANAGER: OnceLock<Arc<SearchIndexManager>> = OnceLock::new();

/// 进程级索引管理器：同一 workspace 的搜索共享快照、覆盖层与后台任务。
pub(crate) fn global() -> &'static Arc<SearchIndexManager> {
    GLOBAL_MANAGER.get_or_init(|| Arc::new(SearchIndexManager::default()))
}
