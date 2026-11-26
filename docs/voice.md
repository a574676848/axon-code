# 语音能力

Axon 的语音把「说话」当作输入方式，把「朗读」当作输出方式，仍然发生在同一个助理、同一条会话里。语音不会新增人格，也不会凭空造出一个新的会话类型。

---

## 1. 能力边界

- **输入**：桌面端采集麦克风音频，上行到本机后台服务，转写成文字后走与打字完全相同的对话链路，因此上下文、工具与权限完全一致。
- **输出**：把回复渲染成朗读稿后合成音频，客户端按顺序播放；打断由服务端判决并下令，客户端执行并回报已清空。
- **明确不做**：声音克隆、情感 TTS、唤醒词，也不把「打断」这个动作写进对话历史或提示词。

识别与合成由云端提供方承担，本机不再运行任何语音模型，也不需要下载模型文件。

---

## 2. 当前状态

**语音界面入口已全部开放**：桌面端的麦克风按钮、录音条、自动朗读开关、消息朗读按钮、设置里的语音输入设备与语音播报、人设页的音色选择都正常显示。

**端到端 realtime 车道已接入**：daemon 用一条 WebSocket 直连阿里百炼 realtime 服务，上行 16 kHz PCM，下行 24 kHz PCM 在 daemon 内降到 16 kHz 后交给桌面端播放。端点与模型来自 `[voice]` 段里 `role = "realtime"` 的模型；没有配置时按进程环境兜底（`AXON_VOICE_REALTIME_API_KEY` 或 `DASHSCOPE_API_KEY`），仍缺凭据就如实回落，不假装就绪。

**合成已接入火山方舟 Agent Plan**：daemon 直连 `wss://openspeech.bytedance.com/api/v3/plan/tts/unidirectional/stream`，文本进、WAV 出；`/tts`、`/cue`、`/preview` 三个接口按裸 WAV 下发，桌面端沿用既有播放模型。端点、资源标识与音色来自 `[voice]` 段里 `role = "tts"` 的模型，凭据按渠道的 `api_key_env` 解析（方舟填 `ARK_KEY`）。

**级联识别已接入火山方舟 Agent Plan**：daemon 直连 `wss://openspeech.bytedance.com/api/v3/plan/sauc/bigmodel_nostream`，上行 16 kHz PCM 按帧封装、末帧收尾，服务端 `result.text` 与 `utterances` 时间戳翻成既有 `partial`/`final` 事件。端点与资源标识来自 `[voice]` 段里 `role = "asr"` 的模型（`id` 填 `volc.seedasr.sauc.duration`），凭据按渠道的 `api_key_env` 解析（方舟填 `ARK_KEY`）；缺凭据时建会话如实回 `provider_unavailable`。两条车道互斥：realtime 配置齐全优先走 realtime，否则回落级联。

---

## 3. 使用中的状态提示

一次语音会话会经历下列状态，桌面端把它们表达为六种可见提示：

| 状态 | 界面表现 |
| --- | --- |
| `idle` | 不占行，入口待命 |
| `arming` | 正在准备（预热或连接提供方） |
| `listening` | 正在听，显示录音反馈 |
| `thinking` | 已定稿、正在生成回复（包含端点判定阶段） |
| `speaking` | 正在朗读，可打断 |
| `interrupted` | 已被打断，等待下一轮 |

出错与连接关闭不走六态，而是用提示条说明原因并复位入口。

---

## 4. 常见表现与处理

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 刚开口就被断开 | 连接后长时间没有有效音频 | 重新发起一次语音会话 |
| 单次会话自动结束 | 单次会话达到上限（120 秒）或空闲超时（30 秒） | 重新开始一轮 |
| 提示「正在准备」后失败 | 预热超时（10 秒），提供方没能在预算内就绪 | 稍后重试；持续失败查提供方状态 |
| 播放停不下来或停后不回听 | 客户端没有回报已清空播放队列 | 服务端在 10 秒后强制清理并记一次超时，界面回到可听状态 |
| 上行音频被丢弃计数 | 客户端送得比服务端消费快，队列满时丢最旧帧 | 正常现象，控制命令（停止、取消）永不丢弃 |
| 持续丢帧后断连 | 连续丢帧超过 10 秒，判定为背压 | 降低输入强度或检查网络后重连 |

---

## 5. 排障入口

`GET /api/v1/voice/status` 是语音唯一排障入口，鉴权与其他网关接口一致。关键字段：

- `runtimeAvailable`：能否建立语音会话；realtime 配置齐全或级联识别渠道齐备即为 `true`。
- `ttsAvailable`：合成渠道是否可用，与会话车道无关。
- `degraded`：不可用原因；正常情况下为 `null`。
- `presets`、`defaultPreset`：音色清单与默认音色。
- `idleTimeoutSec`、`sessionTimeoutSec`、`warmupTimeoutSec`：当前生效的超时值。
- `metrics`：会话数、队列深度、丢帧与最近错误。

错误码与含义：

| 错误码 | 含义 | 可重试 |
| --- | --- | --- |
| `provider_unavailable` | 提供方配置或凭据缺失，或建连失败 | 是（修配置后重试） |
| `model_missing` | 所选模型不可用（提供方不支持或未配置） | 是 |
| `model_unusable` | 提供方拒绝该模型标识 | 否 |
| `worker_unavailable` | 语音工作通道没能建立 | 是 |
| `worker_crashed` | 提供方通道中途断开 | 是 |
| `tts_failed` | 合成本身失败 | 是 |

---

## 6. 配置面

`[voice]` 段是唯一真值源：daemon 启动时把它映射成运行参数注入网关，识别、合成与会话三条能力都从这一份快照读取；桌面设置页与 `axon setup voice` 改的是同一段。字段与校验见[配置参考指南](./configuration.md)。

角色分工：`role = "realtime"` 的模型决定会话车道，`role = "tts"` 的模型决定合成渠道，`role = "asr"` 的模型决定级联识别渠道。缺配置或凭据时该能力如实报不可用，不拿空值去连。

没有任何 `[voice]` 配置时，按环境变量兜底（仅用于开发机与排障）：

| 变量 | 作用 | 默认值 |
| --- | --- | --- |
| `AXON_VOICE_REALTIME_API_KEY` | 百炼凭据；缺省时回退 `DASHSCOPE_API_KEY` | 无 |
| `AXON_VOICE_REALTIME_URL` | WS 端点，可自带 `model` 参数 | 百炼北京区 realtime 地址 |
| `AXON_VOICE_REALTIME_MODEL` | 模型标识 | `qwen-audio-3.1-realtime-flash` |
| `AXON_VOICE_REALTIME_VOICE` | 音色，只在会话首个配置里生效 | `longanlingxin_v3.1` |
| `AXON_VOICE_REALTIME_INSTRUCTIONS` | 系统提示词 | 内置的「回答简短口语化」 |
| `AXON_VOICE_REALTIME_SPEECH_RATE` | 语速 `0.5..=2.0`，越界忽略 | 服务端默认 |
| `AXON_VOICE_REALTIME_LANGUAGE` | 转写语言 | `zh` |
| `AXON_VOICE_REALTIME_MAX_HISTORY_TURNS` | 会话历史轮数 `1..=50` | 服务端默认 |
| `AXON_VOICE_REALTIME_SILENCE_MS` | VAD 静音判定 `200..=6000` | `800` |
| `AXON_VOICE_REALTIME_VAD_THRESHOLD` | VAD 阈值 `-1..=1` | `0.5` |
| `AXON_VOICE_REALTIME_TURN_DETECTION` | 轮次检测策略：`manual` 为按键说话，其余为服务端 VAD | `server_vad` |
| `AXON_VOICE_TTS_API_KEY` | 合成凭据；缺省回退 `ARK_KEY` | 无 |
| `AXON_VOICE_TTS_URL` | 合成端点 | 方舟 Agent Plan 单向流式地址 |
| `AXON_VOICE_TTS_RESOURCE_ID` | 合成资源标识 | `seed-tts-2.0` |
| `AXON_VOICE_TTS_SPEAKER` | 合成音色 | `zh_female_shuangkuaisisi_uranus_bigtts` |
| `AXON_VOICE_TTS_SPEECH_RATE` | 合成语速 `-50..=100`，越界忽略 | `0`（服务端默认） |
| `AXON_VOICE_ASR_API_KEY` | 识别凭据；缺省回退 `ARK_KEY`、`VOLC_API_KEY` | 无 |
| `AXON_VOICE_ASR_URL` | 识别端点 | 方舟 Agent Plan 流式识别地址 |
| `AXON_VOICE_ASR_RESOURCE_ID` | 识别资源标识 | `volc.seedasr.sauc.duration` |
| `AXON_VOICE_ASR_MODEL` | 上报的模型标识（协议内仍发 `bigmodel`） | `doubao-seed-asr-2.0` |
| `AXON_VOICE_ASR_LANGUAGE` | 转写语言，`auto` 由服务端判定 | `zh` |

采样率、帧长与超时仍是协议常量，不开放给用户调整；`AXON_RUNTIME_ROOT` 不影响语音。

---

## 7. 已知限制

- 级联车道的识别与合成都已接入真实提供方，协议翻译有单元用例、合成做过真实冒烟；整链（真人对讲一轮）尚未在真机上验证。realtime 车道有 M0-R 探针实测证据（握手、VAD、转写、回复音频、延迟都已核对）。
- realtime 的助手文字目前只下发 `assistant` 帧，界面接线在后续增量里。
- `metrics.rtfP50` 当前恒为 `null`。
- 试听接口的超时（30 秒）与协议参考值（3 秒）尚未对齐。
- 部分音频设备不接受 16 kHz，前端会重采样后再上行。
- 「打断」的判决链路有单元测试与进程内用例覆盖，尚未在真机上用真实对话轮次验证。

---

## 8. 相关文档

- [桌面工作台](./desktop.md) - 语音入口在界面中的位置
- [配置参考指南](./configuration.md) - 配置总览
- [故障诊断与自愈](./troubleshooting.md) - 连接与凭据类故障
