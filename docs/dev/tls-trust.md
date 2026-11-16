# 桌面客户端 TLS 信任来源（开发）

本文记录桌面客户端两条链路切换到平台根证书的具体落点。用户与运维可理解的结论见 [`../security-baseline.md`](../security-baseline.md) 第 2 节。

## 1. 两条链路各自的接线方式

- **HTTP 一侧**：`ureq` 启用 `platform-verifier` feature，构造 agent 时切换到平台验证器。
- **WebSocket 一侧**：`tokio-tungstenite` 没有对应的 feature，只有 `native-roots` 与 `webpki-roots`，因此必须自建 `Connector::Rustls(Arc<ClientConfig>)`，直接依赖 `rustls` 与 `rustls-platform-verifier` 两个 crate，并保证与既有 crypto provider 一致。

## 2. 为什么不能只用 native-roots

`rustls-tls-native-roots` 与平台验证器不等价：前者只读取根证书文件，不执行平台策略校验（企业策略、证书钉扎、吊销策略等）。两者不得混用后声称行为一致。

不做这件事的后果很具体：企业 TLS 中间盒下，供应商侧联通（走平台验证器），网关侧握手失败（只信编译期根），同一台机器表现为「模型能调通、网关连不上」。放宽或关闭校验都不是可接受的替代方案。

## 3. 相关文档

- [`../security-baseline.md`](../security-baseline.md) - 安全基线与部署侧职责
- [`./desktop-client-internals.md`](./desktop-client-internals.md) - 桌面客户端通信机制
