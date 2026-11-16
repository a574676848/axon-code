# 语音协议与接入（开发）

本文记录语音链路的组件边界、WebSocket 协议、帧布局、超时常量与云端提供方接入落点。用户可见的语音行为见 [../voice.md](../voice.md)。

---

## 1. 进程与组件


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

---

## 2. WebSocket 协议 /api/v1/voice、相位与超时


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

---

## 3. status 字段与指标


`status` 字段：

- `runtimeAvailable`（提供方是否可用，当前恒为 `false`）、`buildVersion`、`presetIds`、`defaultPreset`、`presets`
- `idleTimeoutSec=30`、`sessionTimeoutSec=120`、`warmupTimeoutSec=10`
- `degraded`：提供方不可用时为 `voice_provider_unavailable`，否则 `null`
- `metrics`：`sessionsStarted`、`sessionsActive`、`queueDepth`、`droppedFrames`、`droppedBytes`、`rtfP50`、`lastError`、`audioFlushed`、`playbackDrainsTimedOut`

---

## 4. 文案维护


文案只在 `axon-voice` 的 `VoiceFailure::user_message()` 一处维护，其它层不得另写一份。

---

## 5. 接入云端提供方的落点


1. 在 `crates/axon-daemon/src/infrastructure/gateway_voice_provider.rs` 的 `open()` 里返回 `ProviderSession`：实现 `HostTransport` 的传输、预热要请求的模型族与模型标识。
2. 在 `gateway_voice_ready.rs` 的 `provider_models()` 里返回真实模型名，`ready` 与 `status` 会同时翻成可用。
3. 合成接口把 `provider_unavailable_response()` 换成一次提供方合成调用；朗读稿渲染、人格融合与分句逻辑在 `axon-voice` 里现成可用。
4. 桌面端把 `VOICE_UI_ENABLED` 改回 `true`。

---

## 相关文档

- [../voice.md](../voice.md) - 用户可见的语音行为与限制
- [../configuration.md](../configuration.md) - 配置总览
