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
| Stage 6 | WS 端到端测试、会话状态、安装链 | 完成 | `88ba36410`、`c7e087493` |\n| Stage 7 | 发布链纳入宿主 | 完成 | `f57b9969c` |\n| Stage 8 | 自动安装语音运行时/模型 + 真实推理 | 完成 | `12b3311bf` |\n| Stage 9 | 朗读稿渲染（对齐 Codex 口语化做法） | 完成 | `f64791219` |
| Stage 10 | 角色扮演增强（可落地部分） | 完成 |  |
| Stage 11 | 与人格机制融合 | 完成 |  |
| Stage 12 | WS 真机端到端 | 完成 |  |
| Stage 13 | 语音回合编排与朗读下行 | 完成 | `88bc6a689`、`e1ad691ef`、`c4460cc5c` |

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

未完成：音频转写事件（依赖 ASR 运行时）、桌面端播放与 `playbackDrained` 等播放态协议、
release 产物与 MSI 安装包纳入宿主。'

## Stage 8：自动安装与真实推理

- 安装脚本自动下载 sherpa-onnx v2.0.1 与 VAD/ASR/TTS 模型（约 750 MB），生成 manifest；重复安装幂等。
- 自动接入：源码安装、MSI、deb、macOS pkg；zip/tar 附带脚本。
- 宿主调用官方命令行完成预热、转写与合成；心跳按真实状态上报。
- 真机测试：`cargo test -p axon-voice-host --test real_inference -- --ignored --nocapture` 通过（转写「你也好世界今天天气不错」，合成 6400 字节音频）。
- 未完成：daemon WS 会话保活/音频转发、桌面端播放、zip/tar 无安装钩子的自动补齐。
''

## Stage 9：朗读稿渲染

- 新增 `axon-voice::render`：六条规则（think/工具行、代码块与 diff 概括、表格不逐格、路径取文件名、URL 只读域名、脱敏），以及句子合并与 4 KiB 上限。
- 与 Codex 对齐：Codex 的实时语音由一个独立语音代理按提示词「不朗读表格/diff/代码块，只讲关键结论」生成口语内容；Axon 侧对应为「模型可提供专门朗读稿（spoken override）优先，缺失时由渲染器按规则生成」，正文与朗读稿分离。
- 详略：Minimal 只读结论，Concise 读开头结论与结尾，Detailed 全读。
- 宿主在 TTS 前做防御性二次渲染，避免任何路径直读 markdown。
- 单测 50 项（含 golden 规则、脱敏、短句合并、流式分块、override）；真机推理复验通过。
''

## Stage 10：角色扮演增强（可落地部分）

- 表现指令双流：`[whisper]`、`[sigh]`、`[laugh]` 与 `<express mood speed pitch energy>` 从正文剥离，只映射为合成参数；UI 不显示这些标记。
- 拟人停顿：省略号映射 400 ms、破折号 150 ms、普通句 120 ms，供播放端插入静音。
- 角色口癖：五个预设各带 `thinking_text` 与 `ack_text`（如冷静知性「让我想想……」）。
- 深夜模式：22 点到 6 点能量降低 2 dB、语速降到 0.95 倍。
- 朗读合成接口已用上表现参数与时段画像；默认音色为冷静知性。
- 未实现：CosyVoice2 指令式零样本 TTS 与角色克隆（需要真实模型、ONNX 导出与 C++ 引擎；需求文档中的 cxx 代码只是签名草图，不含引擎实现），留待模型就绪后按 ADR-009 复审条件推进。
''

## Stage 11：与人格机制融合

- `axon-voice::fusion`：人格表达参数（模式、温度、正式度、幽默、详略）决定预设与合成画像；显式预设只覆盖音色四字段，详略始终取人格。
- 群聊/事故偏干练助理；日常高温度偏明朗或俏皮；低温偏冷静；温度调语速、能量、音高；正式度调停顿；深夜降能量降速。
- 桌面 `voice_tts` 读取人格摘要并透传 persona，响应头返回实际预设；摘要缺失回默认基线。
- 测试：融合 8 条 + 语音 70 项 + daemon 203 项通过。
''

## Stage 12：WS 真机端到端

- 真实 WS 连接 + 真实音频 + 本地 ASR 通过，transcript「你也好世界今天天气不错」。
- 预热超时放宽到 45 s（CLI 冷加载约 20 到 30 s），合同 §5 的 10 s 目标需常驻引擎才能满足，已登记偏差。

## Stage 13：语音回合编排与朗读下行

- 提交走 INV-1 端口：WS `final` 转成 `ExternalEventEnvelope` 经 `GatewayContext::submit_external_event` 进入既有 durable ingress；渠道固定 `desktop`，车道键 `(workspace, 私有信任域, desktop, destination)` 与文本入口一致，不新建渠道、不新建 turn 表、无 loopback HTTP。
- epoch 只在提交成功时分配（会话内从 1 起），`turn` 帧携带 `interactionId`、`turnId`、`laneId`、`sessionId`、`epoch`；提交失败回 `submit_rejected(stage=thinking)`，会话保持且不消耗 epoch。
- 助手增量经 `GatewayContext::subscribe` 在 daemon 内部订阅（按本次 Interaction 过滤），`axon_voice::render` 渲染并按句切分后经桥的 `speak` 下发宿主；`seq` 每轮从 1 起。流式分块修正为不在代码围栏内切句，代码、URL 路径、绝对路径与令牌不外读。
- 宿主 `0x04` 音频原样透传为 WS `0x02`（`[0x02][u32be epoch][u32be pcmSeq][s16le]`），`pcmSeq` 不重编号；旧 epoch 音频一律丢弃。
- 停播闭环：一轮结束或 `client_close`/`user_cancel` 发 `stopAudio`，收到 `playbackDrained` 才迁回 `listening` 并记 `audio_flushed`；`speaking` 期 `user_cancel` 立即停播并同时通知宿主取消合成。回执上限 10 s，超时记 `internal` 并强制清理；超时值经 `VoiceTimeouts` 注入，不开放环境变量。
- 指标：`/api/v1/voice/status` 新增 `audioFlushed` 与 `playbackDrainsTimedOut`。
- 测试：daemon 239 项（新增 22 项，含 epoch 不消耗、`submit_rejected`、停播回执迁移、可注入超时强制清理，以及进程内脚本宿主的 `final → turn → speak → 0x02 → stopAudio → playbackDrained` 闭环与 `user_cancel` 打断）；语音 76 项、桥 12 项。
- 未完成：barge 打断判决与 `stopAudio(reason=barge)`、cue 与预览流、桌面端播放与真实声卡回执。

'