//! 系统只读 HTTP 适配器：协议元信息与服务端运行态快照。
//!
//! 两条路由都是只读的：
//!
//! - `GET /api/v1/meta`：协议版本、能力位、服务器时间与上传上限，供客户端在
//!   连接建立时一次性判断后端支持什么。
//! - `GET /api/v1/system/status`：租约、排队、远端适配器、构建版本、服务器时间
//!   与运行时长，供客户端展示后端运行态。
//!
//! 鉴权不在这里做：网关的 `custom()` 分派对 `/api/v1/*` 无条件执行鉴权，适配器
//! 拿不到豁免权，因此这两条路由同样是「需鉴权」面，不存在免鉴权入口。
//!
//! 取值纪律：只输出能读到的真实值。租约读不到时给 `null`，排队读不到时给 `0`，
//! 远端适配器没有可靠来源时给空数组，任何字段都不编造。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axon_config::ConfigLoader;
use axon_daemon::infrastructure::gateway_auth::GatewayIdentity;
use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_session::HealthStore;
use serde_json::{Map, Value, json};

use crate::workbench_http_adapter::runtime_store;

/// `/meta` 路由：协议版本、能力位、服务器时间与上传上限。
const META_PATH: &str = "/api/v1/meta";
/// `/system/status` 路由：服务端运行态只读快照。
const STATUS_PATH: &str = "/api/v1/system/status";
/// 协议版本：客户端据此协商，而不是拿构建版本号猜协议。
const PROTOCOL_VERSION: u32 = 1;
/// 单文件上传上限 64 MiB（67108864 字节）。
///
/// 它指**文件本身**的字节数，不是附件路由的请求体上限（后者还要为 multipart
/// 分隔符与头留固定开销）。
const UPLOAD_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
/// 能力位名字：唯一事实源是合同 §4.5，顺序即载荷顺序，**不得增删**。
const CAPABILITY_NAMES: [&str; 12] = [
    "files",
    "dirs",
    "attachments",
    "config",
    "secrets",
    "providers",
    "mcp",
    "skills",
    "documents",
    "system_status",
    "service_control",
    "terminal",
];
/// 构建版本：取本 crate 的版本号，与 `axon --version` 同源。
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 运行态库 `daemon_hosts.state` 的「运行中」取值，与 axon-session 的建库口径一致。
const HOST_STATE_RUNNING: &str = "running";
/// 运行态库 `jarvis_interactions.status` 的「已接收待领取」取值。
const INTERACTION_RECEIVED_STATUS: &str = "received";
/// 读租约时等待写锁的上限。
///
/// 状态路由占的是网关阻塞池线程，不能为一条 `SELECT` 长时间排队；拿不到锁时
/// 如实按「读不到」处理。
const HOST_LEASE_BUSY_TIMEOUT: Duration = Duration::from_millis(200);
/// 方法不匹配的错误码，与网关自身的 `405` 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 远端适配器：HTTP 网关。
const ADAPTER_GATEWAY_HTTP: &str = "gateway_http";
/// 远端适配器：WebSocket 网关。
const ADAPTER_GATEWAY_WEBSOCKET: &str = "gateway_websocket";
/// 远端适配器：飞书远控。
const ADAPTER_FEISHU: &str = "feishu";

/// 服务器时间：`/meta` 与 `/system/status` 的**唯一**取时实现。
///
/// 两条路由必须来自同一时钟源（Unix 毫秒）与同一函数，否则合同 §6.2 的
/// 「两者相差不得超过一秒」就失去判据。
fn server_now_ms() -> u64 {
    axon_time::unix_now_ms()
}

/// 从 `IP:端口` 形式的监听地址里取出端口。
///
/// 只认最后一段冒号后的数字：IPv6 的 `[::1]:3143` 与 `0.0.0.0:3143` 都要成立，
/// 解析不出来时返回 None，绝不猜一个默认端口给客户端。
fn bind_port(bind: &str) -> Option<u16> {
    let trimmed = bind.trim();
    let (_, port) = trimmed.rsplit_once(':')?;
    port.trim().parse::<u16>().ok()
}

/// 系统只读路由适配器。
pub(crate) struct SystemHttpAdapter {
    /// Axon 运行时根目录；运行态库位于其下。
    axon_root: PathBuf,
    /// 配置文件路径；远端适配器清单据此读取飞书远控开关。
    config_path: PathBuf,
    /// 适配器装配时刻，`uptimeMs` 的真实起点。
    started_at: Instant,
}

impl SystemHttpAdapter {
    /// 以运行时根目录与配置文件路径构造。
    ///
    /// 适配器在守护进程装配阶段（网关启动之前）构造一次，因此这里的时刻就是
    /// 守护进程可对外服务时长的起点，不另取进程启动时间。
    pub(crate) fn new(axon_root: impl Into<PathBuf>, config_path: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
            config_path: config_path.into(),
            started_at: Instant::now(),
        }
    }

    /// `/meta` 载荷：字段恰好五个，不返回构建版本号、主机名与路径。
    ///
    /// `super_admin` 讲的是**本次请求所用凭据是谁**，不是后端能力：客户端据此决定
    /// 是否渲染密钥管理入口。因此它必须来自鉴权身份，不能写成常量。
    fn meta_payload(&self, super_admin: bool) -> Value {
        let capabilities = CAPABILITY_NAMES
            .iter()
            .map(|name| ((*name).to_owned(), Value::Bool(true)))
            .collect::<Map<String, Value>>();
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": Value::Object(capabilities),
            "serverTimeMs": server_now_ms(),
            "uploadLimitBytes": UPLOAD_LIMIT_BYTES,
            "superAdmin": super_admin,
        })
    }

    /// `/system/status` 载荷：服务端运行态只读快照。
    fn status_payload(&self) -> Value {
        let (http_port, web_socket_port) = self.gateway_ports();
        json!({
            "serverTimeMs": server_now_ms(),
            "buildVersion": BUILD_VERSION,
            "lease": self.lease_snapshot(),
            "queueDepth": self.queue_depth(),
            "remoteAdapters": self.remote_adapters(),
            "uptimeMs": self.uptime_ms(),
            // 两个协议面的实际监听端口：客户端只填一个地址时据此把事件流指到正确的
            // 端口上。网关把 HTTP 与 WebSocket 绑成两条独立监听，客户端无法从
            // HTTP 地址里推出 WebSocket 端口，只能由后端自报。
            "httpPort": http_port,
            "webSocketPort": web_socket_port,
        })
    }

    /// 读网关两个协议面的监听端口。
    ///
    /// 只读配置里的两个 bind 键，不做全量校验：这是给客户端做地址校正用的运行态
    /// 事实，读不到就如实给 null，不猜默认端口。
    fn gateway_ports(&self) -> (Option<u16>, Option<u16>) {
        let values = axon_config::raw_string_values(
            &self.config_path,
            &["remote.gateway.http_bind", "remote.gateway.websocket_bind"],
        );
        (
            values
                .first()
                .and_then(Clone::clone)
                .and_then(|bind| bind_port(&bind)),
            values
                .get(1)
                .and_then(Clone::clone)
                .and_then(|bind| bind_port(&bind)),
        )
    }

    /// 守护进程租约读数。
    ///
    /// `active` 讲租约此刻是否仍然生效，`expiresInMs` 讲还剩多久（已过期时为 0），
    /// `owner` 是持有租约的 Host 身份。三者同出运行态库里的一行记录，因此要么一起
    /// 给出，要么一起为空。
    fn lease_snapshot(&self) -> Value {
        let now_ms = server_now_ms();
        match self.read_host_lease() {
            Ok(Some(lease)) => json!({
                "active": lease.expires_at_ms > now_ms,
                "expiresInMs": lease.expires_at_ms.saturating_sub(now_ms),
                "owner": lease.owner,
            }),
            // 运行态库里没有运行中的 Host 记录：确实没有生效租约，不是读不到。
            Ok(None) => json!({
                "active": false,
                "expiresInMs": Value::Null,
                "owner": Value::Null,
            }),
            // 库不存在、被锁或还没有建表：取不到就报 `null`，绝不伪造一个租约。
            Err(_) => Value::Null,
        }
    }

    /// 读取当前 Host 租约的拥有者与绝对到期时间。
    ///
    /// 来源是运行态库 `daemon_hosts` 表：`claim_host` 领取租约、心跳按 `renew_host`
    /// 续租，写的就是这一行，因此它是守护进程此刻真正持有的租约。只取
    /// `state = 'running'` 里到期时间最晚的一行：被回收的旧 Host 仍留在表里，
    /// 但它们已经不在运行。
    fn read_host_lease(&self) -> rusqlite::Result<Option<HostLease>> {
        let path = runtime_store(&self.axon_root).path().to_path_buf();
        // 库还没建出来时连打开都不做：只读路由不得顺手创建运行态库。
        if !path.is_file() {
            return Ok(None);
        }
        let connection = rusqlite::Connection::open(path)?;
        connection.busy_timeout(HOST_LEASE_BUSY_TIMEOUT)?;
        let mut statement = connection.prepare(
            "SELECT host_id, generation, lease_expires_at_ms FROM daemon_hosts \
             WHERE state = ?1 ORDER BY lease_expires_at_ms DESC LIMIT 1",
        )?;
        let mut rows = statement.query(rusqlite::params![HOST_STATE_RUNNING])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let host_id: String = row.get(0)?;
        let generation: String = row.get(1)?;
        let expires_at_ms: i64 = row.get(2)?;
        Ok(Some(HostLease {
            // 与 `DaemonHost::owner_identity` 的信息口径一致，但不带进程号：
            // 对外只讲「哪一台守护进程的哪一代」，不暴露进程身份。
            owner: format!("{host_id}:{generation}"),
            expires_at_ms: u64::try_from(expires_at_ms).unwrap_or(0),
        }))
    }

    /// 排队深度：已可靠接收、尚未被主 turn 领取的 Interaction 条数。
    ///
    /// 来源是运行态健康聚合（`HealthStore::read_runtime_health` 对
    /// `jarvis_interactions` 按状态计数），与既有 `axon daemon health` 的
    /// `interactions.byStatus.received` 同源同口径。读不到时如实返回 0。
    ///
    /// 该聚合在未建库时会按需建表，因此这条路由不会因为「库还没建」而失败。
    fn queue_depth(&self) -> u64 {
        match runtime_store(&self.axon_root).read_runtime_health(server_now_ms()) {
            Ok(counters) => counters
                .interactions
                .by_status
                .get(INTERACTION_RECEIVED_STATUS)
                .copied()
                .unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// 远端适配器清单。
    ///
    /// 只报配置与装配能证明的事实：
    ///
    /// - HTTP 与 WebSocket 两路由 `remote.gateway.http_bind` 与
    ///   `remote.gateway.websocket_bind` 驱动，在守护进程装配阶段一并注册并启动；
    ///   本路由能应答即证明 HTTP 这一路已就绪，而 `GatewayKernel::start` 任一路
    ///   失败都会让守护进程起不来，因此 WebSocket 这一路同样成立。
    /// - 飞书一路由 `remote.feishu_websocket.enabled` 开关决定，读配置即得真值。
    ///
    /// 配置文件读不出来（解析或校验失败）时没有任何可靠来源判断远端适配器，
    /// 此时给空数组，不猜。
    fn remote_adapters(&self) -> Value {
        let Ok(loaded) = ConfigLoader::load_from_path(&self.config_path) else {
            return json!([]);
        };
        json!([
            {"name": ADAPTER_GATEWAY_HTTP, "enabled": true},
            {"name": ADAPTER_GATEWAY_WEBSOCKET, "enabled": true},
            {
                "name": ADAPTER_FEISHU,
                "enabled": loaded.config.remote.feishu_websocket.enabled,
            },
        ])
    }

    /// 运行时长：从适配器装配时刻起算的真实毫秒数。
    fn uptime_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// 方法不匹配响应：路径属于本适配器，但只接受 `GET`。
    fn method_not_allowed() -> GatewayHttpResponse {
        GatewayHttpResponse::json(405, &json!({ "error": METHOD_NOT_ALLOWED }))
    }
}

impl GatewayHttpRouteHandler for SystemHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        // 没有身份信息时按「非超管」判定：`superAdmin` 讲的是「本次这把凭据是谁」，
        // 拿不到身份就不能声称是超管。
        self.route(method, path, query, body, false)
    }

    fn handle_request_as(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
        identity: Option<&GatewayIdentity>,
        _peer: Option<&str>,
    ) -> Option<GatewayHttpResponse> {
        let super_admin = identity.is_some_and(GatewayIdentity::is_super_admin);
        self.route(method, path, query, body, super_admin)
    }
}

impl SystemHttpAdapter {
    /// 两条只读路由的共同实现。
    fn route(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        _body: &[u8],
        super_admin: bool,
    ) -> Option<GatewayHttpResponse> {
        // 精确比较路径：这两条路由没有子路径，也不接受前缀匹配，未命中一律交回
        // 组合器，由后续适配器继续判断。
        if path != META_PATH && path != STATUS_PATH {
            return None;
        }
        if method != "GET" {
            return Some(Self::method_not_allowed());
        }
        let payload = if path == META_PATH {
            self.meta_payload(super_admin)
        } else {
            self.status_payload()
        };
        Some(GatewayHttpResponse::json(200, &payload))
    }
}

/// 运行态库里的一行 Host 租约。
struct HostLease {
    /// 持有租约的 Host 身份（Host 标识与 generation）。
    owner: String,
    /// 租约绝对到期时间（Unix 毫秒）。
    expires_at_ms: u64,
}

#[cfg(test)]
#[path = "system_http_adapter_tests.rs"]
mod system_http_adapter_tests;
