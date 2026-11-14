//! 客户端外壳偏好：持久化时不重置连接探活事实。

use super::{Runtime, persist};

impl Runtime {
    /// 当前客户端侧栏折叠偏好；空值表示用户尚未手动表态。
    #[must_use]
    pub fn sidebar_collapsed(&self) -> Option<bool> {
        self.read().document.client.sidebar_collapsed
    }

    /// 持久化客户端侧栏折叠偏好，不重置连接探活事实。
    ///
    /// # Errors
    ///
    /// 只读状态或写盘失败。
    pub fn set_sidebar_collapsed(&self, collapsed: bool) -> Result<(), String> {
        let mut inner = self.write();
        if inner.read_only {
            return Err("桌面状态文件来自更高的结构版本，本次只读运行，无法保存改动".to_owned());
        }
        let mut document = inner.document.clone();
        document.client.sidebar_collapsed = Some(collapsed);
        persist(inner.root.as_deref(), &document)?;
        inner.document = document;
        Ok(())
    }
}
