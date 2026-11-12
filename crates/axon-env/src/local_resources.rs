//! 本机资源扫描（M4-1）。
//!
//! 只读元数据：SSH 配置里的别名/主机/用户/端口、`*.pub` 配对私钥的**文件名**、
//! known_hosts 的主机名、git 身份。私钥内容、known_hosts 指纹、shell history、
//! 浏览器数据一律不读。

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::redact::redact_email;

/// SSH 主机别名上限。
pub const MAX_SSH_HOSTS: usize = 20;
/// known_hosts 主机名上限。
pub const MAX_KNOWN_HOSTS: usize = 30;

/// 一条 SSH 主机配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    /// `Host` 别名（不含通配模式）。
    pub alias: String,
    /// `HostName`。
    pub hostname: Option<String>,
    /// `User`。
    pub user: Option<String>,
    /// `Port`。
    pub port: Option<u16>,
}

/// git 身份；email 已在扫描期脱敏。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitIdentity {
    /// `user.name`。
    pub name: String,
    /// 脱敏后的 `user.email`。
    pub email: String,
}

/// 本机资源快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalResources {
    /// SSH 主机别名列表。
    pub ssh_hosts: Vec<SshHost>,
    /// 存在 `.pub` 配对的私钥文件名（只有文件名）。
    pub ssh_keys: Vec<String>,
    /// known_hosts 中的非 hashed 主机名（去重）。
    pub known_hosts: Vec<String>,
    /// git 身份。
    pub git_identity: Option<GitIdentity>,
}

impl LocalResources {
    /// 是否没有任何可用资源。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ssh_hosts.is_empty()
            && self.ssh_keys.is_empty()
            && self.known_hosts.is_empty()
            && self.git_identity.is_none()
    }
}

/// 扫描 `home` 下的 SSH 与 git 元数据；任何读取失败都按「没有该项」处理。
#[must_use]
pub fn scan(home: &Path) -> LocalResources {
    let ssh_dir = home.join(".ssh");
    let ssh_hosts = read_text(&ssh_dir.join("config"))
        .map(|text| parse_ssh_config(&text))
        .unwrap_or_default();
    let ssh_keys = scan_ssh_key_names(&ssh_dir);
    let known_hosts = read_text(&ssh_dir.join("known_hosts"))
        .map(|text| parse_known_hosts(&text))
        .unwrap_or_default();
    let git_identity =
        read_text(&home.join(".gitconfig")).and_then(|text| parse_git_identity(&text));
    LocalResources {
        ssh_hosts,
        ssh_keys,
        known_hosts,
        git_identity,
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// 解析 `~/.ssh/config`：跳过 `Host *` 与任何含通配符的块。
#[must_use]
pub fn parse_ssh_config(text: &str) -> Vec<SshHost> {
    let mut hosts = Vec::new();
    let mut current: Option<SshHost> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = parts.next().unwrap_or_default().to_ascii_lowercase();
        let value = parts.next().unwrap_or_default().trim();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "host" => {
                if let Some(host) = current.take() {
                    push_host(&mut hosts, host);
                }
                // 只取第一个非通配别名；含通配符的块整体跳过。
                let alias = value.split_whitespace().next().unwrap_or_default();
                if alias.is_empty() || alias.contains('*') || alias.contains('?') {
                    continue;
                }
                current = Some(SshHost {
                    alias: alias.to_owned(),
                    hostname: None,
                    user: None,
                    port: None,
                });
            }
            "hostname" => {
                if let Some(host) = current.as_mut() {
                    host.hostname = Some(value.to_owned());
                }
            }
            "user" => {
                if let Some(host) = current.as_mut() {
                    host.user = Some(value.to_owned());
                }
            }
            "port" => {
                if let Some(host) = current.as_mut() {
                    host.port = value.parse::<u16>().ok();
                }
            }
            _ => {}
        }
    }
    if let Some(host) = current.take() {
        push_host(&mut hosts, host);
    }
    hosts
}

fn push_host(hosts: &mut Vec<SshHost>, host: SshHost) {
    if hosts.len() >= MAX_SSH_HOSTS {
        return;
    }
    if host.alias.is_empty() {
        return;
    }
    hosts.push(host);
}

/// 列出存在 `.pub` 配对的私钥文件名；只读目录项，不读文件内容。
#[must_use]
pub fn scan_ssh_key_names(ssh_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(ssh_dir) else {
        return Vec::new();
    };
    let mut names = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".pub").map(str::to_owned)
        })
        .filter(|base| ssh_dir.join(format!("{base}.pub")).is_file())
        .filter(|base| ssh_dir.join(base).is_file())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// 解析 known_hosts：只取主机名，跳过 hashed（`|` 开头）与无法识别的行。
#[must_use]
pub fn parse_known_hosts(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('|') {
            continue;
        }
        let Some(field) = line.split_whitespace().next() else {
            continue;
        };
        for host in field.split(',') {
            let host = host.trim();
            if host.is_empty() || host.starts_with('|') {
                continue;
            }
            seen.insert(host.to_owned());
            if seen.len() >= MAX_KNOWN_HOSTS {
                return seen.into_iter().collect();
            }
        }
    }
    seen.into_iter().collect()
}

/// 解析 `.gitconfig` 的 `[user]` 段；email 立即脱敏。
#[must_use]
pub fn parse_git_identity(text: &str) -> Option<GitIdentity> {
    let mut in_user = false;
    let mut name: Option<String> = None;
    let mut email: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            in_user = line
                .trim_start_matches('[')
                .split(']')
                .next()
                .is_some_and(|section| section.trim().eq_ignore_ascii_case("user"));
            continue;
        }
        if !in_user {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "name" if name.is_none() => name = Some(value.trim().to_owned()),
            "email" if email.is_none() => email = Some(redact_email(value)),
            _ => {}
        }
    }
    let name = name?;
    let email = email?;
    Some(GitIdentity { name, email })
}

#[cfg(test)]
#[path = "local_resources_tests.rs"]
mod tests;
