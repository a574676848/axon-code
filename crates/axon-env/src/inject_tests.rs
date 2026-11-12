use std::fs;

use super::*;
use crate::local_resources::{GitIdentity, LocalResources, SshHost, scan};

fn sample_resources() -> LocalResources {
    LocalResources {
        ssh_hosts: vec![SshHost {
            alias: "prod".to_owned(),
            hostname: Some("10.0.0.11".to_owned()),
            user: Some("deploy".to_owned()),
            port: Some(2222),
        }],
        ssh_keys: vec!["id_rsa".to_owned()],
        known_hosts: vec!["github.com".to_owned()],
        git_identity: Some(GitIdentity {
            name: "Alice".to_owned(),
            email: "a***@example.com".to_owned(),
        }),
    }
}

#[test]
fn trigger_words_gate_injection() {
    assert!(should_inject("帮我上服务器看磁盘"));
    assert!(should_inject("先 git status 一下"));
    assert!(should_inject("这个服务部署到哪"));
    assert!(!should_inject("今天天气怎么样"));
}

#[test]
fn disabled_flag_never_renders() {
    let resources = sample_resources();
    assert!(render_block_if_enabled("帮我上服务器看磁盘", &resources, false).is_none());
    // 常量断言写成 const 块：运行期没有任何信息量，编译期失败才是它的作用。
    const { assert!(MEMORY_ENV_ENABLED_DEFAULT, "本机资源感知常量默认必须开启") };
}

#[test]
fn disabled_flag_skips_scanning_entirely() {
    // 传入不存在的 home：关闭状态下不得触发任何读取，也不得 panic。
    let missing = std::path::Path::new("this-home-does-not-exist");
    assert!(maybe_inject(missing, "帮我上服务器看磁盘", false).is_none());
    assert!(maybe_inject(missing, "今天天气", true).is_none());
}

#[test]
fn rendered_block_contains_aliases_and_direct_hint() {
    let block = render_block_if_enabled("帮我上服务器看磁盘", &sample_resources(), true)
        .expect("命中触发词应渲染");
    assert!(block.starts_with("## 本机资源"));
    assert!(block.contains("SSH 别名：prod（deploy@10.0.0.11@2222）"));
    assert!(block.contains("免密登录已配置，优先直连而不是先问凭据"));
}

#[test]
fn non_triggering_text_does_not_render() {
    assert!(render_block_if_enabled("今天天气怎么样", &sample_resources(), true).is_none());
}

#[test]
fn rendered_block_has_no_secrets_or_paths() {
    let block = render_block_if_enabled("帮我上服务器看磁盘", &sample_resources(), true)
        .expect("命中触发词应渲染");
    assert!(!block.contains("PRIVATE KEY"));
    assert!(!block.contains("alice@example.com"));
    assert!(block.contains("a***@example.com"));
    for path_marker in ["C:\\", "/home/", "/Users/", ".ssh", ".gitconfig"] {
        assert!(
            !block.contains(path_marker),
            "注入块不得包含路径片段 {path_marker}：{block}"
        );
    }
}

#[test]
fn rendered_block_is_byte_stable() {
    let resources = sample_resources();
    let first = render_block(&resources);
    let second = render_block(&resources);
    let third = render_block(&resources);
    assert_eq!(first, second);
    assert_eq!(second, third);
}

#[test]
fn rendered_block_respects_token_budget() {
    let resources = LocalResources {
        ssh_hosts: (0..20)
            .map(|index| SshHost {
                alias: format!("host{index}"),
                hostname: Some(format!("10.0.0.{index}")),
                user: Some("deploy".to_owned()),
                port: None,
            })
            .collect(),
        ssh_keys: Vec::new(),
        known_hosts: (0..30)
            .map(|index| format!("known-{index}.internal"))
            .collect(),
        git_identity: None,
    };
    let block = render_block(&resources);
    assert!(block.len() <= ENV_BLOCK_MAX_BYTES);
}

#[test]
fn empty_resources_render_nothing() {
    assert_eq!(render_block(&LocalResources::default()), "");
    assert!(render_block_if_enabled("部署", &LocalResources::default(), true).is_none());
}

#[test]
fn end_to_end_scan_then_render_has_no_full_email() {
    let home = std::env::temp_dir().join(format!(
        "axon-env-inject-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(home.join(".ssh")).unwrap();
    fs::write(
        home.join(".ssh").join("config"),
        "Host prod\n    HostName 10.0.0.11\n    User deploy\n",
    )
    .unwrap();
    fs::write(
        home.join(".gitconfig"),
        "[user]\n    name = Alice\n    email = alice@example.com\n",
    )
    .unwrap();

    let block = maybe_inject(&home, "帮我上服务器看磁盘", true).expect("应渲染");
    assert!(block.contains("prod"));
    assert!(!block.contains("alice@example.com"));
    assert!(block.contains("a***@example.com"));
    assert!(!block.contains(&home.to_string_lossy().to_string()));

    let resources = scan(&home);
    assert_eq!(resources.ssh_hosts.len(), 1);
}
