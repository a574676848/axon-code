//! 控制消息：类型、字段与严格编解码。

use serde::{Deserialize, Serialize};

use crate::frame::FrameError;
use crate::{MAX_FRAME_BYTES, type_name};

/// 控制消息。
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// 父进程问候。
    Hello(Hello),
    /// 宿主就绪。
    Ready(Ready),
    /// 预热请求。
    Warmup(Warmup),
    /// 预热完成。
    Warm(Warm),
    /// 开始会话。
    SessionStart(SessionStart),
    /// 结束会话。
    SessionEnd(SessionEnd),
    /// 中间识别结果。
    Partial(Partial),
    /// 定稿。
    Final(Final),
    /// 合成播报。
    Speak(Speak),
    /// 合成 cue。
    CueSpeak(CueSpeak),
    /// cue 缓存状态。
    CueStatus(CueStatus),
    /// 取消。
    Cancel(Cancel),
    /// 心跳。
    Heartbeat(Heartbeat),
    /// 错误。
    Error(ErrorMsg),
    /// 关闭请求。
    Close,
    /// 关闭确认。
    Closed,
}

impl Control {
    /// 消息类型名。
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Hello(_) => type_name::HELLO,
            Self::Ready(_) => type_name::READY,
            Self::Warmup(_) => type_name::WARMUP,
            Self::Warm(_) => type_name::WARM,
            Self::SessionStart(_) => type_name::SESSION_START,
            Self::SessionEnd(_) => type_name::SESSION_END,
            Self::Partial(_) => type_name::PARTIAL,
            Self::Final(_) => type_name::FINAL,
            Self::Speak(_) => type_name::SPEAK,
            Self::CueSpeak(_) => type_name::CUE_SPEAK,
            Self::CueStatus(_) => type_name::CUE_STATUS,
            Self::Cancel(_) => type_name::CANCEL,
            Self::Heartbeat(_) => type_name::HEARTBEAT,
            Self::Error(_) => type_name::ERROR,
            Self::Close => type_name::CLOSE,
            Self::Closed => type_name::CLOSED,
        }
    }

    /// 编码为控制帧。
    ///
    /// # Errors
    ///
    /// 序列化失败或超过 [`MAX_FRAME_BYTES`]。
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let payload = encode_payload(self)?;
        let mut frame = Vec::with_capacity(payload.len() + 5);
        frame.push(crate::KIND_CONTROL);
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);
        Ok(frame)
    }

    /// 从 JSON 载荷解码，要求字段严格匹配。
    ///
    /// # Errors
    ///
    /// JSON 非法、类型未知或出现未知字段。
    pub fn decode(payload: &[u8]) -> Result<Self, FrameError> {
        decode_payload(payload)
    }
}

/// `hello` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    /// 协议版本。
    pub protocol: u32,
    /// 构建号，必须与宿主一致。
    pub build_commit: String,
    /// 角色，固定 `daemon`。
    pub role: String,
    /// 父进程 id。
    pub pid: u32,
}

/// `ready` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ready {
    /// 协议版本。
    pub protocol: u32,
    /// 构建号。
    pub build_commit: String,
    /// 宿主版本。
    pub host_version: String,
}

/// `warmup` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Warmup {
    /// 模型族。
    pub family: String,
    /// 模型标识。
    pub model: String,
    /// 清单哈希。
    pub manifest_sha256: String,
}

/// `warm` 消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Warm {
    /// 模型族。
    pub family: String,
    /// 模型标识。
    pub model: String,
    /// 实时因子中位数。
    pub rtf_p50: f64,
    /// 常驻内存。
    pub rss_kb: u64,
}

/// `sessionStart` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStart {
    /// 会话标识。
    pub session_id: String,
    /// 会话模式。
    pub mode: String,
    /// 语言提示。
    pub lang: String,
    /// 热词。
    pub hot_words: Vec<String>,
    /// 采样率，固定 16000。
    pub sample_rate: u32,
}

/// `sessionEnd` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionEnd {
    /// 会话标识。
    pub session_id: String,
    /// 结束原因。
    pub reason: String,
}

/// `partial` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Partial {
    /// 句序号。
    pub utterance_seq: u32,
    /// 文本。
    pub text: String,
}

/// `final` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Final {
    /// 句序号。
    pub utterance_seq: u32,
    /// 文本。
    pub text: String,
    /// 起始时刻。
    pub start_ms: u64,
    /// 端点判定时刻。
    pub end_ms: u64,
    /// 定稿产出时刻。
    pub finalize_ms: u64,
}

/// `speak` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Speak {
    /// 回合标识。
    pub turn_id: String,
    /// 轮次。
    pub epoch: u32,
    /// 句子序号。
    pub seq: u32,
    /// 文本。
    pub text: String,
}

/// `cueSpeak` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CueSpeak {
    /// cue 标识。
    pub cue_id: u32,
    /// 代次，换音色后递增。
    pub generation: u32,
    /// 种类。
    pub kind: String,
    /// 文本。
    pub text: String,
}

/// `cueStatus` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CueStatus {
    /// 已缓存条数。
    pub cached: u32,
    /// 不可用原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

/// `cancel` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancel {
    /// 取消原因。
    pub reason: String,
}

/// `heartbeat` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Heartbeat {
    /// 进程 id。
    pub pid: u32,
    /// 常驻内存。
    pub rss_kb: u64,
    /// 模型状态。
    pub model_state: String,
    /// 入站队列深度。
    pub queue_in: u32,
    /// 出站队列深度。
    pub queue_out: u32,
}

/// `error` 的补充字段。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorDetail {
    /// 模型族。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// 退出码。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// 子阶段或原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 丢弃帧数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_frames: Option<u64>,
}

/// `error` 消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorMsg {
    /// 稳定错误码。
    pub code: String,
    /// 阶段。
    pub stage: String,
    /// 是否可重试。
    pub retryable: bool,
    /// 补充字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<ErrorDetail>,
}

impl Control {
    /// 字段取值与长度校验；编解码两侧共用。
    ///
    /// # Errors
    ///
    /// 越界或取值集不符时返回 [`FrameError::InvalidValue`]。
    pub fn validate(&self) -> Result<(), FrameError> {
        const PARTIAL_MAX: usize = 4 * 1024;
        const FINAL_MAX: usize = 16 * 1024;
        const CUE_MAX_CHARS: usize = 24;
        const SESSION_ID_MAX: usize = 64;
        match self {
            Self::SessionStart(message) => {
                if message.session_id.is_empty() || message.session_id.len() > SESSION_ID_MAX {
                    return Err(invalid("sessionId"));
                }
                if message.sample_rate != 16_000 {
                    return Err(invalid("sampleRate"));
                }
                if !matches!(message.mode.as_str(), "push_to_talk" | "full_duplex") {
                    return Err(invalid("mode"));
                }
                if !matches!(message.lang.as_str(), "zh" | "en" | "auto") {
                    return Err(invalid("lang"));
                }
                if message.hot_words.len() > 200
                    || message
                        .hot_words
                        .iter()
                        .any(|word| word.chars().count() > 64)
                {
                    return Err(invalid("hotWords"));
                }
            }
            Self::SessionEnd(message) => {
                if !matches!(message.reason.as_str(), "client_close" | "idle" | "error") {
                    return Err(invalid("reason"));
                }
            }
            Self::Warmup(message) => {
                if !matches!(message.family.as_str(), "vad" | "asr" | "tts" | "cue") {
                    return Err(invalid("family"));
                }
                if !is_sha256_hex(&message.manifest_sha256) {
                    return Err(invalid("manifestSha256"));
                }
            }
            Self::Warm(message) => {
                if !matches!(message.family.as_str(), "vad" | "asr" | "tts" | "cue") {
                    return Err(invalid("family"));
                }
                if !message.rtf_p50.is_finite() || message.rtf_p50 < 0.0 {
                    return Err(invalid("rtfP50"));
                }
            }
            Self::Partial(message) => {
                if message.text.len() > PARTIAL_MAX {
                    return Err(invalid("text"));
                }
            }
            Self::Final(message) => {
                if message.text.len() > FINAL_MAX {
                    return Err(invalid("text"));
                }
                if message.end_ms < message.start_ms || message.finalize_ms < message.end_ms {
                    return Err(invalid("timestamps"));
                }
            }
            Self::Speak(message) => {
                if message.text.len() > PARTIAL_MAX {
                    return Err(invalid("text"));
                }
            }
            Self::CueSpeak(message) => {
                if message.text.chars().count() > CUE_MAX_CHARS {
                    return Err(invalid("text"));
                }
                if !matches!(
                    message.kind.as_str(),
                    "ack" | "thinking" | "interrupted_ack" | "reunion" | "done" | "error_ack"
                ) {
                    return Err(invalid("kind"));
                }
            }
            Self::Cancel(message) => {
                if !matches!(
                    message.reason.as_str(),
                    "barge" | "user_cancel" | "turn_end"
                ) {
                    return Err(invalid("reason"));
                }
            }
            Self::Heartbeat(message) => {
                if !matches!(message.model_state.as_str(), "cold" | "warming" | "warm") {
                    return Err(invalid("modelState"));
                }
            }
            Self::Hello(_)
            | Self::Ready(_)
            | Self::CueStatus(_)
            | Self::Error(_)
            | Self::Close
            | Self::Closed => {}
        }
        Ok(())
    }
}

fn invalid(field: &'static str) -> FrameError {
    FrameError::InvalidValue { field }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn encode_payload(control: &Control) -> Result<Vec<u8>, FrameError> {
    let mut value = match control {
        Control::Hello(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::Ready(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::Warmup(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::Warm(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::SessionStart(message) => {
            serde_json::to_value(message).map_err(FrameError::Serialize)?
        }
        Control::SessionEnd(message) => {
            serde_json::to_value(message).map_err(FrameError::Serialize)?
        }
        Control::Partial(message) => {
            serde_json::to_value(message).map_err(FrameError::Serialize)?
        }
        Control::Final(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::Speak(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::CueSpeak(message) => {
            serde_json::to_value(message).map_err(FrameError::Serialize)?
        }
        Control::CueStatus(message) => {
            serde_json::to_value(message).map_err(FrameError::Serialize)?
        }
        Control::Cancel(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::Heartbeat(message) => {
            serde_json::to_value(message).map_err(FrameError::Serialize)?
        }
        Control::Error(message) => serde_json::to_value(message).map_err(FrameError::Serialize)?,
        Control::Close | Control::Closed => serde_json::json!({}),
    };
    let map = value.as_object_mut().ok_or(FrameError::Truncated)?;
    map.insert(
        "type".to_owned(),
        serde_json::Value::String(control.type_name().to_owned()),
    );
    let payload = serde_json::to_vec(&value).map_err(FrameError::Serialize)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { len: payload.len() });
    }
    Ok(payload)
}

fn decode_payload(payload: &[u8]) -> Result<Control, FrameError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { len: payload.len() });
    }
    let mut value: serde_json::Value =
        serde_json::from_slice(payload).map_err(FrameError::InvalidJson)?;
    let map = value.as_object_mut().ok_or(FrameError::Truncated)?;
    let kind = map
        .remove("type")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(FrameError::Truncated)?;
    let control = match kind.as_str() {
        type_name::HELLO => Control::Hello(from_value(value)?),
        type_name::READY => Control::Ready(from_value(value)?),
        type_name::WARMUP => Control::Warmup(from_value(value)?),
        type_name::WARM => Control::Warm(from_value(value)?),
        type_name::SESSION_START => Control::SessionStart(from_value(value)?),
        type_name::SESSION_END => Control::SessionEnd(from_value(value)?),
        type_name::PARTIAL => Control::Partial(from_value(value)?),
        type_name::FINAL => Control::Final(from_value(value)?),
        type_name::SPEAK => Control::Speak(from_value(value)?),
        type_name::CUE_SPEAK => Control::CueSpeak(from_value(value)?),
        type_name::CUE_STATUS => Control::CueStatus(from_value(value)?),
        type_name::CANCEL => Control::Cancel(from_value(value)?),
        type_name::HEARTBEAT => Control::Heartbeat(from_value(value)?),
        type_name::ERROR => Control::Error(from_value(value)?),
        type_name::CLOSE => require_empty(value, Control::Close)?,
        type_name::CLOSED => require_empty(value, Control::Closed)?,
        other => return Err(FrameError::UnknownType(other.to_owned())),
    };
    control.validate()?;
    Ok(control)
}

fn require_empty(value: serde_json::Value, control: Control) -> Result<Control, FrameError> {
    let map = value.as_object().ok_or(FrameError::Truncated)?;
    if let Some(field) = map.keys().next() {
        return Err(FrameError::UnknownField(field.clone()));
    }
    Ok(control)
}

fn from_value<T>(value: serde_json::Value) -> Result<T, FrameError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).map_err(FrameError::InvalidJson)
}

#[cfg(test)]
mod tests {
    use super::{
        Cancel, Control, ErrorDetail, ErrorMsg, Final, Heartbeat, Hello, Partial, Ready,
        SessionEnd, SessionStart,
    };
    use crate::frame::FrameError;

    fn hello() -> Control {
        Control::Hello(Hello {
            protocol: 1,
            build_commit: "abc".to_owned(),
            role: "daemon".to_owned(),
            pid: 42,
        })
    }

    #[test]
    fn control_roundtrip_keeps_fields() {
        let frame = hello().encode().expect("编码应成功");
        assert_eq!(frame[0], crate::KIND_CONTROL);
        assert_eq!(
            crate::decode_frame(&frame).expect("解码应成功"),
            crate::Frame::Control(hello())
        );
    }

    #[test]
    fn decode_rejects_unknown_field() {
        let payload = br#"{"type":"cancel","reason":"barge","extra":1}"#;
        assert!(matches!(
            Control::decode(payload),
            Err(FrameError::InvalidJson(_))
        ));
    }

    #[test]
    fn decode_rejects_unknown_type() {
        assert!(matches!(
            Control::decode(br#"{"type":"nope"}"#),
            Err(FrameError::UnknownType(_))
        ));
    }

    #[test]
    fn validation_rejects_bad_timestamps_and_text() {
        let bad_order = Control::Final(Final {
            utterance_seq: 1,
            text: "好".to_owned(),
            start_ms: 20,
            end_ms: 10,
            finalize_ms: 30,
        });
        assert!(matches!(
            bad_order.validate(),
            Err(FrameError::InvalidValue {
                field: "timestamps"
            })
        ));

        let huge = Control::Partial(Partial {
            utterance_seq: 1,
            text: "好".repeat(4 * 1024 + 1),
        });
        assert!(matches!(
            huge.validate(),
            Err(FrameError::InvalidValue { field: "text" })
        ));
    }

    #[test]
    fn fieldless_messages_reject_unknown_fields() {
        assert!(matches!(
            Control::decode(br#"{"type":"close","extra":1}"#),
            Err(FrameError::UnknownField(_))
        ));
        assert!(matches!(
            Control::decode(br#"{"type":"closed","extra":1}"#),
            Err(FrameError::UnknownField(_))
        ));
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let payload = vec![b' '; crate::MAX_FRAME_BYTES + 1];
        assert!(matches!(
            Control::decode(&payload),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn every_control_variant_roundtrips() {
        let variants = vec![
            hello(),
            Control::Ready(Ready {
                protocol: 1,
                build_commit: "abc".to_owned(),
                host_version: "0.1.0".to_owned(),
            }),
            Control::SessionStart(SessionStart {
                session_id: "s".to_owned(),
                mode: "full_duplex".to_owned(),
                lang: "zh".to_owned(),
                hot_words: vec!["车道".to_owned()],
                sample_rate: 16_000,
            }),
            Control::SessionEnd(SessionEnd {
                session_id: "s".to_owned(),
                reason: "client_close".to_owned(),
            }),
            Control::Partial(Partial {
                utterance_seq: 1,
                text: "嗯".to_owned(),
            }),
            Control::Final(Final {
                utterance_seq: 1,
                text: "嗯".to_owned(),
                start_ms: 5,
                end_ms: 10,
                finalize_ms: 20,
            }),
            Control::Cancel(Cancel {
                reason: "barge".to_owned(),
            }),
            Control::Heartbeat(Heartbeat {
                pid: 1,
                rss_kb: 2,
                model_state: "warm".to_owned(),
                queue_in: 0,
                queue_out: 0,
            }),
            Control::Error(ErrorMsg {
                code: "model_missing".to_owned(),
                stage: "arming".to_owned(),
                retryable: true,
                detail: Some(ErrorDetail {
                    family: Some("asr".to_owned()),
                    ..ErrorDetail::default()
                }),
            }),
            Control::Close,
            Control::Closed,
        ];
        for control in variants {
            let frame = control.encode().expect("编码应成功");
            assert_eq!(
                crate::decode_frame(&frame).expect("解码应成功"),
                crate::Frame::Control(control.clone()),
                "{} 往返失败",
                control.type_name()
            );
        }
    }
}
