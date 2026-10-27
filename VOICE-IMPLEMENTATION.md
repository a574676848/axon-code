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
| Stage 5 | `/api/v1/voice` WS 控制面 | 完成 | `530d07411` |
| Stage 6 | WS 端到端测试、会话状态、安装链 | 完成 | `88ba36410`、`c7e087493` |\n| Stage 7 | 发布链纳入宿主 | 完成 | `f57b9969c` |\n| Stage 8 | 自动安装语音运行时/模型 + 真实推理 | 完成，待审计 | `12b3311bf` |

## Stage 4 范围声明

Stage 4 交付 WP-06 的可独立验证内核：`HostTransport` 抽象与 `VoiceBridge` 状态机，
覆盖预热、会话、音频推入、取消、结束、事件映射、关闭与错误分类。

## Stage 5 范围声明

Stage 5 交付 `/api/v1/voice` 的 WS 控制面：鉴权、升级、`ready`、严格客户端帧、
`start` 时拉起宿主并预热、合同错误帧与协议错误断连。音频与转写链路不在本阶段。

## Stage 6 进展

- 真 socket 端到端：错 token 拒绝升级、`ready` 首帧、`start` 返回合同错误帧（199 项 daemon 单测含此用例）。
- 会话状态：`stop`/`cancel` 必须先 `start`；`client_close` 主动收线；音频帧必须是 `0x01` 开头、长度合法且已 `start`；
  预热失败后连接关闭，不留在半开状态。
- 预热与宿主握手在阻塞池执行，不占 tokio 工作线程。
- 安装链：`install-from-source.ps1` 与 `install-from-source.sh` 均构建并安装 `axon-voice-host`。

未完成：音频转写事件（依赖 ASR 运行时）、桌面端采集与播放、`playbackDrained` 等播放态协议、
release 产物与 MSI 安装包纳入宿主。'

## Stage 8：自动安装与真实推理

- 安装脚本自动下载 sherpa-onnx v2.0.1 与 VAD/ASR/TTS 模型（约 750 MB），生成 manifest；重复安装幂等。
- 自动接入：源码安装、MSI、deb、macOS pkg；zip/tar 附带脚本。
- 宿主调用官方命令行完成预热、转写与合成；心跳按真实状态上报。
- 真机测试：`cargo test -p axon-voice-host --test real_inference -- --ignored --nocapture` 通过（转写「你也好世界今天天气不错」，合成 6400 字节音频）。
- 未完成：daemon WS 会话保活/音频转发、桌面端采集播放、zip/tar 无安装钩子的自动补齐。
'