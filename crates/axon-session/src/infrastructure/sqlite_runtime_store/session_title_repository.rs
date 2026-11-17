//! 车道标题写入口。
//!
//! 标题由轮后任务用快速模型生成，三端共用 `sessions.title` 这一列，读取端只做回落。
//! 这里只提供单行点写：不派生、不扫描消息表，避免生成口径在读写两侧各存一份。

use super::SqliteRuntimeStore;
use crate::ports::InteractionStoreError;

/// 把底层持久化错误收敛为端口错误。
fn store_error(error: impl std::fmt::Display) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

impl SqliteRuntimeStore {
    /// 写入车道展示标题；返回是否命中已登记的会话行。
    ///
    /// 未命中的情况是「标题先算完、会话行还没登记」，调用方按可重试处理即可；
    /// 这里不插入占位行，否则会给侧栏造出一条没有轮次的空壳会话。
    pub fn update_session_title(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<bool, InteractionStoreError> {
        let connection = self.connection().map_err(store_error)?;
        let changed = connection
            .execute(
                "UPDATE sessions SET title=?1 WHERE session_id=?2",
                rusqlite::params![title, session_id],
            )
            .map_err(store_error)?;
        Ok(changed > 0)
    }
}
