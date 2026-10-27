//! 父进程侧宿主监督：启动、握手、收发、心跳判定与关闭。

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use axon_voice_protocol::{
    Control, ErrorDetail, ErrorMsg, Frame, FrameError, Hello, PROTOCOL_VERSION, read_frame,
    write_frame,
};

/// 监督错误。
#[derive(Debug)]
pub enum SupervisorError {
    /// 进程无法启动。
    Spawn(std::io::Error),
    /// 握手超时或失败。
    Handshake(String),
    /// 写管道失败。
    Write(String),
    /// 读管道失败。
    Read(String),
    /// 等不到期望的帧。
    Timeout,
    /// 子进程已退出。
    Exited(ExitStatus),
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "启动宿主失败：{error}"),
            Self::Handshake(reason) => write!(f, "宿主握手失败：{reason}"),
            Self::Write(reason) => write!(f, "写宿主失败：{reason}"),
            Self::Read(reason) => write!(f, "读宿主失败：{reason}"),
            Self::Timeout => f.write_str("等待宿主超时"),
            Self::Exited(status) => write!(f, "宿主已退出：{status}"),
        }
    }
}

impl std::error::Error for SupervisorError {}

/// 监督配置。
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// 宿主可执行文件路径。
    pub exe: PathBuf,
    /// 期望的构建号。
    pub build_commit: String,
    /// 握手超时。
    pub handshake_timeout: Duration,
    /// 心跳判定超时。
    pub heartbeat_timeout: Duration,
    /// 自动重启上限。
    pub max_restart: u32,
    /// 追加给子进程的环境变量（测试与诊断用）。
    pub env: Vec<(String, String)>,
}

impl HostConfig {
    /// 以可执行文件构造配置，默认 5 s 握手、3 s 心跳、3 次重启。
    #[must_use]
    pub fn new(exe: impl Into<PathBuf>, build_commit: impl Into<String>) -> Self {
        Self {
            exe: exe.into(),
            build_commit: build_commit.into(),
            handshake_timeout: Duration::from_secs(5),
            heartbeat_timeout: Duration::from_secs(3),
            max_restart: 3,
            env: Vec::new(),
        }
    }
}

/// 已握手的宿主客户端。
pub struct HostClient {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Result<Frame, String>>,
    last_frame: Instant,
    heartbeat_timeout: Duration,
}

impl HostClient {
    /// 启动宿主并完成握手。
    ///
    /// # Errors
    ///
    /// 启动失败、握手超时、版本不符或进程提前退出。
    pub fn spawn(config: &HostConfig) -> Result<Self, SupervisorError> {
        let mut command = Command::new(&config.exe);
        command.env_clear();
        for key in ["PATH", "SYSTEMROOT", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, value) in &config.env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(SupervisorError::Spawn)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SupervisorError::Handshake("缺少 stdin".to_owned()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SupervisorError::Handshake("缺少 stdout".to_owned()))?;
        let (tx, frames) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_frame(&mut reader) {
                    Ok(Some(frame)) => {
                        if tx.send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = tx.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        });

        let mut client = Self {
            child,
            stdin,
            frames,
            last_frame: Instant::now(),
            heartbeat_timeout: config.heartbeat_timeout,
        };
        if let Err(error) = client.finish_handshake(config) {
            let _ = client.child.kill();
            let _ = client.child.wait();
            return Err(error);
        }
        Ok(client)
    }

    fn finish_handshake(&mut self, config: &HostConfig) -> Result<(), SupervisorError> {
        self.send(&hello(&config.build_commit))?;
        match self.recv_timeout(config.handshake_timeout)? {
            Frame::Control(Control::Ready(ready)) => {
                if ready.protocol != PROTOCOL_VERSION {
                    return Err(SupervisorError::Handshake("协议版本不符".to_owned()));
                }
                if ready.build_commit != config.build_commit {
                    return Err(SupervisorError::Handshake("构建号不符".to_owned()));
                }
                Ok(())
            }
            other => Err(SupervisorError::Handshake(format!(
                "期望 ready，得到 {other:?}"
            ))),
        }
    }

    /// 发送控制消息。
    ///
    /// # Errors
    ///
    /// 写管道失败。
    pub fn send(&mut self, control: &Control) -> Result<(), SupervisorError> {
        write_frame(&mut self.stdin, control)
            .map_err(|error: FrameError| SupervisorError::Write(error.to_string()))
    }

    /// 在超时内读取一帧。
    ///
    /// # Errors
    ///
    /// 超时、通道关闭或协议错误。
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Frame, SupervisorError> {
        match self.frames.recv_timeout(timeout) {
            Ok(Ok(frame)) => {
                self.last_frame = Instant::now();
                Ok(frame)
            }
            Ok(Err(reason)) => Err(SupervisorError::Read(reason)),
            Err(RecvTimeoutError::Timeout) => Err(SupervisorError::Timeout),
            Err(RecvTimeoutError::Disconnected) => {
                Err(SupervisorError::Read("通道关闭".to_owned()))
            }
        }
    }

    /// 心跳是否在容忍窗口内。
    #[must_use]
    pub fn heartbeat_ok(&self, now: Instant) -> bool {
        now.duration_since(self.last_frame) <= self.heartbeat_timeout
    }

    /// 子进程是否已退出。
    ///
    /// # Errors
    ///
    /// 查询失败。
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, SupervisorError> {
        self.child.try_wait().map_err(SupervisorError::Spawn)
    }

    /// 请求关闭并等待进程退出；超时则强杀。
    ///
    /// # Errors
    ///
    /// 写失败或等待失败。
    pub fn close(mut self) -> Result<ExitStatus, SupervisorError> {
        let _ = self.send(&Control::Close);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut acknowledged = false;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self.frames.recv_timeout(remaining) {
                Ok(Ok(Frame::Control(Control::Closed))) => {
                    acknowledged = true;
                    break;
                }
                // 心跳等无关帧跳过，直到关闭回执或超时。
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => break,
            }
        }
        if !acknowledged {
            let _ = self.child.kill();
        }
        drop(self.stdin);
        self.child.wait().map_err(SupervisorError::Spawn)
    }

    /// 进程是否仍在运行。
    ///
    /// # Errors
    ///
    /// 查询失败。
    pub fn alive(&mut self) -> Result<bool, SupervisorError> {
        Ok(self.try_wait()?.is_none())
    }

    /// 终止并回收；返回退出码。
    pub fn terminate(&mut self) -> Option<i32> {
        let _ = self.child.kill();
        self.child.wait().ok().and_then(|status| status.code())
    }

    /// 强制结束。
    ///
    /// # Errors
    ///
    /// 结束或回收失败。
    pub fn kill(mut self) -> Result<ExitStatus, SupervisorError> {
        self.child.kill().map_err(SupervisorError::Spawn)?;
        self.child.wait().map_err(SupervisorError::Spawn)
    }
}

/// 监督动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorAction {
    /// 继续运行。
    Continue,
    /// 需要重启。
    Restart,
    /// 超过上限，放弃。
    GiveUp,
}

/// 宿主失败信息，供上层映射为 `worker_crashed`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostFailure {
    /// 子阶段。
    pub sub_stage: &'static str,
    /// 退出码；被强杀时为 None。
    pub exit_code: Option<i32>,
}

/// 重启计数与策略。
#[derive(Debug)]
pub struct Supervisor {
    config: HostConfig,
    restarts: u32,
}

impl Supervisor {
    /// 新建监督器。
    #[must_use]
    pub fn new(config: HostConfig) -> Self {
        Self {
            config,
            restarts: 0,
        }
    }

    /// 已重启次数。
    #[must_use]
    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    /// 崩溃后是否允许重启；允许时计数加一。
    pub fn restart_allowed(&mut self) -> bool {
        if self.restarts >= self.config.max_restart {
            return false;
        }
        self.restarts += 1;
        true
    }

    /// 启动一次宿主。
    ///
    /// # Errors
    ///
    /// 见 [`HostClient::spawn`]。
    pub fn spawn(&self) -> Result<HostClient, SupervisorError> {
        HostClient::spawn(&self.config)
    }

    /// 心跳判定：超时则按重启上限给出动作。
    pub fn tick(&mut self, heartbeat_ok: bool) -> SupervisorAction {
        if heartbeat_ok {
            return SupervisorAction::Continue;
        }
        if self.restart_allowed() {
            SupervisorAction::Restart
        } else {
            SupervisorAction::GiveUp
        }
    }

    /// 进程退出后的动作。
    pub fn after_exit(
        &mut self,
        exit_code: Option<i32>,
    ) -> (SupervisorAction, Option<HostFailure>) {
        let failure = HostFailure {
            sub_stage: "warmup",
            exit_code,
        };
        if self.restart_allowed() {
            (SupervisorAction::Restart, Some(failure))
        } else {
            (SupervisorAction::GiveUp, Some(failure))
        }
    }

    /// 心跳判定并按需终止僵死进程。
    ///
    /// 返回动作与失败信息；失败信息由调用方映射为 `worker_crashed{subStage,exitCode}`。
    pub fn guard(
        &mut self,
        client: &mut HostClient,
        now: Instant,
    ) -> (SupervisorAction, Option<HostFailure>) {
        if client.heartbeat_ok(now) {
            return (SupervisorAction::Continue, None);
        }
        // 已经退出的进程不再消耗重启额度。
        if !client.alive().unwrap_or(false) {
            return (SupervisorAction::Continue, None);
        }
        let exit_code = client.terminate();
        let failure = HostFailure {
            sub_stage: "io",
            exit_code,
        };
        let action = if self.restart_allowed() {
            SupervisorAction::Restart
        } else {
            SupervisorAction::GiveUp
        };
        (action, Some(failure))
    }

    /// 配置引用。
    #[must_use]
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// 可执行文件是否存在。
    #[must_use]
    pub fn exe_exists(&self) -> bool {
        Path::new(&self.config.exe).is_file()
    }
}

fn hello(build_commit: &str) -> Control {
    Control::Hello(Hello {
        protocol: PROTOCOL_VERSION,
        build_commit: build_commit.to_owned(),
        role: "daemon".to_owned(),
        pid: std::process::id(),
    })
}

/// 把宿主失败映射为协议错误帧，供 daemon 直接下发。
#[must_use]
pub fn worker_crashed(failure: &HostFailure) -> Control {
    Control::Error(ErrorMsg {
        code: "worker_crashed".to_owned(),
        stage: "worker".to_owned(),
        retryable: true,
        detail: Some(ErrorDetail {
            exit_code: failure.exit_code,
            reason: Some(failure.sub_stage.to_owned()),
            ..ErrorDetail::default()
        }),
    })
}
#[cfg(test)]
mod tests {
    use super::{HostConfig, Supervisor, SupervisorAction};
    use axon_voice_protocol::{Control, ErrorMsg};

    #[test]
    fn restart_limit_is_enforced() {
        let mut supervisor = Supervisor::new(HostConfig::new("missing", "dev"));
        assert_eq!(supervisor.restarts(), 0);
        assert!(supervisor.restart_allowed());
        assert!(supervisor.restart_allowed());
        assert!(supervisor.restart_allowed());
        assert!(!supervisor.restart_allowed());
        assert_eq!(supervisor.restarts(), 3);
    }

    #[test]
    fn tick_reports_restart_then_give_up() {
        let mut supervisor = Supervisor::new(HostConfig::new("missing", "dev"));
        assert_eq!(supervisor.tick(true), SupervisorAction::Continue);
        assert_eq!(supervisor.tick(false), SupervisorAction::Restart);
        assert_eq!(supervisor.tick(false), SupervisorAction::Restart);
        assert_eq!(supervisor.tick(false), SupervisorAction::Restart);
        assert_eq!(supervisor.tick(false), SupervisorAction::GiveUp);
        assert_eq!(supervisor.after_exit(Some(1)).0, SupervisorAction::GiveUp);
    }

    #[test]
    fn crash_maps_to_worker_crashed_error() {
        let failure = super::HostFailure {
            sub_stage: "asr",
            exit_code: Some(13),
        };
        match super::worker_crashed(&failure) {
            Control::Error(ErrorMsg {
                code,
                stage,
                detail,
                ..
            }) => {
                assert_eq!(code, "worker_crashed");
                assert_eq!(stage, "worker");
                let detail = detail.expect("应有 detail");
                assert_eq!(detail.exit_code, Some(13));
                assert_eq!(detail.reason.as_deref(), Some("asr"));
            }
            other => panic!("期望错误帧，得到 {other:?}"),
        }
    }

    #[test]
    fn missing_exe_is_reported() {
        let supervisor = Supervisor::new(HostConfig::new("definitely-missing", "dev"));
        assert!(!supervisor.exe_exists());
    }
}
