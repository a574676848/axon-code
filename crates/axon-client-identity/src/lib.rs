//! Axon 对外请求的客户端身份与 User-Agent 构造。
//!
//! 这里标识的是**本轮从哪个入口发起**（轮次事实），不是会话长期属于哪个渠道：
//! 同一条会话被另一端续聊时，User-Agent 必须换成续聊那一端。

/// Axon 请求入口渠道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientChannel {
    /// 终端 UI。
    Tui,
    /// 命令行。
    Cli,
    /// 桌面工作台。
    Desktop,
    /// Webhook 车道。
    Webhook,
    /// 定时任务。
    Cron,
    /// 目标执行。
    Goal,
}

impl ClientChannel {
    /// 从本轮来源渠道的字符串解析入口。
    ///
    /// 入参应当是「客户端优先、没有客户端退回来源渠道」的那一份（transport context 的
    /// `originChannel`）；未知取值按命令行处理，不编造新的产品名。
    #[must_use]
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).unwrap_or_default() {
            "tui" => Self::Tui,
            "desktop" => Self::Desktop,
            "webhook" => Self::Webhook,
            "cron" => Self::Cron,
            "goal" => Self::Goal,
            _ => Self::Cli,
        }
    }

    const fn product(self) -> &'static str {
        match self {
            Self::Tui => "axon-tui",
            Self::Cli => "axon-cli",
            Self::Desktop => "axon-desktop",
            Self::Webhook => "axon-webhook",
            Self::Cron => "axon-cron",
            Self::Goal => "axon-goal",
        }
    }
}

/// 生成请求级 User-Agent。
#[must_use]
pub fn user_agent(channel: ClientChannel) -> String {
    format!(
        "{}/{} ({}; {})",
        channel.product(),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

#[cfg(test)]
mod tests {
    use super::{ClientChannel, user_agent};

    #[test]
    fn parses_known_channels_and_falls_back_to_cli() {
        assert_eq!(ClientChannel::parse(Some("tui")), ClientChannel::Tui);
        assert_eq!(ClientChannel::parse(Some("cli")), ClientChannel::Cli);
        assert_eq!(
            ClientChannel::parse(Some("desktop")),
            ClientChannel::Desktop
        );
        assert_eq!(
            ClientChannel::parse(Some("webhook")),
            ClientChannel::Webhook
        );
        assert_eq!(ClientChannel::parse(Some("cron")), ClientChannel::Cron);
        assert_eq!(ClientChannel::parse(Some("goal")), ClientChannel::Goal);
        assert_eq!(ClientChannel::parse(Some("unknown")), ClientChannel::Cli);
        assert_eq!(ClientChannel::parse(None), ClientChannel::Cli);
    }

    #[test]
    fn every_entry_channel_has_its_own_product_name() {
        // 上游据此分辨请求来自哪个入口，两两不得重名。
        let names = [
            ClientChannel::Tui.product(),
            ClientChannel::Cli.product(),
            ClientChannel::Desktop.product(),
            ClientChannel::Webhook.product(),
            ClientChannel::Cron.product(),
            ClientChannel::Goal.product(),
        ];
        let unique = names.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(
            unique.len(),
            names.len(),
            "入口渠道的 User-Agent 产品名必须唯一"
        );
    }

    #[test]
    fn user_agent_contains_product_version_and_platform() {
        let value = user_agent(ClientChannel::Tui);
        assert!(value.starts_with("axon-tui/"));
        assert!(value.contains(env!("CARGO_PKG_VERSION")));
        assert!(value.contains(std::env::consts::ARCH));
        assert!(user_agent(ClientChannel::Desktop).starts_with("axon-desktop/"));
        assert!(user_agent(ClientChannel::Cron).starts_with("axon-cron/"));
        assert!(user_agent(ClientChannel::Goal).starts_with("axon-goal/"));
        assert!(user_agent(ClientChannel::Webhook).starts_with("axon-webhook/"));
    }
}
