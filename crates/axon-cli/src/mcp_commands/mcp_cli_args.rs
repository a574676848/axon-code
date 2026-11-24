use std::path::PathBuf;

use clap::{Args, Subcommand};

/// `axon mcp` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct McpCommand {
    /// MCP 子命令。
    #[command(subcommand)]
    pub command: Option<McpSubcommand>,
}

/// `axon mcp` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum McpSubcommand {
    /// 列出本机已配置的 MCP server。
    List(McpListCommand),
    /// 查看单个 MCP server 配置。
    Get(McpGetCommand),
    /// 添加本机 MCP server 配置。
    Add(McpAddCommand),
    /// 移除本机 MCP server 配置。
    Remove(McpRemoveCommand),
    /// 为 Streamable HTTP MCP server 执行 OAuth 登录。
    Login(McpLoginCommand),
    /// 删除 Streamable HTTP MCP server 的本地 OAuth 凭据。
    Logout(McpLogoutCommand),
}

/// `axon mcp list` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct McpListCommand {
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
}

/// `axon mcp get` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct McpGetCommand {
    /// MCP server ID。
    pub server_id: String,
}

/// `axon mcp add` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(override_usage = "axon mcp add [OPTIONS] <NAME> (--url <URL> | -- <COMMAND>...)")]
pub struct McpAddCommand {
    /// MCP server ID。
    pub server_id: String,
    /// Streamable HTTP MCP endpoint URL。
    #[arg(long)]
    pub url: Option<String>,
    /// Bearer token 环境变量名；仅保存变量名，不保存 token 值。
    #[arg(long = "bearer-token-env-var", requires = "url")]
    pub bearer_token_env_var: Option<String>,
    /// MCP stdio server 工作目录。
    #[arg(long)]
    pub working_dir: Option<PathBuf>,
    /// MCP stdio server 启动命令与参数；使用 `--` 后传入。
    #[arg(trailing_var_arg = true, num_args = 0..)]
    pub command: Vec<String>,
}

/// `axon mcp login` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct McpLoginCommand {
    /// MCP server ID。
    pub server_id: String,
    /// 显式 OAuth scopes；可重复传入。
    #[arg(long = "scope")]
    pub scopes: Vec<String>,
    /// 可选 OAuth client_id。
    #[arg(long)]
    pub client_id: Option<String>,
    /// OAuth resource indicator；为空时使用 MCP endpoint URL。
    #[arg(long)]
    pub resource: Option<String>,
    /// 启动本地 callback 并等待浏览器授权完成。
    #[arg(long)]
    pub browser: bool,
}

/// `axon mcp logout` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct McpLogoutCommand {
    /// MCP server ID。
    pub server_id: String,
}

/// `axon mcp remove` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct McpRemoveCommand {
    /// MCP server ID。
    pub server_id: String,
}
