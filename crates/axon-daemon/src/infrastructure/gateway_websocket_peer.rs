//! 连接级错误的严重度判定。
//!
//! 桌面端刷新、进程退出、客户端崩溃与端口探测都会让 TCP 直接断开，tungstenite 把
//! 这些统一报成协议重置或 IO 错误。它们都是连接的正常终点：按 error 记录只会把真正
//! 的服务端故障埋进噪声里，运维再也分不出哪一条值得追。

use tokio_tungstenite::tungstenite::Error;
use tokio_tungstenite::tungstenite::error::ProtocolError;

/// 判断错误是否只表示「对端已经走了」。
///
/// 覆盖三类：对端没走关闭握手就断开、握手没走完就放弃、IO 层的连接已被重置或管道已断。
/// 其余错误（帧太大、写缓冲满、TLS、HTTP 头非法等）都留在 error，那里才是需要追的故障。
pub(super) fn is_peer_disconnect(error: &Error) -> bool {
    match error {
        Error::ConnectionClosed | Error::AlreadyClosed => true,
        Error::Protocol(
            ProtocolError::ResetWithoutClosingHandshake | ProtocolError::HandshakeIncomplete,
        ) => true,
        Error::Io(io) => matches!(
            io.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn client_gone_errors_are_not_server_faults() {
        assert!(is_peer_disconnect(&Error::Protocol(
            ProtocolError::ResetWithoutClosingHandshake
        )));
        assert!(is_peer_disconnect(&Error::Protocol(
            ProtocolError::HandshakeIncomplete
        )));
        // Windows 上客户端进程被杀就是 os error 10054，映射到 ConnectionReset。
        assert!(is_peer_disconnect(&Error::Io(io::Error::from(
            io::ErrorKind::ConnectionReset
        ))));
        assert!(is_peer_disconnect(&Error::Io(io::Error::from(
            io::ErrorKind::BrokenPipe
        ))));
        assert!(is_peer_disconnect(&Error::ConnectionClosed));
    }

    #[test]
    fn server_side_faults_stay_errors() {
        assert!(!is_peer_disconnect(&Error::Protocol(
            ProtocolError::NonZeroReservedBits
        )));
        assert!(!is_peer_disconnect(&Error::Protocol(
            ProtocolError::UnmaskedFrameFromClient
        )));
        assert!(!is_peer_disconnect(&Error::Io(io::Error::from(
            io::ErrorKind::PermissionDenied
        ))));
    }
}
