//! person 类记忆不进画像推断（M6-3）。
//!
//! 防「喜欢某歌手 → 喜欢某类音乐」这类跨层推断：凡 person 卡片类记忆一律排除。
//! 说明：仓库实际的 `MemoryKind` 没有 `Person` 变体，也没有 `tags` 列；
//! 这里以 `Profile` 类型 + 内容引用/摘要中的 `person_card` 标记作为等价判据。

use crate::MemorySqliteRecord;

/// person 卡片的标记串。
pub const PERSON_CARD_TAG: &str = "person_card";

/// 是否为 person 卡片类记忆。
#[must_use]
pub fn is_person_card_like(record: &MemorySqliteRecord) -> bool {
    if record.memory_kind != axon_domain::MemoryKind::Profile {
        return false;
    }
    let content_matches = record
        .record
        .content_ref
        .as_deref()
        .is_some_and(|value| value.contains(PERSON_CARD_TAG) || value.starts_with("person:"));
    let summary_matches = record
        .record
        .source_trace
        .as_ref()
        .is_some_and(|trace| trace.summary.contains(PERSON_CARD_TAG));
    content_matches || summary_matches
}

/// 过滤画像推断的输入集合。
#[must_use]
pub fn profile_inference_inputs(records: &[MemorySqliteRecord]) -> Vec<&MemorySqliteRecord> {
    records
        .iter()
        .filter(|record| !is_person_card_like(record))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{
        MemoryCandidateId, MemoryId, MemoryRecord, MemorySourceOrigin, MemorySourceTrace,
        MemoryState,
    };

    fn record(
        id: u64,
        kind: axon_domain::MemoryKind,
        content_ref: &str,
        summary: &str,
    ) -> MemorySqliteRecord {
        MemorySqliteRecord {
            record: MemoryRecord {
                id: MemoryId(id),
                candidate_id: MemoryCandidateId(1),
                state: MemoryState::Strong,
                content_ref: Some(content_ref.to_owned()),
                source_trace: Some(MemorySourceTrace {
                    origin: MemorySourceOrigin::LocalTimeline,
                    session_id: None,
                    cell_id: None,
                    summary: summary.to_owned(),
                    evidence_ref: None,
                }),
                tombstone: None,
            },
            memory_kind: kind,
            namespace_ref: Some("workspace:current".to_owned()),
            repo_ref: None,
            user_ref: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            last_active_at_ms: 1,
            ttl_expires_at_ms: None,
            confidence_basis_points: 8_000,
            importance_basis_points: 8_000,
            vector_id: None,
            archived_at_ms: None,
            prompt_text_persisted: false,
            raw_payload_persisted: false,
            remote_write_allowed: false,
        }
    }

    #[test]
    fn person_card_is_detected_by_content_ref() {
        let record = record(
            1,
            axon_domain::MemoryKind::Profile,
            "memory://person_card/周杰伦",
            "歌手",
        );
        assert!(is_person_card_like(&record));
    }

    #[test]
    fn person_card_is_detected_by_summary_tag() {
        let record = record(
            2,
            axon_domain::MemoryKind::Profile,
            "memory://a",
            "person_card 张三",
        );
        assert!(is_person_card_like(&record));
    }

    #[test]
    fn plain_preference_is_not_person_card() {
        let record = record(
            3,
            axon_domain::MemoryKind::Preference,
            "memory://pref",
            "喜欢清淡口味",
        );
        assert!(!is_person_card_like(&record));
    }

    #[test]
    fn profile_without_person_tag_is_kept() {
        let record = record(
            4,
            axon_domain::MemoryKind::Profile,
            "memory://profile",
            "常用语言 Rust",
        );
        assert!(!is_person_card_like(&record));
    }

    #[test]
    fn profile_inference_excludes_person_cards() {
        let person = record(
            1,
            axon_domain::MemoryKind::Profile,
            "memory://person_card/周杰伦",
            "歌手",
        );
        let preference = record(
            2,
            axon_domain::MemoryKind::Preference,
            "memory://pref",
            "喜欢清淡口味",
        );
        let records = vec![person, preference];
        let inputs = profile_inference_inputs(&records);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].record.id, MemoryId(2));
        assert!(
            inputs.iter().all(|record| !is_person_card_like(record)),
            "person 卡片不得进入画像推断"
        );
    }
}
