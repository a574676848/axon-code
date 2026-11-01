//! 多 Key 存储与身份分级的契约测试。
//!
//! 覆盖七组必测口径：公开 PBKDF2 向量、明文只出现一次与文件权限、命中与吊销、
//! 热更新、身份分级、管理面查询串禁令、旧 token 通道关闭后的语义。
//!
//! 关于 PBKDF2 向量的出处：PBKDF2 的公开向量有两套。RFC 6070 那套的 PRF 是
//! HMAC-SHA1，本项目的摘要算法固定为 HMAC-SHA256，SHA1 又不在允许的依赖面内，
//! 因此 RFC 6070 无法证伪或证实本实现；这里改用同为 IETF 公开文档的 RFC 7914
//! 第 11 节 PBKDF2-HMAC-SHA256 向量，它覆盖了 RFC 6070 用来验同一套机制的
//! 全部要点：块编号（大端）、迭代链 U1..Uc、多块输出与不足 4 字节的尾块。

use super::super::{GatewayAuth, GatewayIdentity};
use super::*;
use crate::application::ExternalEventEnvelope;
use crate::infrastructure::event_scripts::EventProviderRegistry;
use crate::infrastructure::gateway_http::HttpGatewayAdapter;
use crate::infrastructure::gateway_http_handler::{GatewayHttpResponse, GatewayHttpRouteHandler};
use crate::infrastructure::gateway_kernel::{
    GatewayAdapter, GatewayEgress, GatewayEventStream, GatewayIngress, GatewayKernel,
    GatewayKernelError, GatewaySubscriptionRequest,
};
use axon_protocol::SubmitInteractionReceipt;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// 测试用全局 token。
const TEST_GLOBAL_TOKEN: &str = "gateway-global-token";

/// 十六进制转字节，测试里贴向量用；长度必须偶数。
fn hex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "十六进制文本长度必须为偶数");
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks(2) {
        let digit = |byte: u8| -> u8 {
            match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => panic!("非法十六进制字符"),
            }
        };
        out.push((digit(pair[0]) << 4) | digit(pair[1]));
    }
    out
}

/// 从明文里取出秘密部分：前缀与 `id` 之后、第一个下划线之后的全部内容。
fn plaintext_secret(plaintext: &str) -> &str {
    let (_, secret) = plaintext
        .strip_prefix(PLAINTEXT_PREFIX)
        .and_then(|rest| rest.split_once('_'))
        .expect("明文形如 前缀 + id + 下划线 + 秘密");
    secret
}

// ---------------------------------------------------------------------------
// 1. PBKDF2 公开向量（RFC 7914 第 11 节，PBKDF2-HMAC-SHA-256）
// ---------------------------------------------------------------------------

#[test]
fn pbkdf2_matches_published_rfc7914_sha256_vectors() {
    // P="passwd", S="salt", c=1, dkLen=64（两块输出）。
    let mut output = vec![0_u8; 64];
    pbkdf2_hmac_sha256(b"passwd", b"salt", 1, &mut output).expect("派生");
    assert_eq!(
        output,
        hex(
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
             49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
        )
    );

    // P="Password", S="NaCl", c=80000, dkLen=64。
    let mut output = vec![0_u8; 64];
    pbkdf2_hmac_sha256(b"Password", b"NaCl", 80_000, &mut output).expect("派生");
    assert_eq!(
        output,
        hex(
            "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56\
             a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d"
        )
    );

    // P="passwordPASSWORDpassword", S="saltSALT..."（长盐），c=4096, dkLen=40（尾块不足 32 字节）。
    let mut output = vec![0_u8; 40];
    pbkdf2_hmac_sha256(
        b"passwordPASSWORDpassword",
        b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
        4_096,
        &mut output,
    )
    .expect("派生");
    assert_eq!(
        output,
        hex("348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9")
    );

    // P="pass\0word", S="sa\0lt"：口令与盐里的 0 字节必须按字节处理。
    let mut output = vec![0_u8; 16];
    pbkdf2_hmac_sha256(b"pass\0word", b"sa\0lt", 4_096, &mut output).expect("派生");
    assert_eq!(output, hex("89b69d0516f829893c696226650a8687"));

    // 参数边界：迭代次数为 0 与空输出都必须拒绝，不得静默产出全零。
    assert_eq!(
        pbkdf2_hmac_sha256(b"passwd", b"salt", 0, &mut output),
        Err(GatewayKeyError::Derivation)
    );
    assert_eq!(
        pbkdf2_hmac_sha256(b"passwd", b"salt", 1, &mut []),
        Err(GatewayKeyError::Derivation)
    );
}

#[test]
fn base64_codec_round_trips_published_vectors() {
    // RFC 4648 第 10 节的示例。
    for (plain, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64_encode(plain.as_bytes()), encoded);
        assert_eq!(base64_decode(encoded), Some(plain.as_bytes().to_vec()));
    }
    // URL 安全字母表不带填充，且解码两种字母表都接受。
    assert_eq!(base64_encode_url(&[0xFB, 0xFF]), "-_8");
    assert_eq!(base64_decode("-_8"), Some(vec![0xFB, 0xFF]));
    assert_eq!(base64_decode("+/8"), Some(vec![0xFB, 0xFF]));
    assert_eq!(base64_decode("!!!!"), None);
}

// ---------------------------------------------------------------------------
// 2. 签发：明文只出现一次、落盘只有盐与摘要、文件权限
// ---------------------------------------------------------------------------

#[test]
fn create_returns_plaintext_once_and_stores_only_salt_and_hash() {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    let issued = store.create("部署机").expect("签发");

    assert!(issued.plaintext.starts_with(PLAINTEXT_PREFIX));
    assert!(issued.plaintext.contains(&issued.id), "明文里应带 id");

    let path = root.path().join(DEFAULT_KEY_STORE_RELATIVE_PATH);
    let text = std::fs::read_to_string(&path).expect("读取存储文件");
    assert!(text.contains(&issued.id), "落盘应有 id");
    assert!(text.contains("部署机"), "落盘应有 label");
    assert!(text.contains(KEY_HASH_ALGO), "落盘应登记摘要算法");
    assert!(text.contains("\"version\": 1"), "落盘应带版本：{text}");

    let secret = plaintext_secret(&issued.plaintext);
    assert!(!text.contains(secret), "落盘不得出现明文秘密部分");
    assert!(!text.contains(&issued.plaintext), "落盘不得出现完整明文");

    // 列表只回摘要之外的字段，指纹是 6 位十六进制。
    let listed = store.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, issued.id);
    assert_eq!(listed[0].label, issued.label);
    assert_eq!(listed[0].last_used_at_ms, None);
    assert_eq!(listed[0].fingerprint.len(), FINGERPRINT_CHARS);
    assert!(
        listed[0]
            .fingerprint
            .chars()
            .all(|ch| ch.is_ascii_hexdigit())
    );
    assert!(
        !text.contains(&listed[0].fingerprint),
        "指纹是再散结果，不等同于摘要前缀"
    );
}

#[cfg(unix)]
#[test]
fn create_sets_private_file_permissions_on_unix() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    store.create("权限").expect("签发");
    let path = root.path().join(DEFAULT_KEY_STORE_RELATIVE_PATH);
    let mode = std::fs::metadata(&path)
        .expect("存储文件元数据")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "存储文件权限必须为 0600，实际 {mode:o}");
}

#[test]
fn audit_log_records_actions_without_secrets() {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    let issued = store
        .create_as("审计", &KeyOrigin::http("superadmin", None))
        .expect("签发");
    store
        .revoke_as(&issued.id, &KeyOrigin::cli())
        .expect("吊销");

    let audit = root.path().join("data/logs/gateway-keys-audit.jsonl");
    let text = std::fs::read_to_string(audit).expect("读取审计日志");
    let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 2, "create 与 revoke 各一行：{text}");
    assert!(lines[0].contains("\"action\":\"create\""));
    assert!(lines[0].contains("\"source\":\"http\""));
    assert!(lines[1].contains("\"action\":\"revoke\""));
    assert!(lines[1].contains("\"source\":\"cli\""));
    let secret = plaintext_secret(&issued.plaintext);
    assert!(!text.contains(secret), "审计不得记录明文");
    assert!(!text.contains("pbkdf2"), "审计不得记录摘要");
}

// ---------------------------------------------------------------------------
// 3. 校验：命中、错误明文、吊销后
// ---------------------------------------------------------------------------

#[test]
fn verify_hits_with_identity_and_rejects_wrong_or_revoked() {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    let issued = store.create("命中").expect("签发");

    assert_eq!(
        store.verify(&issued.plaintext),
        Some(KeyIdentity {
            id: issued.id.clone(),
            label: issued.label.clone(),
        })
    );
    assert_eq!(store.verify("axon_key_不存在的 id_AAAA"), None);
    assert_eq!(store.verify("随便一串"), None);
    assert_eq!(store.verify(""), None);

    // 命中后 `last_used_at` 在内存里立即可见（落盘按间隔批量进行）。
    let listed = store.list();
    assert!(
        listed[0].last_used_at_ms.is_some(),
        "命中应更新最后使用时间"
    );

    // 同一把 Key 的明文改动任意一个字符都不命中。
    let mut tampered = issued.plaintext.clone();
    tampered.push('x');
    assert_eq!(store.verify(&tampered), None);

    store.revoke(&issued.id).expect("吊销");
    assert_eq!(store.verify(&issued.plaintext), None);
    assert!(store.list().is_empty(), "已吊销的 Key 不再出现在列表里");
    assert_eq!(
        store.revoke(&issued.id),
        Err(GatewayKeyError::AlreadyRevoked(issued.id.clone()))
    );
    assert_eq!(
        store.revoke("不存在"),
        Err(GatewayKeyError::UnknownKey("不存在".to_owned()))
    );
}

// ---------------------------------------------------------------------------
// 4. 热更新：吊销后同一进程内下一次鉴权立即生效
// ---------------------------------------------------------------------------

#[test]
fn revoke_takes_effect_on_the_next_request_without_restart() {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    let issued = store.create("热更新").expect("签发");
    let auth = GatewayAuth::from_token(TEST_GLOBAL_TOKEN)
        .expect("测试 token")
        .with_key_store(Arc::clone(&store));
    let header = format!("Bearer {}", issued.plaintext);
    assert!(
        auth.identify_request(Some(&header), None).is_some(),
        "吊销前应能通过"
    );

    store.revoke(&issued.id).expect("吊销");
    assert_eq!(
        auth.identify_request(Some(&header), None),
        None,
        "吊销后同一进程内下一次鉴权必须立即失效"
    );

    // 再签发一把同样立即生效：全程没有重新构造鉴权器，也没有重启进程。
    let fresh = store.create("新签发").expect("签发");
    let fresh_header = format!("Bearer {}", fresh.plaintext);
    assert_eq!(
        auth.identify_request(Some(&fresh_header), None),
        Some(GatewayIdentity::Key {
            id: fresh.id,
            label: fresh.label,
        })
    );
}

// ---------------------------------------------------------------------------
// 5. 身份分级
// ---------------------------------------------------------------------------

#[test]
fn identity_grades_global_token_key_and_nobody() {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    let issued = store.create("分级").expect("签发");
    let auth = GatewayAuth::from_token(TEST_GLOBAL_TOKEN)
        .expect("测试 token")
        .with_key_store(Arc::clone(&store));

    let global_header = format!("Bearer {TEST_GLOBAL_TOKEN}");
    let key_header = format!("Bearer {}", issued.plaintext);
    assert_eq!(
        auth.identify_header(Some(&global_header)),
        Some(GatewayIdentity::SuperAdmin)
    );
    assert_eq!(
        auth.identify_header(Some(&key_header)),
        Some(GatewayIdentity::Key {
            id: issued.id.clone(),
            label: issued.label.clone(),
        })
    );
    assert_eq!(auth.identify_header(Some("Bearer 错的")), None);
    assert_eq!(auth.identify_header(Some("Basic 错的")), None);
    assert_eq!(auth.identify_header(None), None);
    // 查询串通道同样返回身份，不是布尔值。
    assert_eq!(
        auth.identify_request(None, Some(&issued.plaintext)),
        Some(GatewayIdentity::Key {
            id: issued.id,
            label: issued.label,
        })
    );
    assert_eq!(auth.identify_request(None, Some("错的")), None);
    assert_eq!(auth.identify_request(None, None), None);
    // 既有布尔入口的语义不变：任一通道命中即通过。
    assert!(auth.authorize_request(Some(&global_header), None));
    assert!(auth.authorize_bearer(&issued.plaintext));
    assert!(!auth.authorize_request(Some("Bearer 错的"), Some("错的")));
}

// ---------------------------------------------------------------------------
// 6、7. 真实 HTTP 路由层：管理面查询串禁令与旧 token 关闭
// ---------------------------------------------------------------------------

/// 只回显身份的路由处理器；`handle_request` 走 trait 默认实现之外的显式覆盖。
struct IdentityEcho;

impl GatewayHttpRouteHandler for IdentityEcho {
    fn handle_request(
        &self,
        _method: &str,
        _path: &str,
        _query: Option<&str>,
        _body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        Some(GatewayHttpResponse::json(
            200,
            &serde_json::json!({ "identity": "fallback" }),
        ))
    }

    fn handle_request_as(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        _body: &[u8],
        identity: Option<&GatewayIdentity>,
        _peer: Option<&str>,
    ) -> Option<GatewayHttpResponse> {
        let identity = match identity {
            Some(GatewayIdentity::SuperAdmin) => serde_json::json!({ "kind": "superAdmin" }),
            Some(GatewayIdentity::Key { id, label }) => {
                serde_json::json!({ "kind": "key", "id": id, "label": label })
            }
            None => serde_json::json!({ "kind": "anonymous" }),
        };
        Some(GatewayHttpResponse::json(
            200,
            &serde_json::json!({ "method": method, "path": path, "identity": identity }),
        ))
    }
}

struct NoopIngress;

impl GatewayIngress for NoopIngress {
    fn submit_external_event(
        &self,
        _event: ExternalEventEnvelope,
    ) -> Result<SubmitInteractionReceipt, GatewayKernelError> {
        Err(GatewayKernelError::Ingress("测试不提交事件".to_owned()))
    }
}

struct NoopEgress;

impl GatewayEgress for NoopEgress {
    fn subscribe(
        &self,
        _request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        Err(GatewayKernelError::Ingress("测试不解订阅".to_owned()))
    }
}

/// 真实 socket 打真实服务线程的最小夹具。
struct TestServer {
    adapter: HttpGatewayAdapter,
    address: SocketAddr,
    _kernel: GatewayKernel,
}

impl TestServer {
    fn start(root: &Path, auth: GatewayAuth) -> Self {
        let kernel = GatewayKernel::new_with_auth(
            Arc::new(NoopIngress) as Arc<dyn GatewayIngress>,
            Arc::new(NoopEgress) as Arc<dyn GatewayEgress>,
            8,
            Some(Arc::new(auth)),
        );
        kernel.start().expect("启动测试内核");
        let context = kernel.context().expect("运行上下文");
        let adapter = HttpGatewayAdapter::new(
            "127.0.0.1:0",
            Arc::new(EventProviderRegistry::new(root.join("events"))),
        )
        .with_route_handler(Arc::new(IdentityEcho));
        adapter.start(context).expect("启动 HTTP 适配器");
        let address = adapter.local_addr().expect("已绑定监听地址");
        Self {
            adapter,
            address,
            _kernel: kernel,
        }
    }

    fn get(&self, target: &str, token: Option<&str>) -> (u16, String) {
        let mut raw = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
        if let Some(token) = token {
            raw.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        raw.push_str("\r\n");
        send_raw(self.address, &raw)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.adapter.stop();
    }
}

fn send_raw(address: SocketAddr, raw: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).expect("连接测试服务");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("设置读取超时");
    stream.write_all(raw.as_bytes()).expect("写入请求");
    let mut buffer = Vec::new();
    let _ = stream.read_to_end(&mut buffer);
    let text = String::from_utf8_lossy(&buffer).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map_or(String::new(), |(_, body)| body.to_owned());
    (status, body)
}

#[test]
fn management_surface_rejects_query_string_token_but_accepts_header() {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    let issued = store.create("管理面").expect("签发");
    let auth = GatewayAuth::from_token(TEST_GLOBAL_TOKEN)
        .expect("测试 token")
        .with_key_store(Arc::clone(&store));
    let server = TestServer::start(root.path(), auth);

    // 超管 token 走查询串：即使 token 正确也必须 401，不能先按查询串通过再在适配器里拒。
    let (status, body) = server.get(&format!("/api/v1/keys?api_token={TEST_GLOBAL_TOKEN}"), None);
    assert_eq!(status, 401, "管理面查询串必须 401：{body}");

    // 同一把 token 走 Authorization 头必须通过，并把身份透出给适配器。
    let (status, body) = server.get("/api/v1/keys", Some(TEST_GLOBAL_TOKEN));
    assert_eq!(status, 200, "超管走头部应通过：{body}");
    assert!(body.contains("superAdmin"), "适配器应拿到超管身份：{body}");

    // 普通 Key 调用管理面一律 403，而不是 401。
    let (status, body) = server.get("/api/v1/keys", Some(&issued.plaintext));
    assert_eq!(status, 403, "普通 Key 调用管理面必须 403：{body}");
    assert!(body.contains("FORBIDDEN"), "403 用统一错误模型：{body}");

    // DELETE 与带 id 的子路径同属管理面。
    let raw = format!(
        "DELETE /api/v1/keys/{}?api_token={TEST_GLOBAL_TOKEN} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        issued.id
    );
    assert_eq!(
        send_raw(server.address, &raw).0,
        401,
        "子路径同样禁止查询串"
    );

    // 百分号编码过的路径同样按管理面处理：编码一下不能绕过通道限制。
    let (status, body) = server.get(
        &format!("/api/v1/%6beys?api_token={TEST_GLOBAL_TOKEN}"),
        None,
    );
    assert_eq!(status, 401, "编码后的管理面路径同样禁止查询串：{body}");

    // 业务路由保留既有查询串通道：Jira 等发送方无法配置头部。
    let (status, body) = server.get(
        &format!("/api/v1/anything?api_token={TEST_GLOBAL_TOKEN}"),
        None,
    );
    assert_eq!(status, 200, "业务路由的查询串通道必须仍然可用：{body}");
    assert!(body.contains("superAdmin"), "{body}");
}

#[test]
fn legacy_token_disabled_blocks_global_token_but_keys_still_pass() {
    let root = tempfile::tempdir().expect("临时运行时根");
    let store = GatewayKeyStore::open(root.path(), "").expect("打开存储");
    let issued = store.create("关闭旧通道").expect("签发");
    let auth = GatewayAuth::from_token(TEST_GLOBAL_TOKEN)
        .expect("测试 token")
        .with_key_store(Arc::clone(&store))
        .with_legacy_token_enabled(false);
    let server = TestServer::start(root.path(), auth);

    let (status, body) = server.get("/api/v1/anything", Some(TEST_GLOBAL_TOKEN));
    assert_eq!(
        status, 401,
        "关闭旧通道后全局 token 在业务路由上不通过：{body}"
    );
    let (status, _body) = server.get(
        &format!("/api/v1/anything?api_token={TEST_GLOBAL_TOKEN}"),
        None,
    );
    assert_eq!(status, 401, "查询串通道同样不通过");

    let (status, body) = server.get("/api/v1/anything", Some(&issued.plaintext));
    assert_eq!(status, 200, "Key 仍必须通过：{body}");
    assert!(body.contains("\"kind\":\"key\""), "{body}");
}
