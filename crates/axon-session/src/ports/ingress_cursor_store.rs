//! Transport ingress cursor 的 Session 端口。
use crate::ports::{RuntimeStore, RuntimeStoreError};
/// 独立于 Session event cursor 的持久水位。
pub trait SessionIngressCursorStore: RuntimeStore {
    /// 读取 adapter 分区 cursor JSON。
    fn load_ingress_cursor(
        &self,
        adapter: &str,
        partition: &str,
    ) -> Result<Option<String>, RuntimeStoreError>;
    /// 在 Action 入账后保存 cursor JSON。
    fn save_ingress_cursor(
        &self,
        adapter: &str,
        partition: &str,
        cursor_json: &str,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
}
