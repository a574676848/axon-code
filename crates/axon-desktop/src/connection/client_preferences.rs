//! 客户端外壳偏好：持久化时不重置连接探活事实。

use super::store::ClientLayer;
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
        self.update_client_layer(|client| client.sidebar_collapsed = Some(collapsed))
    }

    /// 输入坞最近一次显式选择的模型名称；空值表示用户从未选过。
    #[must_use]
    pub fn composer_model(&self) -> Option<String> {
        self.read().document.client.composer_model.clone()
    }

    /// 输入坞最近一次显式选择的思考强度；空值表示用户从未选过。
    #[must_use]
    pub fn composer_reasoning_effort(&self) -> Option<String> {
        self.read()
            .document
            .client
            .composer_reasoning_effort
            .clone()
    }

    /// 记下输入坞选择的主模型名称（配置里的展示名）。
    ///
    /// # Errors
    ///
    /// 只读状态或写盘失败。
    pub fn set_composer_model(&self, model: Option<&str>) -> Result<(), String> {
        let pick = normalize_pick(model);
        self.update_client_layer(|client| client.composer_model = pick)
    }

    /// 记下输入坞选择的思考强度。
    ///
    /// # Errors
    ///
    /// 只读状态或写盘失败。
    pub fn set_composer_reasoning_effort(&self, effort: Option<&str>) -> Result<(), String> {
        let pick = normalize_pick(effort);
        self.update_client_layer(|client| client.composer_reasoning_effort = pick)
    }

    /// 只改客户端层的一个字段。
    ///
    /// 与 `update_document` 分开：那条路会顺带重建生效档案并清零探活结论，改一个外观
    /// 偏好不该让侧栏的连接读数闪回「未连接」。
    fn update_client_layer(&self, update: impl FnOnce(&mut ClientLayer)) -> Result<(), String> {
        let mut inner = self.write();
        if inner.read_only {
            return Err("桌面状态文件来自更高的结构版本，本次只读运行，无法保存改动".to_owned());
        }
        let mut document = inner.document.clone();
        update(&mut document.client);
        persist(inner.root.as_deref(), &document)?;
        inner.document = document;
        Ok(())
    }
}

/// 空白选择一律按「没有缓存」处理，状态文件里不留空字符串。
fn normalize_pick(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::store::ClientStateDocument;

    /// 只改内存的运行态：落盘根为 `None`，测试不会碰到用户真实状态文件。
    fn runtime() -> Runtime {
        Runtime::from_document(ClientStateDocument::default(), false)
    }

    #[test]
    fn composer_pick_round_trips_through_the_client_layer() {
        let runtime = runtime();
        assert_eq!(runtime.composer_model(), None, "初始没有缓存");

        runtime
            .set_composer_model(Some("deepseek-v4.1-flash"))
            .unwrap();
        runtime.set_composer_reasoning_effort(Some("high")).unwrap();
        assert_eq!(
            runtime.composer_model().as_deref(),
            Some("deepseek-v4.1-flash")
        );
        assert_eq!(runtime.composer_reasoning_effort().as_deref(), Some("high"));

        // 改选模型不得清掉已有的强度，改选强度同理。
        runtime.set_composer_model(Some("glm-5")).unwrap();
        assert_eq!(runtime.composer_reasoning_effort().as_deref(), Some("high"));
        runtime.set_composer_reasoning_effort(Some("low")).unwrap();
        assert_eq!(runtime.composer_model().as_deref(), Some("glm-5"));
    }

    #[test]
    fn blank_pick_clears_the_cache_instead_of_storing_an_empty_string() {
        let runtime = runtime();
        runtime.set_composer_model(Some("  ")).unwrap();
        runtime.set_composer_reasoning_effort(Some("high")).unwrap();
        assert_eq!(runtime.composer_model(), None, "空白按没有缓存处理");
        assert_eq!(runtime.composer_reasoning_effort().as_deref(), Some("high"));

        runtime.set_composer_reasoning_effort(None).unwrap();
        assert_eq!(runtime.composer_reasoning_effort(), None);
    }

    #[test]
    fn read_only_runtime_refuses_to_persist_composer_picks() {
        let read_only = Runtime::from_document(ClientStateDocument::default(), true);
        assert!(read_only.set_composer_model(Some("glm-5")).is_err());
        assert_eq!(read_only.composer_model(), None);
    }
}
