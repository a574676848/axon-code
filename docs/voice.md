# 语音能力

> 本文是 Axon 语音机制的单一事实来源（SSOT）。字段、超时与错误码以本文与代码为准；改契约先改本文。

## 1. 定位与边界

- 语音是**同一个 Axon** 的桌面入口，不新增人格、不新增渠道（`sourceChannel` / `replyChannel` 仍为 `desktop`），不新增用户可见的会话概念。
- **全本地**：识别与合成都在本机完成，不接任何云端语音服务；断网可用。
- 语音提交复用文本链路：转写定稿经 daemon 的 `GatewayContext::submit_external_event` 进入同一条交互链路（不另建回合表、不 loopback HTTP）。
- 当前入口只有桌面工作台：麦克风按钮在输入坞，音色与试听在「设置 › 人设 › 音色」。
- 明确不做：声音克隆、CosyVoice2、情感 TTS、唤醒词、把「打断」写进对话历史或提示词。

## 2. 进程与组件

```text
桌面 WebView（采集 / 播放）
   │  invoke('voice_audio', ArrayBuffer)  raw body
   ▼
axon-desktop（Rust）── WebSocket /api/v1/voice ──► axon-daemon
                                                   │  管道（二进制分帧）
                                                   ▼
                                          axon-voice-host（常驻子进程）
                                                   │  本地 WebSocket
                                                   ▼
                              sherpa-onnx-online-websocket-server（ASR，常驻）
                              sherpa-onnx-offline-tts（TTS，按句一次性调用）
```

- 识别是**常驻服务**：模型只加载一次，音频持续喂入，中间结果实时上抛；端点由服务端判定（静音 0.5 s）。
- 合成仍是**按句一次性调用**，每句重新加载模型；这是当前延迟的主要来源，见第 8 节。
- 宿主是唯一的模型持有者；daemon 不链接任何原生音频或推理库。

## 3. 运行时与模型布局

```text
$AXON_RUNTIME_ROOT/voice/
├── models/
│   ├── asr/<model>/manifest.json + 模型文件
│   ├── tts/<model>/manifest.json + 模型文件
│   └── .verify/<清单摘要>.json      校验印章（大小 + 修改时刻）
└── runtime/sherpa-onnx/<版本>/bin/  官方命令行与服务
```

- `AXON_RUNTIME_ROOT` 未设置时回落到 `$HOME/.axon/voice`。
- 模型清单 `manifest.json` 声明族、版本、采样率、语言、许可、每个文件的字节数与 SHA-256。
- 启动只校验元信息（不读整模型）；`warmup` 时才做全量哈希，并通过 `.verify` 印章缓存结果：文件大小或修改时刻变化即重新哈希，任何摘要不符都回 `model_unusable`。
- 模型文件缺失回 `model_missing`，不伪造 `warm`。
- 识别优先使用非量化（fp32）模型；`int8` 在个别语句上会出现重复字与噪声 token。

## 4. WebSocket 协议 `/api/v1/voice`

鉴权复用网关令牌：`Authorization: Bearer` 或查询串 `api_token`；未通过返回 401，不升级。

### 4.1 客户端文本帧

| 帧 | 字段 | 说明 |
| --- | --- | --- |
| `start` | `protocolVersion=1`、`clientSessionId`、`workspaceId`、`replyDestination`、`mode`（`push_to_talk`/`full_duplex`）、`lang`（`zh`/`en`/`auto`）、`tts`、`deviceId?` | 会话首帧，重复即协议错误 |
| `stop` | `utteranceSeq` | 结束当前句，会话保持 |
| `cancel` | `reason`（`user_cancel`/`client_close`） | 客户端不得发 `barge`，打断判决只在 daemon |
| `playbackDrained` | `epoch` | 客户端已清空该轮队列 |
| `setTts` / `setPreset` / `preview` / `cancelPreview` / `cueEnded` / `cueAck` | 见协议文档 | 当前已解析但不改变本机行为（`preview` 走 HTTP 接口） |

### 4.2 服务端文本帧

`ready`（连接级一次）、`state{phase}`、`partial{utteranceSeq,text}`、`final{utteranceSeq,text,startMs,endMs,finalizeMs}`、`turn{interactionId,turnId,laneId,sessionId,epoch}`、`speak{turnId,epoch,seq,text}`、`stopAudio{epoch,reason}`、`error{code,stage,retryable,message,detail}`、`closed{reason}`。

- `ready` 的 `asrModel` / `ttsModel` / `degraded` 来自真实文件探测；`workspaceId` 等会话级字段在 `start` 之前保持初始值。
- `degraded.asr` 只允许 `null`，`degraded.tts` 只允许 `null` / `"system"` / `"none"`，具体原因写在 `detail`（中文）。
- `epoch` 在提交成功时分配，从 1 起，`submit_rejected` 不消耗；客户端丢弃旧 `epoch` 的音频。

### 4.3 二进制帧

| 方向 | 布局 |
| --- | --- |
| 客户端 → daemon | `[0x01][s16le PCM]`，16 kHz 单声道，单帧 ≤ 8 KiB（256 ms） |
| daemon → 客户端 | `[0x02][u32be epoch][u32be pcmSeq][s16le PCM]` 对话音频 |
| daemon → 客户端 | `[0x03][u32be previewId][u32be previewSeq][s16le PCM]` 预览音频 |
| daemon → 客户端 | `[0x04][u32be generation][u32be cueId][u32be cueSeq][s16le PCM]` cue 音频 |

宿主按轮自增的 `pcmSeq` 由 daemon 原样透传，不重编号。

## 5. 相位与状态机

相位全集：`idle`、`arming`、`listening`、`endpointing`、`thinking`、`speaking`、`interrupted`、`error`、`closed`。

桌面展示六态：`idle`（不占行）、`arming`、`listening`、`thinking`（含 `endpointing`）、`speaking`、`interrupted`；`error` 与 `closed` 用提示条与入口复位表达。

## 6. 超时、背压与停播闭环

| 场景 | 值 | 行为 |
| --- | --- | --- |
| 连接后无有效帧 | 30 s | 回 `session_idle(connect)` 并关闭 |
| 单次会话最长 | 120 s | 回 `session_idle(listening)` 并关闭 |
| `start` 到宿主 `warm` | 10 s | `worker_unavailable` |
| `stopAudio` 到 `playbackDrained` | 10 s | 记 `internal` 并强制清理，指标 `playbackDrainsTimedOut` +1 |
| 试听 HTTP | 10 s | 504 `preview_timeout`（协议参考值是 3 s，见第 8 节偏差） |
| 上行队列 | 8 帧（约 0.8 s） | 满则丢最旧并计数，控制命令永不丢 |
| 连续丢帧 | 10 s | 回 `backpressure` 并断连；期间每秒最多回报一次 |

停播所有权：**daemon 判决并下令，客户端执行并回执**。`stopAudio` 后客户端在同一 tick 清空播放队列并回 `playbackDrained`，daemon 收到才把 `speaking` 迁回 `listening` 并记 `audioFlushed`。

推键松手走管道消息 `utteranceEnd`：宿主补 0.7 s 静音触发服务端端点，回 `final` 后继续听，不结束会话。

## 7. HTTP 接口与可观测面

| 接口 | 用途 |
| --- | --- |
| `GET /api/v1/voice/status` | 唯一排障入口 |
| `POST /api/v1/voice/tts` | 按音色合成一段朗读稿（`text`、可选 `presetId`、`persona`） |
| `POST /api/v1/voice/cue` | 合成角色短反馈（`kind`：`ack`/`thinking`/`interrupted_ack`/`reunion`/`done`/`error_ack`） |
| `GET /api/v1/voice/presets` | 音色清单（id/name/available/default） |
| `POST /api/v1/voice/preview` | 试听，成功 200 `audio/wav`，超时 504，音色不可用 409 |

`status` 字段：

- `runtimeAvailable`、`buildCommit`、`presetIds`、`defaultPreset`、`presets`
- `idleTimeoutSec=30`、`sessionTimeoutSec=120`、`warmupTimeoutSec=10`
- `degraded`：运行时不可用时给出稳定原因码，否则 `null`
- `metrics`：`sessionsStarted`、`sessionsActive`、`queueDepth`、`droppedFrames`、`droppedBytes`、`rtfP50`、`lastError`、`audioFlushed`、`playbackDrainsTimedOut`、`host{pid,rssKb,queueIn,queueOut,modelState}`

## 8. 已知偏差与限制

- `metrics.rtfP50` 来自预热时的 3 s 静音探针（分母含 0.8 s 尾补），是**下限口径**，不是真实语音的 P95。
- Windows 上 `metrics.host.rssKb` 恒为 0：宿主只实现了 Linux 的常驻内存读取。
- 试听 HTTP 超时是 10 s，协议参考值为 3 s；接口层与协议层尚未对齐。
- 合成按句一次性加载模型，单句延迟约 2 s 量级；识别已是常驻服务。
- 音频设备可能不接受 16 kHz（常见回落到 48 kHz），由前端重采样到 16 kHz 后再上行。
- 打断的 `barge` 判决链路有单测与集成用例覆盖，尚未在真机以真实助手回合验证。

## 9. 配置面

当前**没有** `[voice]` 用户配置段：采样率、帧长、超时都是协议常量或代码常量，不开放给用户配置（避免用配置换可测性）。

可用的运行态开关只有环境变量 `AXON_RUNTIME_ROOT`（运行态根，决定模型与运行时位置）。模型与运行时由安装脚本自动部署，升级不覆盖用户已有的模型文件。

## 10. 相关文档

- 帧与错误码全文：[`docs/skills/operate-axon/references/`](./skills/operate-axon/references/) 不含语音；详细字段以仓库合同 `PROTOCOL.md` 为准。
- 桌面端界面约束：[UI 约束规范](./ui-constraints.md)、[工作台设计系统](./workbench-design-system.md)。
- 配置总览：[配置参考指南](./configuration.md)。
