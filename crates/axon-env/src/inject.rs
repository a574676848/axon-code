//! 触发词判定与注入块渲染（M4-3）。

use crate::local_resources::LocalResources;

/// 本机资源感知的默认状态：默认开启。
///
/// 需要停用时把该常量改为 `false` 并重编，或在调用点显式传入 `false`。
pub const MEMORY_ENV_ENABLED_DEFAULT: bool = true;

/// 注入块字节预算（≈400 token）。
pub const ENV_BLOCK_MAX_BYTES: usize = 1_600;

/// 触发注入的词表。
pub const ENV_TRIGGER_WORDS: [&str; 12] = [
    "上服务器",
    "服务器",
    "部署",
    "提交",
    "推代码",
    "git",
    "ssh",
    "装了什么",
    "软件",
    "环境",
    "代理",
    "vpn",
];

/// 是否命中注入触发词。
#[must_use]
pub fn should_inject(text: &str) -> bool {
    let lowered = text.to_lowercase();
    ENV_TRIGGER_WORDS.iter().any(|word| lowered.contains(word))
}

/// 开关打开且命中触发词时才渲染；否则返回 `None`，且不触发任何扫描。
#[must_use]
pub fn render_block_if_enabled(
    text: &str,
    resources: &LocalResources,
    enabled: bool,
) -> Option<String> {
    if !enabled || !should_inject(text) {
        return None;
    }
    let block = render_block(resources);
    (!block.is_empty()).then_some(block)
}

/// 组合入口：开关关闭时直接返回 `None`（零文件读写）；打开时扫描并渲染。
#[must_use]
pub fn maybe_inject(home: &std::path::Path, text: &str, enabled: bool) -> Option<String> {
    if !enabled || !should_inject(text) {
        return None;
    }
    let resources = crate::local_resources::scan(home);
    let block = render_block(&resources);
    (!block.is_empty()).then_some(block)
}

/// 渲染注入块：只含别名/主机名/身份，不含路径与凭据。
#[must_use]
pub fn render_block(resources: &LocalResources) -> String {
    let mut lines = Vec::new();
    if !resources.ssh_hosts.is_empty() {
        let aliases = resources
            .ssh_hosts
            .iter()
            .map(|host| {
                let mut parts = vec![host.alias.clone()];
                let mut detail = Vec::new();
                if let Some(user) = host.user.as_deref() {
                    detail.push(user.to_owned());
                }
                if let Some(hostname) = host.hostname.as_deref() {
                    detail.push(hostname.to_owned());
                }
                if let Some(port) = host.port {
                    detail.push(port.to_string());
                }
                if !detail.is_empty() {
                    parts.push(format!("（{}）", detail.join("@")));
                }
                parts.join("")
            })
            .collect::<Vec<_>>()
            .join("、");
        lines.push(format!("SSH 别名：{aliases}"));
    }
    if !resources.ssh_keys.is_empty() {
        lines.push(format!("已配对密钥：{}", resources.ssh_keys.join("、")));
    }
    if !resources.known_hosts.is_empty() {
        lines.push(format!("已知主机：{}", resources.known_hosts.join("、")));
    }
    if let Some(identity) = resources.git_identity.as_ref() {
        lines.push(format!("Git 身份：{} <{}>", identity.name, identity.email));
    }
    if lines.is_empty() {
        return String::new();
    }
    lines.push("免密登录已配置，优先直连而不是先问凭据。".to_owned());
    let mut block = format!("## 本机资源\n{}", lines.join("\n"));
    if block.len() > ENV_BLOCK_MAX_BYTES {
        let mut boundary = ENV_BLOCK_MAX_BYTES;
        while boundary > 0 && !block.is_char_boundary(boundary) {
            boundary -= 1;
        }
        block = block[..boundary].trim_end().to_owned();
    }
    block
}

#[cfg(test)]
#[path = "inject_tests.rs"]
mod tests;
