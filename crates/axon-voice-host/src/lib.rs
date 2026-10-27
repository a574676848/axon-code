//! 语音宿主的控制面与父进程侧监督。
//!
//! 本 crate 同时提供二进制 `axon-voice-host` 与父进程使用的 [`supervisor`]。
//! Stage 2 只实现生命周期：握手、会话开关、严格序列、心跳、关闭与退出码；
//! 模型推理由后续阶段接入，未接入时 `warmup` 返回 `model_missing`，不是占位实现。

pub mod supervisor;

/// 构建号：由 `build.rs` 注入（环境变量或 git 短提交），本地无 git 时为 `dev`。
pub const BUILD_COMMIT: &str = env!("AXON_VOICE_BUILD_COMMIT");

/// 退出码。
pub mod exit_code {
    /// 正常关闭。
    pub const OK: i32 = 0;
    /// 握手版本或构建号不符。
    pub const HANDSHAKE: i32 = 10;
    /// 控制序列非法。
    pub const SEQUENCE: i32 = 11;
    /// 控制帧超限或控制队列溢出。
    pub const FRAME: i32 = 12;
    /// 模型目录不可读或清单校验失败。
    pub const MODEL: i32 = 13;
}
