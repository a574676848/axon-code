/// 渠道失败分类，决定是否触发 failover。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelFailureKind {
    /// 瞬时/渠道级：超时、连接错误、429、403、5xx → 应换渠道。
    Transient,
    /// 永久：鉴权/参数错误 400/401/404 → 快速失败，不换渠道。
    Permanent,
}

/// 由 HTTP 状态 + 错误消息推导分类。纯函数。
#[must_use]
pub fn classify_channel_failure(http_status: Option<u16>, message: &str) -> ChannelFailureKind {
    if let Some(status) = http_status {
        match status {
            408 | 429 | 403 | 500 | 502 | 503 | 504 => ChannelFailureKind::Transient,
            400 | 401 | 404 => ChannelFailureKind::Permanent,
            s if s >= 500 => ChannelFailureKind::Transient, // 其它5xx认为是瞬时
            s if (400..500).contains(&s) => ChannelFailureKind::Permanent, // 其它4xx认为是永久
            _ => ChannelFailureKind::Transient,             // 兜底
        }
    } else {
        // 无 HTTP 状态的错误，比如超时/连接重置等
        let msg_lower = message.to_lowercase();
        if msg_lower.contains("timeout")
            || msg_lower.contains("connection reset")
            || msg_lower.contains("connection refused")
            || msg_lower.contains("closed")
            || msg_lower.contains("connect")
        {
            ChannelFailureKind::Transient
        } else {
            // 保守策略：无状态码的其他传输错误默认为可重试
            ChannelFailureKind::Transient
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_channel_failure() {
        // HTTP 状态码测试
        assert_eq!(
            classify_channel_failure(Some(500), ""),
            ChannelFailureKind::Transient
        );
        assert_eq!(
            classify_channel_failure(Some(503), ""),
            ChannelFailureKind::Transient
        );
        assert_eq!(
            classify_channel_failure(Some(429), ""),
            ChannelFailureKind::Transient
        );
        assert_eq!(
            classify_channel_failure(Some(408), ""),
            ChannelFailureKind::Transient
        );

        assert_eq!(
            classify_channel_failure(Some(400), ""),
            ChannelFailureKind::Permanent
        );
        assert_eq!(
            classify_channel_failure(Some(401), ""),
            ChannelFailureKind::Permanent
        );
        assert_eq!(
            classify_channel_failure(Some(403), ""),
            ChannelFailureKind::Transient
        );
        assert_eq!(
            classify_channel_failure(Some(404), ""),
            ChannelFailureKind::Permanent
        );

        // 其它状态码
        assert_eq!(
            classify_channel_failure(Some(501), ""),
            ChannelFailureKind::Transient
        );
        assert_eq!(
            classify_channel_failure(Some(405), ""),
            ChannelFailureKind::Permanent
        );
    }
}
