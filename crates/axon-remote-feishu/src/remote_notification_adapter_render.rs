//! 远控通知平台 renderer。
//!
//! 该模块把平台无关 `RemoteNotificationCard` 渲染成具体 adapter 可消费的载荷。
//! 当前先提供 Feishu renderer，后续 Mobile / Web 可在此并列扩展。

use axon_domain::{AxonError, AxonResult};

use axon_remote_runtime::{
    REMOTE_NOTIFICATION_CARD_BODY_MAX_CHARS, REMOTE_NOTIFICATION_CARD_KIND_SUMMARY_MAX_CHARS,
    REMOTE_NOTIFICATION_CARD_TITLE_MAX_CHARS, RemoteNotificationAdapterEnvelopeSurface,
    RemoteNotificationCard, RemoteNotificationCardTone, RemoteTransportKind,
};

/// Feishu renderer runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationFeishuRendererRuntime;

impl RemoteNotificationFeishuRendererRuntime {
    /// 把平台无关通知卡片渲染为 Feishu interactive card JSON。
    ///
    /// # Errors
    ///
    /// 当卡片不是 Feishu 表面、边界无效或序列化失败时返回错误。
    pub fn render(card: &RemoteNotificationCard) -> AxonResult<String> {
        validate_feishu_card(card)?;
        let payload = serde_json::json!({
            "config": {
                "wide_screen_mode": true,
                "enable_forward": false
            },
            "header": {
                "title": {
                    "tag": "plain_text",
                    "content": card.title
                },
                "template": feishu_template(card.tone)
            },
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!(
                        "**投递数**：{}\n**类别**：{}\n\n{}",
                        card.delivery_count,
                        card.kind_summary,
                        card.body
                    )
                },
                {
                    "tag": "note",
                    "elements": [
                        {
                            "tag": "plain_text",
                            "content": "Axon 仅输出 bounded summary；原始载荷 与原始交互流保持关闭。"
                        }
                    ]
                }
            ]
        });
        let text = serde_json::to_string(&payload)
            .map_err(|_| AxonError::runtime_failure("飞书远控卡片序列化失败"))?;
        if text.trim().is_empty()
            || text.chars().count() > REMOTE_NOTIFICATION_CARD_BODY_MAX_CHARS * 8
        {
            return Err(AxonError::runtime_failure("飞书远控卡片载荷无效"));
        }
        Ok(text)
    }
}

fn validate_feishu_card(card: &RemoteNotificationCard) -> AxonResult<()> {
    if card.transport != RemoteTransportKind::Feishu
        || card.surface != RemoteNotificationAdapterEnvelopeSurface::FeishuCard
        || !card.local_only
        || card.network_send_allowed
        || card.raw_payload_forward_allowed
    {
        return Err(AxonError::runtime_failure("飞书远控卡片安全边界无效"));
    }
    if card.title.trim().is_empty()
        || card.title.chars().count() > REMOTE_NOTIFICATION_CARD_TITLE_MAX_CHARS
        || card.body.trim().is_empty()
        || card.body.chars().count() > REMOTE_NOTIFICATION_CARD_BODY_MAX_CHARS
        || card.kind_summary.trim().is_empty()
        || card.kind_summary.chars().count() > REMOTE_NOTIFICATION_CARD_KIND_SUMMARY_MAX_CHARS
    {
        return Err(AxonError::runtime_failure("飞书远控卡片文本无效"));
    }
    Ok(())
}

fn feishu_template(tone: RemoteNotificationCardTone) -> &'static str {
    match tone {
        RemoteNotificationCardTone::Info => "blue",
        RemoteNotificationCardTone::Warning => "orange",
        RemoteNotificationCardTone::Danger => "red",
    }
}
