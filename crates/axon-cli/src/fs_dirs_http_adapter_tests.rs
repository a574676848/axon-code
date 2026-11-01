//! 目录浏览路由的契约测试。
//!
//! 覆盖回执形状、只列子目录、单层上限与 `truncated`、附件目录不可浏览、
//! 非法形态与不存在的目录、未命中回落与方法边界。

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{DIRS_PATH, FsDirsHttpAdapter};

/// 组装查询串，逐项做百分号编码，模拟真实客户端。
fn query(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

/// 发一次请求；未命中时必须返回 `None`。
fn call(
    adapter: &FsDirsHttpAdapter,
    method: &str,
    path: &str,
    query: Option<&str>,
) -> (u16, Value) {
    let response = adapter
        .handle_request(method, path, query, &[])
        .expect("目录浏览路由必须命中");
    let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, payload)
}

/// 以临时目录为运行时根构造适配器。
fn fixture() -> (axon_test_support::TestTempDir, FsDirsHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = FsDirsHttpAdapter::new(directory.path().to_path_buf());
    (directory, adapter)
}

/// 断言载荷里的错误文案是中文。
fn assert_chinese(payload: &Value) {
    let message = payload["message"].as_str().expect("错误回执必须带 message");
    assert!(
        message
            .chars()
            .any(|value| ('\u{4e00}'..='\u{9fff}').contains(&value)),
        "错误文案必须给中文原因：{message}"
    );
}

#[test]
fn browse_lists_only_subdirectories_in_ascending_order() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    std::fs::create_dir_all(root.join("beta")).expect("测试目录必须可创建");
    std::fs::create_dir_all(root.join("alpha")).expect("测试目录必须可创建");
    std::fs::write(root.join("note.txt"), b"x").expect("测试文件必须可写");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        DIRS_PATH,
        Some(&query(&[("path", root_text)])),
    );
    assert_eq!(status, 200);
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["dirs", "parent", "path", "truncated"],
        "回执字段必须与合同一致"
    );
    assert_eq!(
        payload["path"],
        json!(root_text),
        "必须回归一化后的绝对路径"
    );
    assert_eq!(payload["truncated"], json!(false));

    let dirs = payload["dirs"].as_array().expect("dirs 必须是数组");
    let names = dirs
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["alpha", "beta"], "只列子目录，按名称升序");
    let mut entry_keys = dirs[0]
        .as_object()
        .expect("目录项必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    entry_keys.sort_unstable();
    assert_eq!(entry_keys, vec!["name", "path"]);
    assert!(
        dirs[0]["path"]
            .as_str()
            .expect("目录项 path 必须是字符串")
            .ends_with("alpha"),
        "目录项必须给出绝对路径"
    );
}

#[test]
fn browse_reports_the_parent_and_returns_null_at_the_filesystem_root() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).expect("测试目录必须可创建");
    let nested_text = nested.to_str().expect("临时目录必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        DIRS_PATH,
        Some(&query(&[("path", nested_text)])),
    );
    assert_eq!(status, 200);
    assert_eq!(
        payload["parent"],
        json!(root.to_str().expect("临时目录必须是合法 UTF-8")),
        "父目录必须是绝对路径"
    );

    // 盘根没有父目录：必须回 null，而不是回自己。
    let filesystem_root = root
        .ancestors()
        .last()
        .expect("临时目录必然有盘根祖先")
        .to_str()
        .expect("盘根必须是合法 UTF-8")
        .to_owned();
    let (status, payload) = call(
        &adapter,
        "GET",
        DIRS_PATH,
        Some(&query(&[("path", filesystem_root.as_str())])),
    );
    assert_eq!(status, 200);
    assert_eq!(payload["parent"], json!(null));
}

#[test]
fn browse_stops_at_five_hundred_subdirectories_and_flags_truncation() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    for index in 0..501 {
        std::fs::create_dir_all(root.join(format!("dir-{index:04}"))).expect("测试目录必须可创建");
    }
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        DIRS_PATH,
        Some(&query(&[("path", root_text)])),
    );
    assert_eq!(status, 200);
    let dirs = payload["dirs"].as_array().expect("dirs 必须是数组");
    assert_eq!(dirs.len(), 500, "单层最多 500 条");
    assert_eq!(payload["truncated"], json!(true), "被截断必须如实标注");
}

#[test]
fn browse_rejects_the_attachments_directory() {
    let (directory, adapter) = fixture();
    let attachments = directory.path().join("data").join("attachments");
    std::fs::create_dir_all(attachments.join("uploaded")).expect("测试目录必须可创建");
    let attachments_text = attachments.to_str().expect("临时目录必须是合法 UTF-8");

    for path in [
        attachments_text.to_owned(),
        attachments
            .join("uploaded")
            .to_str()
            .map(str::to_owned)
            .expect("临时目录必须是合法 UTF-8"),
    ] {
        let (status, payload) = call(
            &adapter,
            "GET",
            DIRS_PATH,
            Some(&query(&[("path", path.as_str())])),
        );
        assert_eq!(status, 403, "{path}");
        assert_chinese(&payload);
    }
}

#[test]
fn browse_rejects_illegal_forms_and_absent_directories() {
    let (directory, adapter) = fixture();
    let root = directory.path();

    // 形态非法：相对路径是 400，设备命名空间与长路径前缀是 403。
    let (status, payload) = call(
        &adapter,
        "GET",
        DIRS_PATH,
        Some(&query(&[("path", "relative/dir")])),
    );
    assert_eq!(status, 400);
    assert_chinese(&payload);
    for path in [r"\\.\PhysicalDrive0", r"\\?\C:\Windows", r"\\server\share"] {
        let (status, payload) = call(&adapter, "GET", DIRS_PATH, Some(&query(&[("path", path)])));
        assert_eq!(status, 403, "{path}");
        assert_chinese(&payload);
    }

    // 绝对路径但不存在，或指向文件：都是 404。
    let absent = root.join("absent");
    let (status, payload) = call(
        &adapter,
        "GET",
        DIRS_PATH,
        Some(&query(&[(
            "path",
            absent.to_str().expect("临时路径必须是合法 UTF-8"),
        )])),
    );
    assert_eq!(status, 404);
    assert_chinese(&payload);
    std::fs::write(root.join("note.txt"), b"x").expect("测试文件必须可写");
    let note = root.join("note.txt");
    let (status, payload) = call(
        &adapter,
        "GET",
        DIRS_PATH,
        Some(&query(&[(
            "path",
            note.to_str().expect("临时路径必须是合法 UTF-8"),
        )])),
    );
    assert_eq!(status, 404);
    assert_chinese(&payload);
}

#[test]
fn browse_requires_the_path_parameter() {
    let (_directory, adapter) = fixture();
    for query in [None, Some("path="), Some("other=x")] {
        let (status, payload) = call(&adapter, "GET", DIRS_PATH, query);
        assert_eq!(status, 400, "{query:?}");
        assert_chinese(&payload);
    }
}

#[test]
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/fs",
        "/api/v1/fs/dirs/",
        "/api/v1/files",
        "/api/v1/attachments",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于目录浏览族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405() {
    let (directory, adapter) = fixture();
    let root_text = directory
        .path()
        .to_str()
        .expect("临时目录必须是合法 UTF-8")
        .to_owned();
    for method in ["POST", "PUT", "DELETE"] {
        let (status, payload) = call(
            &adapter,
            method,
            DIRS_PATH,
            Some(&query(&[("path", root_text.as_str())])),
        );
        assert_eq!(status, 405, "{method}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}
