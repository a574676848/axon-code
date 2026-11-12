//! 本机资源感知（M4）。
//!
//! 只做三件事：
//! 1. 扫描 `~/.ssh/config`、`~/.ssh/*.pub` 配对私钥名、`~/.ssh/known_hosts` 主机名与 `~/.gitconfig` 身份；
//! 2. 把身份信息脱敏（email → `首字母***@域名`）；
//! 3. 仅在命中触发词且开关打开时渲染注入块。
//!
//! 硬约束：不读私钥内容、不读 known_hosts 指纹、不读 shell history 与浏览器数据、
//! 输出不含绝对路径、不含完整 email、不含任何凭据。

mod inject;
mod local_resources;
mod redact;

pub use inject::{
    ENV_BLOCK_MAX_BYTES, ENV_TRIGGER_WORDS, MEMORY_ENV_ENABLED_DEFAULT, maybe_inject,
    render_block_if_enabled, should_inject,
};
pub use local_resources::{
    GitIdentity, LocalResources, MAX_KNOWN_HOSTS, MAX_SSH_HOSTS, SshHost, scan,
};
pub use redact::redact_email;
