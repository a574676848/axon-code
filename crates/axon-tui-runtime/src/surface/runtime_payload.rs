//! Runtime event payload 抽象。

/// Runtime payload 稳定 kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimePayloadKind(&'static str);

impl RuntimePayloadKind {
    /// 创建 payload kind。
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    /// 返回稳定 kind 文本。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Runtime event payload 契约。
pub trait RuntimePayload {
    /// 返回稳定 payload kind。
    fn payload_kind(&self) -> RuntimePayloadKind;
}
