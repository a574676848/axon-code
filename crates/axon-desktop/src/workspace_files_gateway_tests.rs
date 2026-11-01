//! [`crate::workspace_files_gateway`] 的用例。
//!
//! 单独成篇只为守住单文件 700 行硬限。这里断言的都是**纯函数**：查询串编码、状态码
//! 文案、段头文件名、预览路径映射与流式分段读取，不需要起网关，也不碰真实档案。

use std::io::Read as _;

use super::{
    MultipartReader, UploadBody, failure_message, header_safe_name, percent_encode, preview_target,
    query_string,
};

#[test]
fn query_values_are_percent_encoded_byte_by_byte() {
    // 空格编成 %20；`+` 必须编码，否则服务端会把它读成空格。
    assert_eq!(percent_encode("a b"), "a%20b");
    assert_eq!(percent_encode("工作区"), "%E5%B7%A5%E4%BD%9C%E5%8C%BA");
    assert_eq!(percent_encode("a#b"), "a%23b");
    assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
    assert_eq!(percent_encode("a+b"), "a%2Bb");
    // 路径分隔符与盘符照编：解码回原值是服务端的责任，收窄字符集只会多一份事实。
    assert_eq!(percent_encode(r"C:\work\a.md"), "C%3A%5Cwork%5Ca.md");
    assert_eq!(percent_encode("C:/work/a.md"), "C%3A%2Fwork%2Fa.md");
    assert_eq!(percent_encode("aZ0-_.~"), "aZ0-_.~");
}

#[test]
fn query_string_joins_pairs_with_encoded_keys_and_values() {
    let query = query_string(&[("root", "C:/work 区"), ("path", "a#b&c.md")])
        .unwrap_or_else(|| "缺少查询串".to_string());
    assert_eq!(query, "root=C%3A%2Fwork%20%E5%8C%BA&path=a%23b%26c.md");
    assert!(query_string(&[]).is_none(), "空参数表不得产出查询串");
}

#[test]
fn a_missing_route_is_reported_as_an_outdated_backend_not_a_missing_file() {
    // 老后端：网关的嵌套错误模型。
    let outdated = failure_message(
        404,
        r#"{"error":{"code":"NOT_FOUND","message":"未找到指定的接口资源","details":null}}"#,
    );
    assert_eq!(outdated, "后台服务版本过旧，该能力需要升级后台服务");

    // 文件路由自己的结论：扁平错误模型，带中文原因。
    let missing = failure_message(404, r#"{"error":"not_found","message":"目标不是文件"}"#);
    assert!(missing.contains("目标不存在"), "{missing}");
    assert!(
        !missing.contains("版本过旧"),
        "目标不存在不是版本问题：{missing}"
    );
    assert_ne!(outdated, missing, "两种 404 必须给出不同结论");
}

#[test]
fn rejected_credentials_are_reported_as_disconnected() {
    let rejected = failure_message(
        401,
        r#"{"error":{"code":"UNAUTHORIZED","message":"未授权访问","details":null}}"#,
    );
    assert!(rejected.starts_with("未连接"), "{rejected}");
    assert!(rejected.contains("凭据"), "{rejected}");
    assert_ne!(
        rejected,
        failure_message(404, ""),
        "凭据问题与版本过旧必须是两句话"
    );
}

#[test]
fn oversized_reads_keep_the_backend_reason() {
    let message = failure_message(
        413,
        r#"{"error":"too_large","message":"文件超过上限","limit":2097152}"#,
    );
    assert!(message.contains("上限"), "{message}");
    assert!(message.contains("文件超过上限"), "{message}");
}

#[test]
fn multipart_file_names_stay_inside_the_header_boundary() {
    assert_eq!(header_safe_name("a;b=c\"d.png"), "a_b_c_d.png");
    assert_eq!(header_safe_name("实时语音（1）.md"), "实时语音（1）.md");
    assert_eq!(header_safe_name("C:\\tmp\\shot.png"), "shot.png");
    assert_eq!(header_safe_name("   "), "attachment");
    assert!(!header_safe_name("a\r\nb.png").contains(['\r', '\n']));
}

#[test]
fn preview_paths_that_cannot_be_addressed_are_rejected() {
    assert_eq!(
        preview_target(r"C:\srv\att\a.png").expect("Windows 绝对路径必须可映射"),
        ("C:/srv/att".to_string(), "a.png".to_string())
    );
    assert_eq!(
        preview_target("/srv/att/a.png").expect("POSIX 绝对路径必须可映射"),
        ("/srv/att".to_string(), "a.png".to_string())
    );
    assert_eq!(
        preview_target("C:/a.png").expect("盘根必须补齐分隔符"),
        ("C:/".to_string(), "a.png".to_string())
    );
    // 越界与映射不出去的形态一律拒绝：云档案下没有任何本机回退路径。
    for rejected in [
        "a.png",
        "",
        "/srv/att/",
        "/srv/att/..",
        "../../etc/passwd.png",
        "C:/srv/../a.png",
    ] {
        assert!(preview_target(rejected).is_err(), "{rejected} 必须被拒绝");
    }
}

#[test]
fn the_multipart_reader_yields_head_payload_and_tail_in_order() {
    let payload = b"payload-bytes";
    let mut reader = MultipartReader::new(
        b"head:".to_vec(),
        UploadBody::Memory(payload),
        b":tail".to_vec(),
    );
    let mut assembled = Vec::new();
    // 用很小的缓冲逐段读，确保分段之间不会漏字节。
    let mut buffer = [0_u8; 3];
    loop {
        let read = reader.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break;
        }
        assembled.extend_from_slice(&buffer[..read]);
    }
    assert_eq!(
        assembled,
        b"head:payload-bytes:tail".to_vec(),
        "三段必须按段头、本体、收尾的顺序原样输出"
    );
}
