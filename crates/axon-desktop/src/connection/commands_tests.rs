//! 保存闸门的单元测试。
//!
//! 闸门是「能不能落云档案」的唯一判据，必须能在不联网的前提下逐档验证：HTTP 面与
//! 事件流面任意一面没拿到肯定结论，都不允许写档案。

use super::{http_gate, save_gate, ws_gate};
use crate::connection::probe::{
    MetaPayload, ProbeFailure, ProbeOutcome, ProbeReady, WsProbeOutcome,
};
use crate::connection::profile::CapabilitySet;

/// 构造一份 Ready 的探活结论。
fn ready_outcome() -> ProbeOutcome {
    ProbeOutcome::Ready(Box::new(ProbeReady {
        meta: MetaPayload {
            protocol_version: 1,
            capabilities: CapabilitySet::default(),
            server_time_ms: 0,
            upload_limit_bytes: 64 * 1024 * 1024,
            super_admin: false,
        },
        latency_ms: 3,
        clock_offset_ms: 0,
        clock_verified: true,
        build_version: Some("test".to_owned()),
    }))
}

#[test]
fn ready_http_and_ready_websocket_pass_the_gate() {
    assert!(http_gate(&ready_outcome()).is_ok());
    assert!(ws_gate(&WsProbeOutcome::Ready).is_ok());
    assert!(save_gate(&ready_outcome(), &WsProbeOutcome::Ready).is_ok());
}

#[test]
fn outdated_backend_is_rejected_because_meta_is_missing() {
    let error = http_gate(&ProbeOutcome::Outdated).expect_err("老后端不得通过闸门");
    assert!(
        error.contains("版本过旧") && error.contains("/api/v1/meta"),
        "拒绝理由必须指向缺失的 /meta，实际：{error}"
    );
    assert!(
        save_gate(&ProbeOutcome::Outdated, &WsProbeOutcome::Ready).is_err(),
        "事件流端口可用也不能掩盖 HTTP 面缺 /meta"
    );
}

#[test]
fn failed_probe_is_rejected_with_its_own_message() {
    for failure in [
        ProbeFailure::CredentialRejected { status: 401 },
        ProbeFailure::CertificateInvalid("自签证书".to_owned()),
        ProbeFailure::Unreachable("连接被拒绝".to_owned()),
        ProbeFailure::ProtocolUnsupported(9),
    ] {
        let expected = failure.message();
        let error = http_gate(&ProbeOutcome::Failed(failure)).expect_err("失败的探活不得通过闸门");
        assert_eq!(
            error, expected,
            "闸门必须原样透出探活的分类文案，不能自己再编一句"
        );
    }
}

#[test]
fn every_non_ready_websocket_conclusion_blocks_saving() {
    let cases = [
        WsProbeOutcome::WrongPort,
        WsProbeOutcome::CredentialRejected { status: 401 },
        WsProbeOutcome::Unexpected { status: 200 },
        WsProbeOutcome::Unreachable("连接被拒绝".to_owned()),
    ];
    for ws in cases {
        let error = ws_gate(&ws).expect_err("事件流面没确认可用时不得保存");
        assert!(!error.is_empty(), "拒绝理由不得为空：{ws:?}");
        assert!(
            save_gate(&ready_outcome(), &ws).is_err(),
            "HTTP 面可用也不能掩盖事件流面未确认：{ws:?}"
        );
    }
}

#[test]
fn unexpected_websocket_status_names_the_status_code() {
    let error = ws_gate(&WsProbeOutcome::Unexpected { status: 502 }).expect_err("必须拒绝");
    assert!(
        error.contains("502"),
        "拒绝理由必须带上真实状态码，实际：{error}"
    );
}
