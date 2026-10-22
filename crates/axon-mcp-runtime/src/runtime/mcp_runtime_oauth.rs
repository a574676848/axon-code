//! MCP runtime OAuth 凭据装配。
//!
//! OAuth 登录写入的 access token 必须注入 Streamable HTTP 配置，才能同时覆盖
//! tools/list 装配、tools/call 执行与 prompts/resources 调用；否则授权成功后 server
//! 仍会返回 401，TUI 与 CLI 会持续显示未授权。凭据键与登录入口保持一致，统一使用
//! 规范化后的 server ID。

use std::path::Path;

use axon_agent_runtime::{McpAuthRuntime, McpOAuthStatusRequest, McpServerConfig};

/// 为 Streamable HTTP server 注入运行态 OAuth access token。
///
/// 显式配置 `bearerTokenEnvVar` 的 server 保持原样；token 缺失、读取失败或刷新失败时
/// 返回原配置，由 transport 层继续给出 401/403 授权引导。
pub(super) fn config_with_runtime_oauth(
    root: &Path,
    server_id: &str,
    config: McpServerConfig,
) -> McpServerConfig {
    match config {
        McpServerConfig::StreamableHttp(mut http) if http.bearer_token_env_var.is_none() => {
            if let Ok(Some(token)) =
                McpAuthRuntime::oauth_token_for_request(McpOAuthStatusRequest {
                    root: root.to_path_buf(),
                    server_name: server_id.to_owned(),
                    url: http.url.clone(),
                })
            {
                http.oauth_access_token = Some(token.access_token);
            }
            McpServerConfig::StreamableHttp(http)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use axon_agent_runtime::{
        McpAuthRuntime, McpHttpServerConfig, McpOAuthStatusRequest, McpServerCommand,
    };
    use axon_crypto::{Sha256Hex, sha256_hex_prefix};

    use super::*;

    const OAUTH_STORE_REF_LEN: usize = 32;
    const OAUTH_STORE_DIR: &str = "mcp-oauth";
    const OAUTH_TOKEN_DIR: &str = "tokens";
    const TEST_SERVER_ID: &str = "sm_cloud_platform";
    const TEST_URL: &str = "https://sm-cloud.exexm.com/mcp/v1/platform";
    const TEST_ACCESS_TOKEN: &str = "test-access-token";
    const TEST_BEARER_ENV_VAR: &str = "AXON_TEST_MCP_BEARER_TOKEN";

    #[test]
    fn http_config_uses_stored_oauth_token() {
        let root = test_root("oauth-token");
        write_stored_oauth_token(&root, TEST_SERVER_ID, TEST_URL, TEST_ACCESS_TOKEN);

        let config = config_with_runtime_oauth(
            &root,
            TEST_SERVER_ID,
            McpServerConfig::StreamableHttp(http_config(None)),
        );

        let McpServerConfig::StreamableHttp(http) = config else {
            panic!("Streamable HTTP 配置应保持原 transport");
        };
        assert_eq!(http.oauth_access_token.as_deref(), Some(TEST_ACCESS_TOKEN));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn http_config_without_credential_keeps_token_empty() {
        let root = test_root("oauth-missing");

        let config = config_with_runtime_oauth(
            &root,
            TEST_SERVER_ID,
            McpServerConfig::StreamableHttp(http_config(None)),
        );

        let McpServerConfig::StreamableHttp(http) = config else {
            panic!("Streamable HTTP 配置应保持原 transport");
        };
        assert_eq!(http.oauth_access_token, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn http_config_keeps_bearer_env_precedence() {
        let root = test_root("oauth-env");
        write_stored_oauth_token(&root, TEST_SERVER_ID, TEST_URL, TEST_ACCESS_TOKEN);

        let config = config_with_runtime_oauth(
            &root,
            TEST_SERVER_ID,
            McpServerConfig::StreamableHttp(http_config(Some(TEST_BEARER_ENV_VAR))),
        );

        let McpServerConfig::StreamableHttp(http) = config else {
            panic!("Streamable HTTP 配置应保持原 transport");
        };
        assert_eq!(http.oauth_access_token, None);
        assert_eq!(
            http.bearer_token_env_var.as_deref(),
            Some(TEST_BEARER_ENV_VAR)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stdio_config_is_returned_unchanged() {
        let config = McpServerConfig::Stdio(McpServerCommand {
            program: "axon-test-mcp".to_owned(),
            args: Vec::new(),
            working_dir: None,
            env: BTreeMap::new(),
            tool_timeout_ms: None,
        });

        let result = config_with_runtime_oauth(&test_root("oauth-stdio"), TEST_SERVER_ID, config);

        assert!(matches!(result, McpServerConfig::Stdio(_)));
    }

    fn http_config(bearer_token_env_var: Option<&str>) -> McpHttpServerConfig {
        McpHttpServerConfig {
            url: TEST_URL.to_owned(),
            headers: Vec::new(),
            bearer_token_env_var: bearer_token_env_var.map(ToOwned::to_owned),
            oauth_access_token: None,
            tool_timeout_ms: None,
        }
    }

    /// 写入与 credential store 同构的 token 文件。
    ///
    /// store 的凭据文件键由「server ID + NUL + endpoint URL」双重 SHA-256 取前 32 位十六进制
    /// 生成，这里按同一算法生成，并通过 `oauth_status` 复核，避免测试与实现悄悄漂移。
    fn write_stored_oauth_token(root: &Path, server_id: &str, url: &str, access_token: &str) {
        let mut hasher = Sha256Hex::new();
        hasher.update(server_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(url.as_bytes());
        let credential_ref = sha256_hex_prefix(hasher.finalize(), OAUTH_STORE_REF_LEN);
        let token_dir = root.join(OAUTH_STORE_DIR).join(OAUTH_TOKEN_DIR);
        fs::create_dir_all(&token_dir).expect("OAuth 凭据目录应可创建");
        let payload = serde_json::json!({
            "server_name": server_id,
            "url": url,
            "access_token": access_token,
        });
        fs::write(
            token_dir.join(format!("{credential_ref}.json")),
            payload.to_string(),
        )
        .expect("OAuth 凭据应可写入");

        let status = McpAuthRuntime::oauth_status(McpOAuthStatusRequest {
            root: root.to_path_buf(),
            server_name: server_id.to_owned(),
            url: url.to_owned(),
        })
        .expect("OAuth 凭据状态应可读取");
        assert!(
            status.0,
            "测试凭据应能被 credential store 识别：{credential_ref}"
        );
    }

    fn test_root(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root().join(format!(
            "axon-mcp-runtime-oauth-{name}-{}-{nonce}",
            std::process::id()
        ))
    }
}
