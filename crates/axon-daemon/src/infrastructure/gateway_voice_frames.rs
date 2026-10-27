//! `/api/v1/voice` 的客户端帧：类型、严格解析与会话态分类。
//!
//! 未知字段、未知类型与非法取值一律拒绝（PROTOCOL §2.1）；这里不碰 IO 与宿主。

use serde::Deserialize;

/// 协议版本。
pub(super) const PROTOCOL_VERSION: u8 = 1;

/// `start` 帧里会话循环需要的字段（模式与语言已在校验阶段限定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StartRequest {
    /// 客户端会话 id。
    pub(super) client_session_id: String,
    /// 工作区 id。
    pub(super) workspace_id: String,
    /// 回复目的地，与文本入口同车道键。
    pub(super) reply_destination: String,
}

/// 取消原因，客户端只允许这两个。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelReason {
    /// 用户取消。
    UserCancel,
    /// 客户端关闭。
    ClientClose,
}

/// 客户端帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientFrame {
    /// 开始会话。
    Start {
        /// 客户端会话 id。
        client_session_id: String,
        /// 工作区 id。
        workspace_id: String,
        /// 回复目的地。
        reply_destination: String,
        /// 是否朗读文本轮。
        tts: bool,
    },
    /// 结束当前句。
    Stop {
        /// 句序号。
        utterance_seq: u32,
    },
    /// 取消。
    Cancel {
        /// 原因。
        reason: CancelReason,
    },
    /// 已识别但当前不处理的合法帧。
    Ignored,
}

/// `start` 帧。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartWire {
    protocol_version: u8,
    client_session_id: String,
    workspace_id: String,
    reply_destination: String,
    mode: String,
    lang: String,
    tts: bool,
    #[serde(default)]
    device_id: Option<String>,
}

/// `stop` 帧。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StopWire {
    utterance_seq: u32,
}

/// `cancel` 帧。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelWire {
    reason: String,
}

/// 文本帧分类结果。
pub(crate) enum Classified {
    /// 开始会话。
    Start(StartRequest),
    /// 结束当前句。
    Stop(u32),
    /// 取消。
    Cancel(CancelReason),
    /// 已知但暂不处理。
    Ignored,
}

/// 按会话态分类：`start` 只能一次，`stop`/`cancel` 必须在 `start` 之后。
pub(crate) fn classify(started: bool, text: &str) -> Result<Classified, ()> {
    match parse_client_frame(text)? {
        ClientFrame::Start {
            client_session_id,
            workspace_id,
            reply_destination,
            ..
        } => {
            if started {
                Err(())
            } else {
                Ok(Classified::Start(StartRequest {
                    client_session_id,
                    workspace_id,
                    reply_destination,
                }))
            }
        }
        ClientFrame::Stop { utterance_seq } => {
            if started {
                Ok(Classified::Stop(utterance_seq))
            } else {
                Err(())
            }
        }
        ClientFrame::Cancel { reason } => {
            if started {
                Ok(Classified::Cancel(reason))
            } else {
                Err(())
            }
        }
        ClientFrame::Ignored => Ok(Classified::Ignored),
    }
}

/// 解析客户端控制帧；未知字段、未知类型与非法取值都拒绝。
pub(crate) fn parse_client_frame(text: &str) -> Result<ClientFrame, ()> {
    let mut value: serde_json::Value = serde_json::from_str(text).map_err(|_| ())?;
    let kind = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .ok_or(())?;
    let object = value.as_object_mut().ok_or(())?;
    object.remove("type");
    match kind.as_str() {
        "start" => {
            let wire: StartWire = serde_json::from_value(value).map_err(|_| ())?;
            if wire.protocol_version != PROTOCOL_VERSION
                || wire.client_session_id.is_empty()
                || wire.client_session_id.len() > 64
                || wire.workspace_id.is_empty()
                || wire.reply_destination.is_empty()
                || !matches!(wire.mode.as_str(), "push_to_talk" | "full_duplex")
                || !matches!(wire.lang.as_str(), "zh" | "en" | "auto")
            {
                return Err(());
            }
            let _ = wire.device_id;
            Ok(ClientFrame::Start {
                client_session_id: wire.client_session_id,
                workspace_id: wire.workspace_id,
                reply_destination: wire.reply_destination,
                tts: wire.tts,
            })
        }
        "stop" => {
            let wire: StopWire = serde_json::from_value(value).map_err(|_| ())?;
            Ok(ClientFrame::Stop {
                utterance_seq: wire.utterance_seq,
            })
        }
        "cancel" => {
            let wire: CancelWire = serde_json::from_value(value).map_err(|_| ())?;
            let reason = match wire.reason.as_str() {
                "user_cancel" => CancelReason::UserCancel,
                "client_close" => CancelReason::ClientClose,
                _ => return Err(()),
            };
            Ok(ClientFrame::Cancel { reason })
        }
        "setTts" | "setPreset" | "cancelPreview" | "cueEnded" | "cueAck" | "preview"
        | "playbackDrained" => Ok(ClientFrame::Ignored),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CancelReason, Classified, ClientFrame, classify, parse_client_frame};

    fn start_json() -> &'static str {
        r#"{"type":"start","protocolVersion":1,"clientSessionId":"c-1","workspaceId":"w-1","replyDestination":"desktop","mode":"full_duplex","lang":"zh","tts":false}"#
    }

    #[test]
    fn start_frame_is_accepted() {
        match parse_client_frame(start_json()) {
            Ok(ClientFrame::Start {
                client_session_id, ..
            }) => assert_eq!(client_session_id, "c-1"),
            other => panic!("期望 start，得到 {other:?}"),
        }
    }

    #[test]
    fn start_rejects_unknown_field() {
        let json = start_json().replace("\"tts\":false", "\"tts\":false,\"extra\":1");
        assert_eq!(parse_client_frame(&json), Err(()));
    }

    #[test]
    fn start_rejects_bad_version_and_mode() {
        assert_eq!(
            parse_client_frame(
                &start_json().replace("\"protocolVersion\":1", "\"protocolVersion\":2")
            ),
            Err(())
        );
        assert_eq!(
            parse_client_frame(&start_json().replace("\"full_duplex\"", "\"half_duplex\"")),
            Err(())
        );
    }

    #[test]
    fn cancel_reason_matches_contract() {
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"user_cancel"}"#),
            Ok(ClientFrame::Cancel {
                reason: CancelReason::UserCancel
            })
        );
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"client_close"}"#),
            Ok(ClientFrame::Cancel {
                reason: CancelReason::ClientClose
            })
        );
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"barge"}"#),
            Err(())
        );
    }

    #[test]
    fn stop_requires_sequence() {
        assert_eq!(
            parse_client_frame(r#"{"type":"stop","utteranceSeq":3}"#),
            Ok(ClientFrame::Stop { utterance_seq: 3 })
        );
        assert_eq!(parse_client_frame(r#"{"type":"stop"}"#), Err(()));
    }

    #[test]
    fn classify_enforces_session_state() {
        assert!(matches!(
            classify(false, start_json()),
            Ok(Classified::Start(_))
        ));
        assert!(classify(true, start_json()).is_err(), "重复 start 应拒绝");
        assert!(classify(false, r#"{"type":"stop","utteranceSeq":1}"#).is_err());
        assert!(
            classify(false, r#"{"type":"cancel","reason":"user_cancel"}"#).is_err(),
            "未 start 不能 cancel"
        );
        assert!(matches!(
            classify(true, r#"{"type":"stop","utteranceSeq":1}"#),
            Ok(Classified::Stop(1))
        ));
        assert!(matches!(
            classify(true, r#"{"type":"cancel","reason":"user_cancel"}"#),
            Ok(Classified::Cancel(CancelReason::UserCancel))
        ));
        assert!(matches!(
            classify(true, r#"{"type":"cancel","reason":"client_close"}"#),
            Ok(Classified::Cancel(CancelReason::ClientClose))
        ));
        assert!(matches!(
            classify(true, r#"{"type":"setTts","enabled":true}"#),
            Ok(Classified::Ignored)
        ));
    }
}
