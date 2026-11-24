//! 端点解析的纯函数级断言：不读本机真实配置，全部走显式入参。

use super::{normalize_cloud_address, probe_target};

#[test]
fn wildcard_bind_addresses_map_to_loopback_targets() {
    assert_eq!(probe_target("0.0.0.0:3143"), "127.0.0.1:3143");
    assert_eq!(probe_target("[::]:3144"), "[::1]:3144");
    assert_eq!(probe_target("::3144"), "[::1]:3144");
    assert_eq!(probe_target(" 127.0.0.1:3143 "), "127.0.0.1:3143");
}

#[test]
fn bare_domain_gets_https_and_default_port() {
    let address = normalize_cloud_address("axon.example.com").expect("应可规范化");
    assert_eq!(address.http_base, "https://axon.example.com");
    assert_eq!(address.ws_base, "wss://axon.example.com");
    assert!(!address.plaintext_private);
}

#[test]
fn explicit_port_is_preserved_and_websocket_scheme_follows_http() {
    let address = normalize_cloud_address("https://axon.example.com:8443").expect("应可规范化");
    assert_eq!(address.http_base, "https://axon.example.com:8443");
    assert_eq!(address.ws_base, "wss://axon.example.com:8443");

    // 用户直接粘贴 ws / wss 地址时等价换算，不报错。
    let pasted = normalize_cloud_address("wss://axon.example.com").expect("应可规范化");
    assert_eq!(pasted.http_base, "https://axon.example.com");
    assert_eq!(pasted.ws_base, "wss://axon.example.com");
}

#[test]
fn plaintext_http_is_allowed_on_any_host_and_only_flagged() {
    // 程序不做网络层限制：能不能连通、要不要 TLS 由部署方的网络与运维策略决定。
    // 私网与公网一律接受，`plaintext_private` 只用于界面提示，不构成拒绝。
    for allowed in [
        "http://localhost:3143",
        "http://127.0.0.1:3143",
        "http://10.1.2.3",
        "http://172.16.0.9",
        "http://192.168.1.20:8080",
        "http://[::1]:3143",
        "http://axon.example.com",
        "http://203.0.113.9:3143",
    ] {
        let address = normalize_cloud_address(allowed)
            .unwrap_or_else(|error| panic!("{allowed} 应被接受：{error}"));
        assert!(address.http_base.starts_with("http://"), "{allowed}");
        assert!(address.plaintext_private, "{allowed} 应标记为明文地址");
    }
}

#[test]
fn empty_or_path_bearing_addresses_are_rejected_with_actionable_messages() {
    assert!(normalize_cloud_address("   ").is_err());

    let with_path =
        normalize_cloud_address("https://axon.example.com/api/v1").expect_err("带路径必须被拒绝");
    assert!(with_path.contains("路径"), "{with_path}");

    let with_query = normalize_cloud_address("https://axon.example.com/?token=x")
        .expect_err("带查询串必须被拒绝");
    assert!(with_query.contains("查询串"), "{with_query}");
}

#[test]
fn ws_port_is_rewritten_from_the_backend_report_without_touching_host_or_scheme() {
    // 只填一个 HTTP 地址时，事件流端口来自后端自报：主机与协议保持用户填写的那一份。
    assert_eq!(
        super::ws_base_with_port("ws://192.168.1.20:3143", 3144),
        "ws://192.168.1.20:3144"
    );
    assert_eq!(
        super::ws_base_with_port("wss://axon.example.com", 8443),
        "wss://axon.example.com:8443"
    );
    // 端口本来就对时原样返回，不产生一次无意义的字符串重写。
    assert_eq!(
        super::ws_base_with_port("ws://127.0.0.1:3144", 3144),
        "ws://127.0.0.1:3144"
    );
    // 解析不了的输入原样返回，交给探测环节如实报连不上。
    assert_eq!(super::ws_base_with_port("不是地址", 3144), "不是地址");
}

#[test]
fn plaintext_addresses_of_any_host_are_accepted() {
    // 能不能连通、要不要 TLS 由部署方的网络与运维策略决定，客户端不做网络层限制。
    let public =
        normalize_cloud_address("http://203.0.113.9:3143").expect("公网明文地址应可规范化");
    assert_eq!(public.http_base, "http://203.0.113.9:3143");
    assert_eq!(public.ws_base, "ws://203.0.113.9:3143");
    assert!(public.plaintext_private, "只用于界面提示，不构成拒绝");
}

#[test]
fn ws_base_converts_back_to_its_http_form_for_port_probing() {
    assert_eq!(
        super::ws_base_to_http("wss://axon.example.com:8443"),
        "https://axon.example.com:8443"
    );
    assert_eq!(
        super::ws_base_to_http("ws://127.0.0.1:3144"),
        "http://127.0.0.1:3144"
    );
}
#[test]
fn resolving_endpoints_sixty_times_reads_the_config_at_most_sixty_times() {
    /*
     * P1：一次请求只允许解析一次端点，60 次请求的读盘次数上限就是 60。
     * 这里钉的是「没有隐藏的重复解析」，而不是具体耗时。
     */
    use crate::connection::endpoint::CONFIG_FILE_READS;

    let before = CONFIG_FILE_READS.with(|reads| reads.get());
    for _ in 0..60 {
        let _ = super::local_endpoints();
    }
    let reads = CONFIG_FILE_READS.with(|reads| reads.get()) - before;
    assert!(
        reads <= 60,
        "60 次端点解析最多读 60 次配置，实际 {reads} 次"
    );
    assert_eq!(reads, 60, "每次解析恰好读一次配置，既不多也不少");
}
