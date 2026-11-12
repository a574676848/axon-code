use std::fs;
use std::path::PathBuf;

use super::*;

fn temp_home(label: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "axon-env-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(home.join(".ssh")).unwrap();
    home
}

const SSH_CONFIG: &str = r#"
# 全局默认，必须跳过
Host *
    ServerAliveInterval 30

Host prod
    HostName 10.0.0.11
    User deploy
    Port 2222

host staging
  hostname staging.internal
  user ubuntu

Host *.internal
    User ignored
"#;

const KNOWN_HOSTS: &str = r#"
github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIexample
gitlab.com,alt.gitlab.com ssh-rsa AAAAB3NzaC1yc2Eexample
|1|aGFzaGVk|aGFzaGVk ssh-ed25519 AAAAC3NzaHashedExample
github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIexample
"#;

const GITCONFIG: &str = r#"
[core]
    editor = vim
[user]
    name = Alice Zhang
    email = alice@example.com
[alias]
    st = status
"#;

#[test]
fn ssh_config_parses_aliases_and_skips_wildcards() {
    let hosts = parse_ssh_config(SSH_CONFIG);
    assert_eq!(hosts.len(), 2, "通配块必须被跳过：{hosts:?}");
    assert_eq!(hosts[0].alias, "prod");
    assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.11"));
    assert_eq!(hosts[0].user.as_deref(), Some("deploy"));
    assert_eq!(hosts[0].port, Some(2222));
    assert_eq!(hosts[1].alias, "staging");
    assert_eq!(hosts[1].hostname.as_deref(), Some("staging.internal"));
    assert_eq!(hosts[1].user.as_deref(), Some("ubuntu"));
    assert_eq!(hosts[1].port, None);
}

#[test]
fn known_hosts_skips_hashed_lines_and_dedups() {
    let hosts = parse_known_hosts(KNOWN_HOSTS);
    assert_eq!(hosts, vec!["alt.gitlab.com", "github.com", "gitlab.com"]);
    assert!(!hosts.iter().any(|host| host.starts_with('|')));
    assert!(!hosts.iter().any(|host| host.contains("AAAAC3")));
}

#[test]
fn git_identity_redacts_email_at_parse_time() {
    let identity = parse_git_identity(GITCONFIG).expect("应解析到 user 段");
    assert_eq!(identity.name, "Alice Zhang");
    assert_eq!(identity.email, "a***@example.com");
    assert!(!identity.email.contains("alice@example.com"));
}

#[test]
fn git_identity_requires_both_fields() {
    assert!(parse_git_identity("[user]\n name = Bob\n").is_none());
    assert!(parse_git_identity("[core]\n email = x@y.com\n").is_none());
}

#[test]
fn key_scan_lists_only_paired_file_names() {
    let home = temp_home("keys");
    let ssh = home.join(".ssh");
    fs::write(ssh.join("id_rsa"), b"-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n").unwrap();
    fs::write(ssh.join("id_rsa.pub"), b"ssh-ed25519 AAAAexample\n").unwrap();
    fs::write(ssh.join("id_ed25519"), b"-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n").unwrap();

    let names = scan_ssh_key_names(&ssh);
    assert_eq!(names, vec!["id_rsa"], "没有 .pub 配对的私钥不应出现");
}

#[test]
fn full_scan_collects_all_sections_without_leaking_secrets() {
    let home = temp_home("scan");
    let ssh = home.join(".ssh");
    fs::write(ssh.join("config"), SSH_CONFIG).unwrap();
    fs::write(ssh.join("known_hosts"), KNOWN_HOSTS).unwrap();
    fs::write(ssh.join("id_rsa"), b"-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n").unwrap();
    fs::write(ssh.join("id_rsa.pub"), b"ssh-ed25519 AAAAexample\n").unwrap();
    fs::write(home.join(".gitconfig"), GITCONFIG).unwrap();

    let resources = scan(&home);
    assert_eq!(resources.ssh_hosts.len(), 2);
    assert_eq!(resources.ssh_keys, vec!["id_rsa"]);
    assert_eq!(resources.known_hosts.len(), 3);
    assert!(resources.git_identity.is_some());
    assert!(!resources.is_empty());

    let debug = format!("{resources:?}");
    assert!(!debug.contains("PRIVATE KEY"), "调试输出不得包含私钥标记");
    assert!(
        !debug.contains("alice@example.com"),
        "调试输出不得包含完整 email"
    );
    assert!(!debug.contains("AAAAC3"), "调试输出不得包含 known_hosts 指纹");
}

#[test]
fn missing_home_yields_empty_resources() {
    let resources = scan(std::path::Path::new("this-path-does-not-exist-axon-env"));
    assert!(resources.is_empty());
}

#[test]
fn ssh_hosts_are_capped() {
    let mut config = String::new();
    for index in 0..30 {
        config.push_str(&format!("Host host{index}\n    HostName 10.0.0.{index}\n"));
    }
    let hosts = parse_ssh_config(&config);
    assert_eq!(hosts.len(), MAX_SSH_HOSTS);
}
