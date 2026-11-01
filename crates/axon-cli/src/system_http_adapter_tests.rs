//! 系统只读路由测试：`/meta` 载荷形状、能力位键集合、双路由时钟一致性、
//! 方法边界与未命中路径的回落。

use axon_daemon::GatewayHttpRouteHandler;
use axon_session::RuntimeStore;
use serde_json::Value;

use super::{
    CAPABILITY_NAMES, META_PATH, METHOD_NOT_ALLOWED, STATUS_PATH, SystemHttpAdapter, runtime_store,
};

/// 构造一个指向临时运行态根与临时配置路径的适配器。
///
/// 临时根里没有 `config.toml`，因此远端适配器清单走默认配置：飞书一路默认关闭。
fn fixture() -> (axon_test_support::TestTempDir, SystemHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = SystemHttpAdapter::new(
        directory.path().to_path_buf(),
        directory.path().join("config.toml"),
    );
    (directory, adapter)
}

/// 发一次带身份的 GET 并按 JSON 解析回执。
fn get_json_as(
    adapter: &SystemHttpAdapter,
    path: &str,
    identity: Option<&axon_daemon::infrastructure::gateway_auth::GatewayIdentity>,
) -> (u16, Value) {
    let response = adapter
        .handle_request_as("GET", path, None, &[], identity, None)
        .expect("系统路由必须命中");
    let body = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, body)
}

/// 发一次 GET 并按 JSON 解析回执（无身份入口）。
fn get_json(adapter: &SystemHttpAdapter, path: &str) -> (u16, Value) {
    let response = adapter
        .handle_request("GET", path, None, &[])
        .expect("系统路由必须命中");
    let body = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, body)
}

#[test]
fn meta_returns_exactly_the_five_contract_fields() {
    let (_directory, adapter) = fixture();
    let (status, body) = get_json_as(
        &adapter,
        META_PATH,
        Some(&axon_daemon::infrastructure::gateway_auth::GatewayIdentity::SuperAdmin),
    );
    assert_eq!(status, 200);

    let mut keys = body
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "capabilities",
            "protocolVersion",
            "serverTimeMs",
            "superAdmin",
            "uploadLimitBytes",
        ],
        "顶层键必须恰好是合同 §6.2 的五个"
    );

    assert_eq!(body["protocolVersion"], 1);
    assert_eq!(body["uploadLimitBytes"], 67_108_864_u64);
    assert_eq!(body["superAdmin"], true, "超管凭据必须自报为超管");
    assert!(
        body["serverTimeMs"].as_u64().is_some_and(|now| now > 0),
        "服务器时间必须是 Unix 毫秒时间戳"
    );
}

#[test]
fn meta_reports_super_admin_from_the_request_identity() {
    use axon_daemon::infrastructure::gateway_auth::GatewayIdentity;

    let (_directory, adapter) = fixture();

    // 普通 Key：不是超管，界面因此不渲染密钥管理入口。
    let key = GatewayIdentity::Key {
        id: "k1".to_owned(),
        label: "e2e".to_owned(),
    };
    let (_, body) = get_json_as(&adapter, META_PATH, Some(&key));
    assert_eq!(body["superAdmin"], false);

    // 没有身份（绕过路由层直调适配器）：一律按非超管处理，不得默认为超管。
    let (_, body) = get_json(&adapter, META_PATH);
    assert_eq!(body["superAdmin"], false);
}

#[test]
fn meta_capabilities_cover_exactly_the_twelve_contract_names() {
    let (_directory, adapter) = fixture();
    let (_status, body) = get_json(&adapter, META_PATH);

    let capabilities = body["capabilities"].as_object().expect("能力位是对象");
    let mut names = capabilities.keys().map(String::as_str).collect::<Vec<_>>();
    names.sort_unstable();
    let mut expected = CAPABILITY_NAMES.to_vec();
    expected.sort_unstable();
    assert_eq!(names, expected, "能力位键集合必须与合同 §4.5 逐字一致");
    assert!(
        capabilities
            .values()
            .all(|value| value == &Value::Bool(true)),
        "能力位取值先全部自报为 true"
    );
}

#[test]
fn both_routes_share_one_clock_within_one_second() {
    let (_directory, adapter) = fixture();
    let (_meta_status, meta) = get_json(&adapter, META_PATH);
    let (_status_status, status_payload) = get_json(&adapter, STATUS_PATH);

    let meta_ms = meta["serverTimeMs"].as_u64().expect("服务器时间");
    let status_ms = status_payload["serverTimeMs"].as_u64().expect("服务器时间");
    assert!(
        meta_ms.abs_diff(status_ms) <= 1_000,
        "两条路由的时间相差 {} 毫秒，超过合同 §6.2 的一秒判据",
        meta_ms.abs_diff(status_ms)
    );
}

#[test]
fn non_get_methods_are_rejected_with_405_on_both_paths() {
    let (_directory, adapter) = fixture();
    for path in [META_PATH, STATUS_PATH] {
        for method in ["POST", "PUT", "DELETE", "PATCH", "HEAD"] {
            let response = adapter
                .handle_request(method, path, None, b"{}")
                .expect("系统路径必须命中，不得落到后续适配器");
            assert_eq!(response.status, 405, "{method} {path}");
            let body = serde_json::from_slice::<Value>(&response.body).expect("正文必须是 JSON");
            assert_eq!(body["error"].as_str(), Some(METHOD_NOT_ALLOWED), "{method}");
        }
    }
}

#[test]
fn unmatched_paths_fall_through_to_the_next_adapter() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/nope",
        "/api/v1/system",
        "/api/v1/meta/",
        "/api/v1/meta/extra",
        "/api/v1/system/status2",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于系统路由，必须返回 None"
        );
    }
}

#[test]
fn status_payload_only_reports_readings_it_can_prove() {
    let (_directory, adapter) = fixture();
    let (status, body) = get_json(&adapter, STATUS_PATH);
    assert_eq!(status, 200);

    assert_eq!(
        body["buildVersion"].as_str(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(
        body["queueDepth"].as_u64().is_some(),
        "排队深度必须是一个真实读数"
    );
    assert!(
        body["uptimeMs"].as_u64().is_some(),
        "运行时长必须是一个真实读数"
    );

    // 租约只有两种合法形态：读不到时为 null，读到时为三个字段齐全的对象。
    let lease = &body["lease"];
    if !lease.is_null() {
        assert!(lease["active"].as_bool().is_some(), "active 必须是布尔值");
        assert!(lease.get("expiresInMs").is_some(), "expiresInMs 必须存在");
        assert!(lease.get("owner").is_some(), "owner 必须存在");
    }

    // 临时根里没有 config.toml：飞书一路按默认关闭上报，而不是凭空为真。
    let adapters = body["remoteAdapters"].as_array().expect("远端适配器是数组");
    let feishu = adapters
        .iter()
        .find(|entry| entry["name"] == "feishu")
        .expect("飞书一路必须在清单里");
    assert_eq!(feishu["enabled"], false);
}

#[test]
fn lease_reports_the_live_host_claim_of_the_runtime_store() {
    const LEASE_DURATION_MS: u64 = 30_000;
    let (directory, adapter) = fixture();
    // 真实领取一次租约：适配器读的必须是守护进程写进运行态库的那一行，
    // 而不是任何内存里的副本。
    let host_id = axon_domain::RuntimeHostId::new("axon-daemon-test").expect("Host 标识");
    runtime_store(directory.path())
        .claim_host(
            &host_id,
            4_242,
            None,
            super::server_now_ms(),
            LEASE_DURATION_MS,
        )
        .expect("领取租约");

    let (_status, body) = get_json(&adapter, STATUS_PATH);
    let lease = &body["lease"];
    assert_eq!(lease["active"], true, "刚领取的租约必须判定为生效");
    assert!(
        lease["owner"]
            .as_str()
            .is_some_and(|owner| owner.starts_with("axon-daemon-test:")),
        "租约拥有者要带上 Host 标识与 generation：{lease}"
    );
    let expires_in_ms = lease["expiresInMs"].as_u64().expect("剩余时长");
    assert!(
        expires_in_ms <= LEASE_DURATION_MS,
        "剩余时长不得超过租约时长：{expires_in_ms}"
    );
}
