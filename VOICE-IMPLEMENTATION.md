# 语音实施进度（仓库内摘要）

> 详细台账与审计记录在合同仓库 `axon-voice-contract/IMPLEMENTATION.md`。
> 本文件只登记仓库内的阶段状态与范围，避免范围声明只存在于仓库之外。

## 阶段状态

| 阶段 | 范围 | 状态 | 提交 |
| --- | --- | --- | --- |
| Stage 1 | WP-01 领域合同（`axon-voice`） | 完成 | `6b582ea5e` |
| Stage 2 | WP-05 协议与宿主骨架（`axon-voice-protocol`、`axon-voice-host`） | 完成 | `833ccd232` |
| Stage 3 | WP-02 清单与校验 + WAV 输入 | 完成 | `40e3a6cab` |
| Stage 4 | WP-06 桥接内核（`axon-voice-bridge`） | 完成 | `90fcc459c` |
| Stage 5 | `/api/v1/voice` WS 入口（握手 + 预热错误） | 实现完成，审计中 | `96f9307e6` |

## Stage 4 范围声明

本阶段交付 WP-06 的可独立验证内核：`HostTransport` 抽象与 `VoiceBridge` 状态机，
覆盖预热、会话、音频推入、取消、结束、事件映射、关闭与错误分类。

**不包含** `/api/v1/voice` 的 WS 路由。原因：

- 仓库当前 axum 只启用了 `http1`、`tokio`，没有 `ws` feature，也没有入站
  `WebSocketUpgrade`/`on_upgrade` 处理器可复用；
- 现有 `gateway_websocket` 是出站客户端适配器，不是入站服务端。

接线点已确认（`gateway_http_routes.rs` 的 `router()` 增路由，handler 用
`WebSocketUpgrade`，`on_upgrade` 内以阻塞任务独占 `VoiceBridge`），预计 2 人日，
放在 Stage 5，与桌面端链路一起验证。

## 当前限制

ASR 运行时尚未接入：真实宿主对合法模型回 `model_unusable`（原因注明运行时未接入），
心跳恒 `cold`；桥会把它作为 `BridgeError::Host` 上抛，不会伪造就绪。
## Stage 5 交付与边界

已交付：axum `ws` feature、`/api/v1/voice` 路由（在 `/api/v1/{*rest}` 兜底之前）、网关 token 鉴权、
连接后 `ready`、客户端控制帧严格解析、`start` 时拉起宿主并预热、错误帧按合同字段回传。

未交付（Stage 6）：音频二进制帧上行、partial/final 转写事件、`state` 相位帧、playbackDrained、
WS 端到端测试（真实 socket + 客户端）、桌面端接入。原因：这些依赖 ASR 运行时，当前宿主只能回
`model_unusable`，先打通控制面与错误面，避免用假转写撑门面。