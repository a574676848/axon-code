//! 用户问答交互的编译期阈值与载荷上限常量（SSOT 单一事实源）。
//!
//! 全部常量均为集中定义的编译期常量，不引入任何配置键。

use axon_domain::UserQuestionItem;

/// 首次等待窗口时长（毫秒）：30 秒。
pub const DEFAULT_WAIT_WINDOW_MS: u64 = 30_000;

/// 等待退避乘数：2 倍。
pub const BACKOFF_FACTOR: u32 = 2;

/// 单次提问生命周期总等待预算（毫秒）：120 秒。
pub const TOTAL_BUDGET_MS: u64 = 120_000;

/// 每条车道允许驻留的未答提问最大数量：3 项。
pub const MAX_PENDING_PER_LANE: usize = 3;

/// 宿主等待循环唤醒检查取消/关停的周期（毫秒）：250 毫秒。
pub const CANCELLATION_POLL_INTERVAL_MS: u64 = 250;

/// 单次提问允许包含的最大问题数量：8 项。
pub const MAX_QUESTIONS_PER_CALL: usize = 8;

/// 单个问题允许包含的最大选项数量：20 项。
pub const MAX_OPTIONS_PER_QUESTION: usize = 20;

/// 单个问题正文的最大字符数：512 字符。
pub const MAX_QUESTION_CHARS: usize = 512;

/// 辅助细节 (detail) 的最大字符数：2048 字符（与 schema 的 `maxLength` 同口径，按字符计数）。
pub const MAX_DETAIL_CHARS: usize = 2048;

/// 单次提问总载荷的最大字节数：8192 字节 (8 KiB)。
pub const MAX_PAYLOAD_BYTES: usize = 8192;

/// 提问记录默认过期时间跨度（毫秒）：24 小时 (86,400,000 毫秒)。
pub const DEFAULT_EXPIRES_AFTER_MS: i64 = 86_400_000;

/// 单轮对账补投与过期标记的最大扫描条数：32 条。
///
/// 两者都跑在答案路由收尾与 Daemon 维护 tick 上，必须有界，否则一次崩溃积压
/// 会把单轮维护拖成长事务。
pub const MAX_RECONCILE_BATCH: usize = 32;

/// 校验提问载荷是否在编译期常量硬限之内。
pub fn validate_question_payload(
    questions: &[UserQuestionItem],
    payload_raw_bytes: usize,
) -> Result<(), &'static str> {
    if questions.is_empty() {
        return Err("EMPTY_QUESTIONS");
    }
    if questions.len() > MAX_QUESTIONS_PER_CALL {
        return Err("EXCEEDED_MAX_QUESTIONS");
    }
    if payload_raw_bytes > MAX_PAYLOAD_BYTES {
        return Err("EXCEEDED_MAX_PAYLOAD_BYTES");
    }
    for q in questions {
        if q.question.trim().is_empty() {
            return Err("EMPTY_QUESTION_TEXT");
        }
        if q.question.chars().count() > MAX_QUESTION_CHARS {
            return Err("EXCEEDED_MAX_QUESTION_CHARS");
        }
        if q.options.len() > MAX_OPTIONS_PER_QUESTION {
            return Err("EXCEEDED_MAX_OPTIONS");
        }
        if let Some(detail) = &q.detail
            && detail.chars().count() > MAX_DETAIL_CHARS
        {
            return Err("EXCEEDED_MAX_DETAIL_CHARS");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::UserQuestionOption;

    #[test]
    fn test_limits_values() {
        assert_eq!(DEFAULT_WAIT_WINDOW_MS, 30_000);
        assert_eq!(BACKOFF_FACTOR, 2);
        assert_eq!(TOTAL_BUDGET_MS, 120_000);
        assert_eq!(MAX_PENDING_PER_LANE, 3);
        assert_eq!(CANCELLATION_POLL_INTERVAL_MS, 250);
        assert_eq!(MAX_QUESTIONS_PER_CALL, 8);
        assert_eq!(MAX_OPTIONS_PER_QUESTION, 20);
        assert_eq!(MAX_QUESTION_CHARS, 512);
        assert_eq!(MAX_DETAIL_CHARS, 2048);
        assert_eq!(MAX_PAYLOAD_BYTES, 8192);
        assert_eq!(DEFAULT_EXPIRES_AFTER_MS, 86_400_000);
    }

    #[test]
    fn test_validate_question_payload_boundaries() {
        let empty: Vec<UserQuestionItem> = Vec::new();
        assert_eq!(
            validate_question_payload(&empty, 10),
            Err("EMPTY_QUESTIONS")
        );

        let single = vec![UserQuestionItem {
            id: "q1".to_string(),
            question: "你好？".to_string(),
            header: None,
            detail: None,
            options: vec![],
            multi_select: false,
        }];
        assert_eq!(validate_question_payload(&single, 50), Ok(()));

        // 超过载荷大小上限
        assert_eq!(
            validate_question_payload(&single, MAX_PAYLOAD_BYTES + 1),
            Err("EXCEEDED_MAX_PAYLOAD_BYTES")
        );

        // 超过 8 个问题
        let nine: Vec<UserQuestionItem> = (0..9)
            .map(|i| UserQuestionItem {
                id: format!("q{i}"),
                question: "问题".to_string(),
                header: None,
                detail: None,
                options: vec![],
                multi_select: false,
            })
            .collect();
        assert_eq!(
            validate_question_payload(&nine, 500),
            Err("EXCEEDED_MAX_QUESTIONS")
        );

        // 超过 20 个选项
        let options: Vec<UserQuestionOption> = (0..21)
            .map(|i| UserQuestionOption {
                label: format!("opt{i}"),
                description: None,
            })
            .collect();
        let with_too_many_options = vec![UserQuestionItem {
            id: "q1".to_string(),
            question: "问题".to_string(),
            header: None,
            detail: None,
            options,
            multi_select: false,
        }];
        assert_eq!(
            validate_question_payload(&with_too_many_options, 500),
            Err("EXCEEDED_MAX_OPTIONS")
        );

        // 超过 512 字符的问题
        let long_q = vec![UserQuestionItem {
            id: "q1".to_string(),
            question: "a".repeat(513),
            header: None,
            detail: None,
            options: vec![],
            multi_select: false,
        }];
        assert_eq!(
            validate_question_payload(&long_q, 600),
            Err("EXCEEDED_MAX_QUESTION_CHARS")
        );
    }
}
