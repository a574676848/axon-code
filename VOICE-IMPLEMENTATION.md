# 语音实施进度（仓库内摘要）

> 详细台账与审计记录在合同仓库 `axon-voice-contract/IMPLEMENTATION.md`。
> 本文件只登记仓库内的阶段状态与范围，避免范围声明只存在于仓库之外。

## 阶段状态

| 阶段 | 范围 | 状态 | 提交 |
| --- | --- | --- | --- |
| Stage 1 | WP-01 领域合同（`axon-voice`） | 完成 | `6b582ea5e` |
| Stage 2 | WP-05 协议与宿主骨架（`axon-voice-protocol`、`axon-voice-host`） | 完成 | `833ccd232` |
| Stage 3 | WP-02 清单与校验 + WAV 输入 | 完成 | `40e3a6cab` |
| Stage 4 | WP-06 桥接内核（`axon-voice-bridge`） | 完成 | 本次提交 |
| Stage 5 | `/api/v1/voice` WS 接线 | 未开始 | — |

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
