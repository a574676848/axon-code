//! Axon 对外请求的客户端身份与 User-Agent 构造。

/// Axon 请求入口渠道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientChannel {
    /// 终端 UI。
    Tui,
    /// 命令行。
    Cli,
    /// 桌面工作台。
    Desktop,
    /// 飞书。
    Feishu,
    /// Memory 后台能力。
    Memory,
}

impl ClientChannel {
    /// 从 transport context 的渠道字符串解析入口。
    #[must_use]
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).unwrap_or_default() {
            "tui" => Self::Tui,
            "desktop" => Self::Desktop,
            "feishu" => Self::Feishu,
            "memory" => Self::Memory,
            _ => Self::Cli,
        }
    }

    const fn product(self) -> &'static str {
        match self {
            Self::Tui => "axon-tui",
            Self::Cli => "axon-cli",
            Self::Desktop => "axon-desktop",
            Self::Feishu => "axon-feishu",
            Self::Memory => "axon-memory",
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
        assert_eq!(
            ClientChannel::parse(Some("desktop")),
            ClientChannel::Desktop
        );
        assert_eq!(ClientChannel::parse(Some("feishu")), ClientChannel::Feishu);
        assert_eq!(ClientChannel::parse(Some("unknown")), ClientChannel::Cli);
    }

    #[test]
    fn every_entry_channel_has_its_own_product_name() {
        // 上游据此分辨请求来自哪个入口，两两不得重名。
        let names = [
            ClientChannel::Tui.product(),
            ClientChannel::Cli.product(),
            ClientChannel::Desktop.product(),
            ClientChannel::Feishu.product(),
            ClientChannel::Memory.product(),
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
    }
}
