//! `/api/v1/keys` 管理面的契约测试。
//!
//! 覆盖七组必测口径：超管三条路由的形状、普通 Key 一律 403、明文只出现在签发回执、
//! 吊销后同一 Key 不再命中、未命中路径返回 None、方法不匹配 405、入参与未知 id 的分档。

use std::sync::Arc;

use axon_daemon::GatewayHttpRouteHandler;
use axon_daemon::infrastructure::gateway_auth::GatewayIdentity;
use serde_json::{Value, json};

use super::*;

/// 测试用超管身份。
fn super_admin() -> GatewayIdentity {
    GatewayIdentity::SuperAdmin
}

/// 测试用普通 Key 身份。
fn plain_key() -> GatewayIdentity {
    GatewayIdentity::Key {
        id: "3f2b-test-key".to_owned(),
        label: "测试签发的 Key".to_owned(),
    }
}

/// 建一个隔离的存储句柄；临时目录随返回值一起存活。
fn test_store() -> (tempfile::TempDir, Arc<GatewayKeyStore>) {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    (root, store)
}

/// 按身份发一次管理面请求。
fn call(
    adapter: &KeysHttpAdapter,
    method: &str,
    path: &str,
    body: &[u8],
    identity: &GatewayIdentity,
) -> GatewayHttpResponse {
    adapter
        .handle_request_as(method, path, None, body, Some(identity), None)
        .expect("命中管理面路径")
}

/// 解析响应体。
fn body_of(response: &GatewayHttpResponse) -> Value {
    serde_json::from_slice(&response.body).expect("响应体应是合法 JSON")
}

/// 签发一把 Key 并返回回执体。
fn issue(adapter: &KeysHttpAdapter, label: &str) -> Value {
    let payload = json!({ "label": label }).to_string();
    let response = call(
        adapter,
        "POST",
        KEYS_PATH,
        payload.as_bytes(),
        &super_admin(),
    );
    assert_eq!(response.status, 200);
    body_of(&response)
}

#[test]
fn super_admin_lists_creates_and_revokes_with_expected_shapes() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(Arc::clone(&store));

    let created = issue(&adapter, "部署机 A");
    let id = created["id"].as_str().expect("回执带 id");
    assert_eq!(created["label"], "部署机 A");
    assert!(
        created["plaintext"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "签发回执必须带明文"
    );
    assert!(created["createdAtMs"].as_u64().is_some_and(|ms| ms > 0));

    let listed = call(&adapter, "GET", KEYS_PATH, b"", &super_admin());
    assert_eq!(listed.status, 200);
    assert_eq!(listed.content_type, "application/json");
    let payload = body_of(&listed);
    let keys = payload["keys"].as_array().expect("列表是数组");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["id"], id);
    assert_eq!(keys[0]["label"], "部署机 A");
    assert_eq!(
        keys[0]["createdAtMs"], created["createdAtMs"],
        "列表与签发回执的创建时间必须同源"
    );
    assert!(keys[0]["lastUsedAtMs"].is_null(), "从未使用应为 null");
    assert_eq!(
        keys[0]["fingerprint"].as_str().map(str::len),
        Some(6),
        "指纹前 6 位"
    );

    let revoked = call(
        &adapter,
        "DELETE",
        &format!("{KEYS_ID_PREFIX}{id}"),
        b"",
        &super_admin(),
    );
    assert_eq!(revoked.status, 200);
    assert_eq!(body_of(&revoked)["id"], id);
    assert_eq!(body_of(&revoked)["revoked"], true);

    let after = body_of(&call(&adapter, "GET", KEYS_PATH, b"", &super_admin()));
    assert!(
        after["keys"].as_array().is_some_and(|keys| keys.is_empty()),
        "吊销后列表不再含该 Key"
    );
}

#[test]
fn plain_key_is_forbidden_on_every_management_route() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(Arc::clone(&store));
    let issued = store.create("已存在的 Key").expect("签发");
    let key = plain_key();

    let label_body = json!({ "label": "不该签发成功" }).to_string();
    let cases = [
        ("GET", KEYS_PATH.to_owned(), Vec::new()),
        ("POST", KEYS_PATH.to_owned(), label_body.into_bytes()),
        (
            "DELETE",
            format!("{KEYS_ID_PREFIX}{}", issued.id),
            Vec::new(),
        ),
    ];
    for (method, path, body) in cases {
        let response = call(&adapter, method, &path, &body, &key);
        assert_eq!(response.status, 403, "{method} {path} 必须 403");
        assert_eq!(
            body_of(&response)["error"],
            "forbidden",
            "403 的错误码必须是小写 forbidden"
        );
    }

    // 被拒的签发与吊销都不能产生副作用。
    assert_eq!(store.list().len(), 1);
    assert!(
        store.verify(&issued.plaintext).is_some(),
        "被拒的吊销不得真的吊销"
    );
}

#[test]
fn plaintext_appears_only_in_the_issue_receipt() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(Arc::clone(&store));

    let created = issue(&adapter, "只回一次");
    let plaintext = created["plaintext"].as_str().expect("回执带明文");
    assert!(
        store.verify(plaintext).is_some(),
        "回执里的明文必须能通过鉴权"
    );

    let listed = call(&adapter, "GET", KEYS_PATH, b"", &super_admin());
    let text = String::from_utf8(listed.body.clone()).expect("列表是 UTF-8");
    assert!(!text.contains(plaintext), "列表不得回明文：{text}");

    // 列表只回五个字段，且不含任何摘要字段。
    let payload = body_of(&listed);
    let entry = payload["keys"][0].as_object().expect("列表项是对象");
    let mut fields: Vec<&str> = entry.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        vec!["createdAtMs", "fingerprint", "id", "label", "lastUsedAtMs"]
    );
    assert!(!text.contains("salt"), "列表不得回盐");
    assert!(!text.contains("hash"), "列表不得回摘要");
}

#[test]
fn revoking_one_key_leaves_the_others_usable() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(Arc::clone(&store));

    let first = issue(&adapter, "第一把");
    let second = issue(&adapter, "第二把");
    let first_plaintext = first["plaintext"].as_str().expect("明文");
    let second_plaintext = second["plaintext"].as_str().expect("明文");
    let first_id = first["id"].as_str().expect("id");

    let response = call(
        &adapter,
        "DELETE",
        &format!("{KEYS_ID_PREFIX}{first_id}"),
        b"",
        &super_admin(),
    );
    assert_eq!(response.status, 200);

    assert!(
        store.verify(first_plaintext).is_none(),
        "吊销后同一 Key 的校验必须不再命中"
    );
    assert!(
        store.verify(second_plaintext).is_some(),
        "其他 Key 仍应可用"
    );
}

#[test]
fn unmatched_paths_leave_the_composite_alone() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(store);

    for path in [
        "/api/v1/keysx",
        "/api/v1/key",
        "/api/v1/workspaces",
        "/api/v1/keys/",
        "/api/v1/keys/abc/def",
    ] {
        assert!(
            adapter
                .handle_request_as("GET", path, None, b"", Some(&super_admin()), None)
                .is_none(),
            "{path} 必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_is_reported_as_405() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(Arc::clone(&store));
    let issued = store.create("方法判定").expect("签发");

    for (method, path) in [
        ("PUT", KEYS_PATH.to_owned()),
        ("DELETE", KEYS_PATH.to_owned()),
        ("PATCH", KEYS_PATH.to_owned()),
        ("GET", format!("{KEYS_ID_PREFIX}{}", issued.id)),
        ("POST", format!("{KEYS_ID_PREFIX}{}", issued.id)),
    ] {
        let response = call(&adapter, method, &path, b"", &super_admin());
        assert_eq!(response.status, 405, "{method} {path} 必须 405");
        assert_eq!(body_of(&response)["error"], "method_not_allowed");
    }
}

#[test]
fn missing_or_blank_label_and_broken_json_are_rejected() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(store);

    let broken = call(&adapter, "POST", KEYS_PATH, b"not-json", &super_admin());
    assert_eq!(broken.status, 400);
    assert_eq!(body_of(&broken)["error"], "invalid_json");

    for payload in [json!({}), json!({ "label": "   " }), json!({ "label": 7 })] {
        let response = call(
            &adapter,
            "POST",
            KEYS_PATH,
            payload.to_string().as_bytes(),
            &super_admin(),
        );
        assert_eq!(response.status, 400, "{payload} 必须 400");
        let body = body_of(&response);
        assert_eq!(body["error"], "missing_label");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "missing_label 必须带中文原因"
        );
    }
}

#[test]
fn unknown_key_id_is_reported_as_not_found() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(Arc::clone(&store));
    let issued = store.create("先签发再吊销").expect("签发");
    let path = format!("{KEYS_ID_PREFIX}{}", issued.id);

    assert_eq!(
        call(&adapter, "DELETE", &path, b"", &super_admin()).status,
        200
    );
    // 第二次吊销与不存在的 id 同一档：列表里都看不到它。
    for bogus in [
        path,
        format!("{KEYS_ID_PREFIX}不存在的标识"),
        format!("{KEYS_ID_PREFIX}不存在的标识 "),
    ] {
        let response = call(&adapter, "DELETE", &bogus, b"", &super_admin());
        assert_eq!(response.status, 404, "{bogus} 必须 404");
        assert_eq!(body_of(&response)["error"], "key_not_found");
    }
}

#[test]
fn identity_less_calls_never_pass_the_management_surface() {
    let (_root, store) = test_store();
    let adapter = KeysHttpAdapter::new(store);

    let response = adapter
        .handle_request("GET", KEYS_PATH, None, b"")
        .expect("命中管理面路径");
    assert_eq!(response.status, 403);
    assert_eq!(body_of(&response)["error"], "forbidden");
}
