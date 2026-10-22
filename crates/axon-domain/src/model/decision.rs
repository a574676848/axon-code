//! Work Ledger Decision 领域模型。

use super::{DecisionId, ScopeRef};
use serde::{Deserialize, Serialize};

/// Decision 效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionEffect {
    /// 允许执行。
    Allow,
    /// 拒绝执行。
    Deny,
    /// 撤销之前的允许。
    Revoke,
}

/// 执行前需要复核的持久 Decision。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// Decision 标识。
    pub decision_id: DecisionId,
    /// 作用域。
    pub scope: ScopeRef,
    /// 发起或撤销者标识。
    pub actor: String,
    /// 当前效果。
    pub effect: DecisionEffect,
    /// revision。
    pub revision: u64,
    /// 过期时间；None 表示不按时间过期。
    pub expires_at_ms: Option<u64>,
    /// 写入时间。
    pub created_at_ms: u64,
}

impl DecisionRecord {
    /// 判断 Decision 当前是否有效；过期和撤销均 fail-closed。
    #[must_use]
    pub fn is_effective_at(&self, now_ms: u64) -> bool {
        self.effect == DecisionEffect::Allow
            && self.expires_at_ms.is_none_or(|expires| now_ms < expires)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_or_revoked_decision_fails_closed() {
        let decision = DecisionRecord {
            decision_id: DecisionId::new("decision-1").expect("ID"),
            scope: ScopeRef::global(),
            actor: "user".to_owned(),
            effect: DecisionEffect::Allow,
            revision: 1,
            expires_at_ms: Some(10),
            created_at_ms: 1,
        };
        assert!(decision.is_effective_at(9));
        assert!(!decision.is_effective_at(10));
        assert!(
            !DecisionRecord {
                effect: DecisionEffect::Revoke,
                ..decision
            }
            .is_effective_at(1)
        );
    }
}
