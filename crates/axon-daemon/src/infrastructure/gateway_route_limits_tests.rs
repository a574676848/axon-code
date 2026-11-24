//! 每路由请求体上限表的契约测试。
//!
//! 这张表决定 `/api/v1/*` 收多少正文，错一行就等于给某条路由放宽或收紧额度，
//! 因此逐条核对：表内取值、表外回落、一维参数、方法与大小写精确性、准入判定。

use super::*;

/// 1 MiB 的字节数，用于把断言写成可读的等式。
const MIB: usize = 1024 * 1024;
/// 附件路由的请求体上限：64 MiB 文件本体加 8 KiB 的 multipart 固定开销。
const ATTACHMENT_TOTAL: usize = 64 * MIB + 8 * 1024;

#[test]
fn declared_routes_report_their_own_limits() {
    assert_eq!(body_limit("PUT", "/api/v1/config"), 4 * MIB);
    assert_eq!(body_limit("PUT", "/api/v1/secrets"), 64 * 1024);
    assert_eq!(body_limit("POST", "/api/v1/attachments"), ATTACHMENT_TOTAL);
    assert_eq!(body_limit("GET", "/api/v1/meta"), 0);
    // 管理面：签发的请求体只留 4 KiB，列表与吊销不声明正文。
    assert_eq!(body_limit("POST", "/api/v1/keys"), 4 * 1024);
    assert_eq!(body_limit("GET", "/api/v1/keys"), 0);
    assert_eq!(body_limit("DELETE", "/api/v1/keys/abc123"), 0);
    // 技能移除：标识走路径段，请求体不承载任何内容，因此同样不声明正文。
    assert_eq!(body_limit("DELETE", "/api/v1/skills/alpha-skill"), 0);
    assert_eq!(body_limit("GET", "/api/v1/skills"), 0);
}

#[test]
fn file_and_directory_routes_declare_no_body() {
    // 文件与目录三条路由都是 GET，不声明正文：带上正文即按超限拒绝。
    for path in ["/api/v1/files", "/api/v1/files/content", "/api/v1/fs/dirs"] {
        assert_eq!(body_limit("GET", path), 0, "{path} 不得声明正文");
        assert_eq!(
            admit_body(body_limit("GET", path), Some(1), false),
            BodyAdmission::TooLarge,
            "{path} 带正文必须拒绝"
        );
        assert_eq!(
            admit_body(body_limit("GET", path), None, false),
            BodyAdmission::Allowed(0)
        );
    }
    // 非 GET 方法不命中登记，按表外回落，证明这三行确实只登记了 GET。
    for method in ["POST", "PUT"] {
        assert_eq!(
            body_limit(method, "/api/v1/files"),
            FALLBACK_BODY_LIMIT,
            "{method} 不得命中文件路由"
        );
    }
}

#[test]
fn attachment_route_keeps_the_file_limit_plus_overhead() {
    // 文件本体 64 MiB 加 8 KiB 的 multipart 固定开销：文件超限只能在读体过程中判，
    // 请求体上限只用来提前挡住声明超限的对端。
    assert_eq!(ATTACHMENT_TOTAL, 64 * MIB + 8 * 1024);
    assert_eq!(
        admit_body(ATTACHMENT_TOTAL, Some(ATTACHMENT_TOTAL + 1), false),
        BodyAdmission::TooLarge
    );
    // 声明刚好等于文件本体上限：请求体带上 multipart 开销后仍在门槛之内，前判定不触发。
    assert_eq!(
        admit_body(ATTACHMENT_TOTAL, Some(64 * MIB + 512), false),
        BodyAdmission::Allowed(ATTACHMENT_TOTAL)
    );
    // 文件本体 + 1 字节：请求体仍低于加 8 KiB 的门槛，因此判定权在读体过程。
    assert_eq!(
        admit_body(ATTACHMENT_TOTAL, Some(64 * MIB + 1 + 512), false),
        BodyAdmission::Allowed(ATTACHMENT_TOTAL)
    );
}

#[test]
fn unlisted_paths_fall_back_to_four_mebibytes() {
    assert_eq!(body_limit("GET", "/api/v1/whatever"), 4 * MIB);
    assert_eq!(
        body_limit("POST", "/api/v1/whatever"),
        FALLBACK_BODY_LIMIT,
        "表外路径必须拿到默认额度，不得回落到 router 级取值"
    );
}

#[test]
fn single_parameter_segment_matches_exactly_one_segment() {
    assert_eq!(body_limit("DELETE", "/api/v1/keys/abc123"), 0);
    assert_eq!(
        body_limit("DELETE", "/api/v1/keys/abc/def"),
        FALLBACK_BODY_LIMIT
    );
    assert_eq!(
        body_limit("DELETE", "/api/v1/keys/"),
        FALLBACK_BODY_LIMIT,
        "参数段不可为空，尾随斜杠不命中吊销路由"
    );
    assert_eq!(body_limit("DELETE", "/api/v1/skills/alpha-skill"), 0);
    assert_eq!(
        body_limit("DELETE", "/api/v1/skills/alpha-skill/extra"),
        FALLBACK_BODY_LIMIT,
        "技能标识同样只占一段路径"
    );
    assert_eq!(
        body_limit("DELETE", "/api/v1/skills/"),
        FALLBACK_BODY_LIMIT,
        "尾随斜杠不命中技能移除路由"
    );
}

#[test]
fn method_match_is_exact_and_case_sensitive() {
    // 小写方法不是合法方法：不命中表，按表外回落。
    assert_eq!(body_limit("put", "/api/v1/config"), FALLBACK_BODY_LIMIT);
    // 精确命中必须与回落值区分开，否则本条断言证明不了任何事。
    assert_eq!(body_limit("put", "/api/v1/secrets"), FALLBACK_BODY_LIMIT);
    assert_eq!(body_limit("PUT", "/api/v1/secrets"), 64 * 1024);
    assert_eq!(
        body_limit("PUT", "/api/v1/attachments"),
        FALLBACK_BODY_LIMIT
    );
    assert_eq!(body_limit("POST", "/api/v1/attachments"), ATTACHMENT_TOTAL);
    // GET 与 PUT 各自登记：同一路径下方法不同，取值也随之不同。
    assert_eq!(body_limit("GET", "/api/v1/config"), 0);
    assert_eq!(body_limit("POST", "/api/v1/config"), FALLBACK_BODY_LIMIT);
}

#[test]
fn admission_rejects_oversized_declaration_before_reading_body() {
    assert_eq!(
        admit_body(64 * 1024, Some(64 * 1024 + 1), false),
        BodyAdmission::TooLarge
    );
    assert_eq!(
        admit_body(64 * 1024, Some(64 * 1024), false),
        BodyAdmission::Allowed(64 * 1024)
    );
    // 不声明正文的路由带上正文：与其它超限同一错误码语义，不是 400。
    assert_eq!(admit_body(0, Some(1), false), BodyAdmission::TooLarge);
}

#[test]
fn admission_treats_chunked_as_missing_length_and_absent_as_no_body() {
    assert_eq!(
        admit_body(FALLBACK_BODY_LIMIT, None, true),
        BodyAdmission::LengthRequired
    );
    // 无长度且非分块：视为无正文，GET、HEAD、DELETE 因此不受影响。
    assert_eq!(admit_body(0, None, false), BodyAdmission::Allowed(0));
    assert_eq!(
        admit_body(4 * MIB, None, false),
        BodyAdmission::Allowed(4 * MIB)
    );
    // 声明长度 0 与声明长度缺省等价，都不报错。
    assert_eq!(admit_body(0, Some(0), false), BodyAdmission::Allowed(0));
}
