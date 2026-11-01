//! 附件上传路由的契约测试。
//!
//! 覆盖回执形状、64 MiB 文件本体上限（成功与超限两种）、超限不落盘、文件名收敛、
//! multipart 形态校验、未命中回落与方法边界。

use std::path::PathBuf;

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{ATTACHMENTS_PATH, AttachmentsHttpAdapter, MAX_FILE_BYTES};

/// 测试用 multipart 分隔符。
const BOUNDARY: &str = "axon-test-boundary";

/// 以临时目录为运行时根构造适配器。
fn fixture() -> (
    axon_test_support::TestTempDir,
    AttachmentsHttpAdapter,
    PathBuf,
) {
    let directory = axon_test_support::test_temp_dir();
    let attachments = directory.path().join("data").join("attachments");
    let adapter = AttachmentsHttpAdapter::new(directory.path().to_path_buf());
    (directory, adapter, attachments)
}

/// 组装一段只有 `file` 字段的 multipart 报文。
fn multipart(file_name: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(content.len() + 256);
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

/// 发一次上传请求并按 JSON 解析回执。
fn upload(adapter: &AttachmentsHttpAdapter, body: &[u8]) -> (u16, Value) {
    let response = adapter
        .handle_request("POST", ATTACHMENTS_PATH, None, body)
        .expect("附件路由必须命中");
    let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, payload)
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

/// 统计附件目录下残留的条目数；目录不存在按 0 计。
fn leftovers(attachments: &std::path::Path) -> usize {
    std::fs::read_dir(attachments).map_or(0, Iterator::count)
}

#[test]
fn upload_lands_the_file_and_reports_the_contract_shape() {
    let (_directory, adapter, attachments) = fixture();
    let content = b"hello attachment";
    let (status, payload) = upload(&adapter, &multipart("a.txt", content));
    assert_eq!(status, 200);

    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["bytes", "image", "name", "path"]);
    assert_eq!(payload["name"], json!("a.txt"), "展示名保留用户原始写法");
    assert_eq!(payload["bytes"], json!(content.len()));
    assert_eq!(payload["image"], json!(false));

    let landed = PathBuf::from(payload["path"].as_str().expect("path 必须是字符串"));
    assert!(landed.is_file(), "附件必须落盘：{}", landed.display());
    assert!(
        !landed.to_string_lossy().starts_with(r"\\?\"),
        "对外路径不得带长路径前缀"
    );
    assert!(
        landed.starts_with(&attachments),
        "附件必须落在运行时根的 data/attachments 之下"
    );
    assert_eq!(std::fs::read(&landed).expect("附件必须可读"), content);
}

#[test]
fn upload_flags_images_by_extension() {
    let (_directory, adapter, _attachments) = fixture();
    let (status, payload) = upload(&adapter, &multipart("photo.png", b"png"));
    assert_eq!(status, 200);
    assert_eq!(payload["image"], json!(true));
}

#[test]
fn upload_converges_the_disk_name_but_keeps_the_display_name() {
    let (_directory, adapter, _attachments) = fixture();
    let (status, payload) = upload(&adapter, &multipart("../../etc/passwd", b"x"));
    assert_eq!(status, 200);
    assert_eq!(
        payload["name"],
        json!("../../etc/passwd"),
        "展示名保留用户原始写法"
    );
    let landed = PathBuf::from(payload["path"].as_str().expect("path 必须是字符串"));
    assert_eq!(
        landed.file_name().and_then(|value| value.to_str()),
        Some("passwd"),
        "磁盘名必须收敛成单段安全名"
    );
}

#[test]
fn upload_accepts_exactly_sixteen_mebibytes() {
    let (_directory, adapter, _attachments) = fixture();
    let content = vec![b'a'; MAX_FILE_BYTES as usize];
    let (status, payload) = upload(&adapter, &multipart("edge.bin", &content));
    assert_eq!(status, 200, "恰好 64 MiB 必须成功");
    assert_eq!(payload["bytes"], json!(MAX_FILE_BYTES));
}

#[test]
fn upload_rejects_sixteen_mebibytes_plus_one_without_leaving_anything_behind() {
    let (_directory, adapter, attachments) = fixture();
    let content = vec![b'a'; MAX_FILE_BYTES as usize + 1];
    let (status, payload) = upload(&adapter, &multipart("over.bin", &content));
    assert_eq!(status, 413, "64 MiB 加 1 字节必须在读体过程中 413");
    assert_chinese(&payload);
    assert_eq!(payload["limit"], json!(MAX_FILE_BYTES), "回执必须说明上限");
    assert_eq!(leftovers(&attachments), 0, "超限上传不得留下任何文件或目录");
}

#[test]
fn upload_rejects_a_body_that_is_not_multipart() {
    let (_directory, adapter, attachments) = fixture();
    for body in [
        &b"not multipart at all"[..],
        &b"{\"name\":\"a.txt\"}"[..],
        &[][..],
    ] {
        let (status, payload) = upload(&adapter, body);
        assert_eq!(status, 400, "非 multipart 正文必须 400");
        assert_eq!(payload["error"], json!("unsupported_content_type"));
        assert_chinese(&payload);
    }
    assert_eq!(leftovers(&attachments), 0, "拒绝的请求绝不能落盘");
}

#[test]
fn upload_reports_a_missing_file_field() {
    let (_directory, adapter, attachments) = fixture();
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\nvalue\r\n--{BOUNDARY}--\r\n"
    )
    .into_bytes();
    let (status, payload) = upload(&adapter, &body);
    assert_eq!(status, 400);
    assert_eq!(payload["error"], json!("missing_file_field"));
    assert_chinese(&payload);
    assert_eq!(leftovers(&attachments), 0);
}

#[test]
fn upload_reports_a_file_part_without_a_filename() {
    let (_directory, adapter, _attachments) = fixture();
    let body =
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\nx\r\n--{BOUNDARY}--\r\n")
            .into_bytes();
    let (status, payload) = upload(&adapter, &body);
    assert_eq!(status, 400);
    assert_eq!(payload["error"], json!("invalid_multipart"));
    assert_chinese(&payload);
}

#[test]
fn upload_accepts_an_empty_file() {
    let (_directory, adapter, _attachments) = fixture();
    let (status, payload) = upload(&adapter, &multipart("empty.txt", &[]));
    assert_eq!(status, 200);
    assert_eq!(payload["bytes"], json!(0));
}

#[test]
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter, _attachments) = fixture();
    for path in [
        "/api/v1/attachments/",
        "/api/v1/files",
        "/api/v1/fs/dirs",
        "/api/v1/attachment",
    ] {
        assert!(
            adapter.handle_request("POST", path, None, &[]).is_none(),
            "{path} 不属于附件族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405() {
    let (_directory, adapter, _attachments) = fixture();
    for method in ["GET", "PUT", "DELETE", "PATCH"] {
        let response = adapter
            .handle_request(method, ATTACHMENTS_PATH, None, &[])
            .expect("附件路由必须命中");
        assert_eq!(response.status, 405, "{method}");
        let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}
