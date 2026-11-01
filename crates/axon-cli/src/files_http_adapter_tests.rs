//! 文件列表与文件内容路由的契约测试。
//!
//! 覆盖回执形状、单层上限与 `truncated`、文本与二进制两条读取上限、越界拒绝、
//! 附件目录不可浏览、未命中回落与方法边界。

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{CONTENT_PATH, FilesHttpAdapter, LIST_PATH};

/// 组装查询串，逐项做百分号编码，模拟真实客户端。
fn query(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

/// 发一次请求；未命中时必须返回 `None`。
fn call(adapter: &FilesHttpAdapter, method: &str, path: &str, query: Option<&str>) -> (u16, Value) {
    let response = adapter
        .handle_request(method, path, query, &[])
        .expect("文件路由必须命中");
    let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, payload)
}

/// 构造一个以临时目录为运行时根的适配器，并在其中建一个工作区根。
fn fixture() -> (axon_test_support::TestTempDir, FilesHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = FilesHttpAdapter::new(directory.path().to_path_buf());
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
fn listing_reports_the_contract_shape_in_ascending_order() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    std::fs::create_dir_all(root.join("src")).expect("测试目录必须可创建");
    std::fs::write(root.join("b.txt"), b"bb").expect("测试文件必须可写");
    std::fs::write(root.join("a.txt"), b"a").expect("测试文件必须可写");

    let (status, payload) = call(
        &adapter,
        "GET",
        LIST_PATH,
        Some(&query(&[(
            "root",
            root.to_str().expect("临时目录必须是合法 UTF-8"),
        )])),
    );
    assert_eq!(status, 200);
    assert_eq!(
        payload["truncated"],
        json!(false),
        "未超上限时必须如实报 false"
    );

    let entries = payload["entries"].as_array().expect("entries 必须是数组");
    let names = entries
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["a.txt", "b.txt", "src"], "必须按名称升序");

    let mut keys = entries[0]
        .as_object()
        .expect("目录项必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["isDir", "modifiedMs", "name", "path", "size"],
        "目录项字段必须与合同一致"
    );
    assert_eq!(entries[0]["isDir"], json!(false));
    assert_eq!(entries[0]["size"], json!(1));
    assert_eq!(entries[0]["path"], json!("a.txt"), "路径相对工作区根");
    assert_eq!(entries[2]["isDir"], json!(true));
    assert_eq!(entries[2]["size"], json!(0), "目录不带 size");
    assert!(
        entries[0]["modifiedMs"].is_u64(),
        "修改时间必须给真实毫秒值"
    );
}

#[test]
fn listing_a_subdirectory_keeps_paths_relative_to_the_root() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    std::fs::create_dir_all(root.join("src").join("nested")).expect("测试目录必须可创建");
    std::fs::write(root.join("src").join("a.txt"), b"a").expect("测试文件必须可写");

    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");
    let (status, payload) = call(
        &adapter,
        "GET",
        LIST_PATH,
        Some(&query(&[("root", root_text), ("path", "src")])),
    );
    assert_eq!(status, 200);
    let entries = payload["entries"].as_array().expect("entries 必须是数组");
    assert_eq!(entries.len(), 2, "一次只列一层，不得递归");
    assert_eq!(entries[0]["path"], json!("src/a.txt"));
    assert_eq!(entries[1]["path"], json!("src/nested"));
}

#[test]
fn listing_stops_at_five_hundred_entries_and_flags_truncation() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    for index in 0..501 {
        std::fs::write(root.join(format!("file-{index:04}.txt")), b"x").expect("测试文件必须可写");
    }
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");
    let (status, payload) = call(
        &adapter,
        "GET",
        LIST_PATH,
        Some(&query(&[("root", root_text)])),
    );
    assert_eq!(status, 200);
    let entries = payload["entries"].as_array().expect("entries 必须是数组");
    assert_eq!(entries.len(), 500, "单层最多 500 条");
    assert_eq!(payload["truncated"], json!(true), "被截断必须如实标注");
}

#[test]
fn listing_reports_absent_and_non_directory_targets_as_404() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    std::fs::write(root.join("a.txt"), b"a").expect("测试文件必须可写");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    for relative in ["absent", "a.txt"] {
        let (status, payload) = call(
            &adapter,
            "GET",
            LIST_PATH,
            Some(&query(&[("root", root_text), ("path", relative)])),
        );
        assert_eq!(status, 404, "{relative}");
        assert_chinese(&payload);
    }
}

#[test]
fn listing_rejects_escapes_with_403_and_a_chinese_reason() {
    let (directory, adapter) = fixture();
    let root = directory.path().join("root");
    std::fs::create_dir_all(&root).expect("测试目录必须可创建");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&outside).expect("测试目录必须可创建");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    for relative in [
        "..",
        "../outside",
        "a/../../outside",
        "file.txt::$DATA",
        r"\\server\share",
        r"\\?\C:\Windows",
    ] {
        let (status, payload) = call(
            &adapter,
            "GET",
            LIST_PATH,
            Some(&query(&[("root", root_text), ("path", relative)])),
        );
        assert_eq!(status, 403, "{relative} 必须被拒绝");
        assert_chinese(&payload);
    }

    // 绝对路径跳出同样 403。
    let (status, payload) = call(
        &adapter,
        "GET",
        LIST_PATH,
        Some(&query(&[
            ("root", root_text),
            ("path", outside.to_str().expect("临时目录必须是合法 UTF-8")),
        ])),
    );
    assert_eq!(status, 403);
    assert_chinese(&payload);
}

#[test]
fn listing_never_treats_the_attachments_directory_as_a_workspace() {
    let (directory, adapter) = fixture();
    let attachments = directory.path().join("data").join("attachments");
    std::fs::create_dir_all(attachments.join("uploaded")).expect("测试目录必须可创建");
    std::fs::write(attachments.join("secret.png"), b"png").expect("测试文件必须可写");
    let root_text = directory.path().to_str().expect("临时目录必须是合法 UTF-8");

    for relative in ["data/attachments", "data/attachments/uploaded"] {
        let (status, payload) = call(
            &adapter,
            "GET",
            LIST_PATH,
            Some(&query(&[("root", root_text), ("path", relative)])),
        );
        assert_eq!(status, 403, "{relative}");
        assert_chinese(&payload);
    }
    // 附件目录的父目录本身不是附件目录，仍可正常浏览。
    let (status, _payload) = call(
        &adapter,
        "GET",
        LIST_PATH,
        Some(&query(&[("root", root_text), ("path", "data")])),
    );
    assert_eq!(status, 200);
}

#[test]
fn content_reads_text_and_reports_the_byte_count() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    std::fs::write(root.join("a.txt"), "中文正文\n".as_bytes()).expect("测试文件必须可写");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        CONTENT_PATH,
        Some(&query(&[("root", root_text), ("path", "a.txt")])),
    );
    assert_eq!(status, 200);
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["bytes", "text", "truncated"]);
    assert_eq!(payload["text"], json!("中文正文\n"));
    assert_eq!(payload["bytes"], json!("中文正文\n".len()));
    assert_eq!(payload["truncated"], json!(false));
}

#[test]
fn content_reads_binary_as_base64_with_a_mime_type() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    let bytes = [0u8, 1, 2, 3, 255];
    std::fs::write(root.join("a.png"), bytes).expect("测试文件必须可写");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        CONTENT_PATH,
        Some(&query(&[
            ("root", root_text),
            ("path", "a.png"),
            ("encoding", "base64"),
        ])),
    );
    assert_eq!(status, 200);
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["base64", "bytes", "mime"]);
    assert_eq!(payload["mime"], json!("image/png"));
    assert_eq!(payload["bytes"], json!(5));
    assert_eq!(payload["base64"], json!("AAECA/8="));
}

#[test]
fn content_enforces_the_text_and_binary_read_limits() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    let text_limit = 2 * 1024 * 1024;
    let binary_limit = 12 * 1024 * 1024;
    std::fs::write(root.join("exact.txt"), vec![b'a'; text_limit]).expect("测试文件必须可写");
    std::fs::write(root.join("over.txt"), vec![b'a'; text_limit + 1]).expect("测试文件必须可写");
    std::fs::write(root.join("over.bin"), vec![0u8; binary_limit + 1]).expect("测试文件必须可写");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    let read = |relative: &str, encoding: Option<&str>| {
        let mut pairs = vec![("root", root_text), ("path", relative)];
        if let Some(encoding) = encoding {
            pairs.push(("encoding", encoding));
        }
        call(&adapter, "GET", CONTENT_PATH, Some(&query(&pairs)))
    };

    // 恰好等于上限必须成功。
    let (status, payload) = read("exact.txt", None);
    assert_eq!(status, 200);
    assert_eq!(payload["bytes"], json!(text_limit));

    let (status, payload) = read("over.txt", None);
    assert_eq!(status, 413, "文本超过 2 MiB 必须 413");
    assert_chinese(&payload);
    assert_eq!(payload["limit"], json!(text_limit), "回执必须说明上限");

    let (status, payload) = read("over.bin", Some("base64"));
    assert_eq!(status, 413, "二进制超过 12 MiB 必须 413");
    assert_chinese(&payload);
    assert_eq!(payload["limit"], json!(binary_limit));
}

#[test]
fn content_rejects_an_unknown_encoding() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    std::fs::write(root.join("a.txt"), b"a").expect("测试文件必须可写");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        CONTENT_PATH,
        Some(&query(&[
            ("root", root_text),
            ("path", "a.txt"),
            ("encoding", "utf8"),
        ])),
    );
    assert_eq!(status, 400);
    assert_chinese(&payload);
}

#[test]
fn content_requires_both_root_and_path() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "GET", CONTENT_PATH, None);
    assert_eq!(status, 400);
    assert_chinese(&payload);
    let (status, _payload) = call(&adapter, "GET", LIST_PATH, Some("path=a.txt"));
    assert_eq!(status, 400);
}

#[test]
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/files/",
        "/api/v1/files/content/extra",
        "/api/v1/fs/dirs",
        "/api/v1/documents/soul",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于文件族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405() {
    let (_directory, adapter) = fixture();
    for (method, path) in [
        ("POST", LIST_PATH),
        ("PUT", LIST_PATH),
        ("DELETE", CONTENT_PATH),
        ("PATCH", CONTENT_PATH),
    ] {
        let (status, payload) = call(&adapter, method, path, None);
        assert_eq!(status, 405, "{method} {path}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}

/// 内容路由不拦附件路径：云端档案的附件预览本来就要向服务端取字节。
#[test]
fn content_still_serves_files_under_the_attachments_directory() {
    let (directory, adapter) = fixture();
    let attachments = directory.path().join("data").join("attachments");
    std::fs::create_dir_all(&attachments).expect("测试目录必须可创建");
    std::fs::write(attachments.join("secret.txt"), b"payload").expect("测试文件必须可写");
    let root_text = directory.path().to_str().expect("临时目录必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        CONTENT_PATH,
        Some(&query(&[
            ("root", root_text),
            ("path", "data/attachments/secret.txt"),
        ])),
    );
    assert_eq!(status, 200);
    assert_eq!(payload["text"], json!("payload"));
}

/// 相对路径与根内绝对路径必须命中同一个文件。
#[test]
fn content_accepts_an_absolute_path_inside_the_root() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    std::fs::write(root.join("a.txt"), b"a").expect("测试文件必须可写");
    let absolute = root.join("a.txt");
    let root_text = root.to_str().expect("临时目录必须是合法 UTF-8");
    let absolute_text = absolute.to_str().expect("临时路径必须是合法 UTF-8");

    let (status, payload) = call(
        &adapter,
        "GET",
        CONTENT_PATH,
        Some(&query(&[("root", root_text), ("path", absolute_text)])),
    );
    assert_eq!(status, 200);
    assert_eq!(payload["text"], json!("a"));
}
