# 语音能力

> 本文是 Axon 语音机制的单一事实来源（SSOT）。字段、超时与错误码以本文与代码为准；改契约先改本文。

## 1. 定位与边界

- 语音是**同一个 Axon** 的桌面入口，不新增人格、不新增渠道（`sourceChannel` / `replyChannel` 仍为 `desktop`），不新增用户可见的会话概念。
- 识别与合成由**云端提供方**承担（规划为阿里百炼与火山引擎）。本机模型宿主已整体移除，仓库不再包含任何本机推理、模型清单或模型下载逻辑。
- 语音提交复用文本链路：转写定稿经 daemon 的 `GatewayContext::submit_external_event` 进入同一条交互链路（不另建回合表、不 loopback HTTP）。
- 明确不做：声音克隆、情感 TTS、唤醒词、把「打断」写进对话历史或提示词。

## 2. 当前状态（开工前必读）

- 没有任何云端提供方接入：`gateway_voice_provider::open()` 恒返回 `provider_unavailable`，`/api/v1/voice` 收到 `start` 即回 `error{code=provider_unavailable, stage=arming}` 并关闭连接。
- 三个合成接口（`/tts`、`/cue`、`/preview`）统一返回 409 `provider_unavailable`，不产出音频；音色清单与运行态读数照常可读。
- 桌面端语音入口与配置**已隐藏**：`ui/src/speech/enabled.ts` 的 `VOICE_UI_ENABLED = false` 收起了输入坞麦克风、录音条、顶栏自动朗读开关、消息朗读按钮、设置里的语音输入设备与语音播报、人设页的音色。组件、协议与宿主命令全部保留，改回 `true` 即恢复。
- 安装脚本不下载任何语音资源：sherpa-onnx 运行时与 VAD/ASR/TTS 模型、`.provisioned.json` 标记与安装期预置流程都不在仓库与安装链中。

## 3. 进程与组件

```text
桌面 WebView（采集 / 播放）
   │  invoke('voice_audio', ArrayBuffer)  raw body
   ▼
axon-desktop（Rust）── WebSocket /api/v1/voice ──► axon-daemon
                                                   │  帧解析 → 会话/超时 → 工作线程
                                                   ▼
                                      axon-voice-bridge（HostTransport 端口）
                                                   │
                                                   ▼
                                云端提供方适配器（阿里百炼 / 火山引擎，待接入）
```

- daemon 不链接任何原生音频或推理库；它只负责帧、相位、提交、朗读稿渲染与停播闭环。
- 工作线程把提供方通道映射成事件：上行音频进有界队列（满了丢最旧并计数），控制命令（`stop`/`cancel`/`shutdown`）永不丢弃。
- 提供方实现 `HostTransport`（`crates/axon-voice-bridge`）：收发控制帧、读取一帧、心跳存活判定与关闭。进程监督、模型加载都属于提供方实现自己的事。

## 4. 保留的基础设施

| 组件 | 职责 |
| --- | --- |
| `crates/axon-voice` | 领域合同：相位状态机、失败分类、音色预设与韵律画像、朗读稿渲染、人格融合、`SpeechToText` / `TextToSpeech` 端口 |
| `crates/axon-voice-protocol` | 提供方管道帧：控制帧 JSON 与 `0x01`/`0x02`/`0x03`/`0x04` 二进制帧的编解码与校验 |
| `crates/axon-voice-bridge` | 桥状态机：预热、会话、推音频、取消、结束、事件映射与关闭 |
| daemon `gateway_voice*` | `/api/v1/voice` 入口、帧解析、回合编排、提供方工作线程、指标、HTTP 接口 |
| 桌面 `speech/**` | 采集与合帧、回声抑制外的播放队列、六态映射、偏好存储、协议帧构造 |

## 5. WebSocket 协议 `/api/v1/voice`

鉴权复用网关令牌：`Authorization: Bearer` 或查询串 `api_token`；未通过返回 401，不升级。

### 5.1 客户端文本帧

| 帧 | 字段 | 说明 |
| --- | --- | --- |
| `start` | `protocolVersion=1`、`clientSessionId`、`workspaceId`、`replyDestination`、`mode`（`push_to_talk`/`full_duplex`）、`lang`（`zh`/`en`/`auto`）、`tts`、`deviceId?` | 会话首帧，重复即协议错误 |
| `stop` | `utteranceSeq` | 结束当前句，会话保持 |
| `cancel` | `reason`（`user_cancel`/`client_close`） | 客户端不得发 `barge`，打断判决只在 daemon |
| `playbackDrained` | `epoch` | 客户端已清空该轮队列 |
| `setTts` / `setPreset` / `preview` / `cancelPreview` / `cueEnded` / `cueAck` | 见协议文档 | 当前已解析但不改变行为（`preview` 走 HTTP 接口） |

### 5.2 服务端文本帧

`ready`（连接级一次）、`state{phase}`、`partial{utteranceSeq,text}`、`final{utteranceSeq,text,startMs,endMs,finalizeMs}`、`turn{interactionId,turnId,laneId,sessionId,epoch}`、`speak{turnId,epoch,seq,text}`、`stopAudio{epoch,reason}`、`error{code,stage,retryable,message,detail}`、`closed{reason}`。

- `ready` 的 `asrModel` 在提供方接入前恒为 `"unavailable"`，`ttsModel` 恒为 `"none"`，`tts` 为 `false`；原因写在 `degraded.detail`（中文）。接入后两者改为提供方配置里的真实模型名。
- `degraded.asr` 与 `degraded.tts` 只能是 `null`（没有系统音色兜底，也不编造第二套词汇），具体原因统一进 `detail`。
- `epoch` 在提交成功时分配，从 1 起，`submit_rejected` 不消耗；客户端丢弃旧 `epoch` 的音频。

### 5.3 二进制帧

| 方向 | 布局 |
| --- | --- |
| 客户端 → daemon | `[0x01][s16le PCM]`，16 kHz 单声道，单帧 ≤ 8 KiB（256 ms） |
| daemon → 客户端 | `[0x02][u32be epoch][u32be pcmSeq][s16le PCM]` 对话音频 |
| daemon → 客户端 | `[0x03][u32be previewId][u32be previewSeq][s16le PCM]` 预览音频 |
| daemon → 客户端 | `[0x04][u32be generation][u32be cueId][u32be cueSeq][s16le PCM]` cue 音频 |

提供方按轮自增的 `pcmSeq` 由 daemon 原样透传，不重编号。

## 6. 相位与状态机

相位全集：`idle`、`arming`、`listening`、`endpointing`、`thinking`、`speaking`、`interrupted`、`error`、`closed`。

桌面展示六态：`idle`（不占行）、`arming`、`listening`、`thinking`（含 `endpointing`）、`speaking`、`interrupted`；`error` 与 `closed` 用提示条与入口复位表达。

## 7. 超时、背压与停播闭环

| 场景 | 值 | 行为 |
| --- | --- | --- |
| 连接后无有效帧 | 30 s | 回 `session_idle(connect)` 并关闭 |
| 单次会话最长 | 120 s | 回 `session_idle(listening)` 并关闭 |
| `start` 到提供方 `warm` | 10 s | `worker_unavailable` |
| `stopAudio` 到 `playbackDrained` | 10 s | 记 `internal` 并强制清理，指标 `playbackDrainsTimedOut` +1 |
| 上行队列 | 8 帧（约 2 s） | 满则丢最旧并计数，控制命令永不丢 |
| 连续丢帧 | 10 s | 回 `backpressure` 并断连；期间每秒最多回报一次 |

停播所有权：**daemon 判决并下令，客户端执行并回执**。`stopAudio` 后客户端在同一 tick 清空播放队列并回 `playbackDrained`，daemon 收到才把 `speaking` 迁回 `listening` 并记 `audioFlushed`。

推键松手走管道消息 `utteranceEnd`：由提供方补静音触发端点，回 `final` 后继续听，不结束会话。

## 8. HTTP 接口与可观测面

| 接口 | 用途 |
| --- | --- |
| `GET /api/v1/voice/status` | 唯一排障入口 |
| `POST /api/v1/voice/tts` | 按音色合成一段朗读稿（`text`、可选 `presetId`）；当前 409 `provider_unavailable` |
| `POST /api/v1/voice/cue` | 合成角色短反馈（`kind`：`ack`/`thinking`/`interrupted_ack`/`reunion`/`done`/`error_ack`）；当前 409 `provider_unavailable` |
| `GET /api/v1/voice/presets` | 音色清单（id/name/available/default） |
| `POST /api/v1/voice/preview` | 试听；当前 409 `provider_unavailable` |

`status` 字段：

- `runtimeAvailable`（提供方是否可用，当前恒为 `false`）、`buildVersion`、`presetIds`、`defaultPreset`、`presets`
- `idleTimeoutSec=30`、`sessionTimeoutSec=120`、`warmupTimeoutSec=10`
- `degraded`：提供方不可用时为 `voice_provider_unavailable`，否则 `null`
- `metrics`：`sessionsStarted`、`sessionsActive`、`queueDepth`、`droppedFrames`、`droppedBytes`、`rtfP50`、`lastError`、`audioFlushed`、`playbackDrainsTimedOut`

## 9. 错误码

除协议错误码外，与提供方相关的取值：

| 错误码 | 含义 | 可重试 |
| --- | --- | --- |
| `provider_unavailable` | 尚未接入云端语音提供方 | 否 |
| `model_missing` | 所选模型不可用（提供方不支持或未配置） | 是 |
| `model_unusable` | 提供方拒绝该模型标识 | 否 |
| `worker_unavailable` | 语音工作线程没能建立通道 | 是 |
| `worker_crashed` | 提供方通道中途断开 | 是 |
| `tts_failed` | 合成本身失败 | 是 |

文案只在 `axon-voice` 的 `VoiceFailure::user_message()` 一处维护，其它层不得另写一份。

## 10. 接入云端提供方的落点

1. 在 `crates/axon-daemon/src/infrastructure/gateway_voice_provider.rs` 的 `open()` 里返回 `ProviderSession`：实现 `HostTransport` 的传输、预热要请求的模型族与模型标识。
2. 在 `gateway_voice_ready.rs` 的 `provider_models()` 里返回真实模型名，`ready` 与 `status` 会同时翻成可用。
3. 合成接口把 `provider_unavailable_response()` 换成一次提供方合成调用；朗读稿渲染、人格融合与分句逻辑在 `axon-voice` 里现成可用。
4. 桌面端把 `VOICE_UI_ENABLED` 改回 `true`。

## 11. 已知限制

- 没有任何真机 ASR/TTS 端到端证据：本机模型移除后，转写与合成链路只在进程内脚本提供方的用例中验证编排，真实音频未验证。
- `metrics.rtfP50` 只在提供方预热回执里写入，当前恒为 `null`。
- 试听 HTTP 超时常量为 30 s，协议参考值是 3 s；两者尚未对齐。
- 音频设备可能不接受 16 kHz（常见回落到 48 kHz），由前端重采样到 16 kHz 后再上行。
- 打断的 `barge` 判决链路有单测与进程内用例覆盖，尚未在真机以真实助手回合验证。

## 12. 配置面

当前**没有** `[voice]` 用户配置段：采样率、帧长、超时都是协议常量或代码常量，不开放给用户配置（避免用配置换可测性）。提供方凭据与模型标识属于接入工作的一部分，接入时再定义配置键。

`AXON_RUNTIME_ROOT` 不再影响语音：模型、运行时与下载目录都不存在了。

## 13. 相关文档

- 桌面端职责与命令：`docs/skills/operate-axon/references/desktop-app.md`。
- 桌面端界面约束：[UI 约束规范](./ui-constraints.md)、[工作台设计系统](./workbench-design-system.md)。
- 配置总览：[配置参考指南](./configuration.md)。
