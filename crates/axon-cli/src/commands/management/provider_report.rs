//! Provider 连通性探测报告。

use std::fmt::Write as _;
use std::time::Duration;

use axon_config::{ProviderConfig, provider_credential};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use serde::Serialize;

use crate::PROVIDER_TEST_TIMEOUT_MS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderTestReport {
    pub(crate) name: String,
    pub(crate) protocol: String,
    pub(crate) base_url: String,
    pub(crate) status: ProviderTestStatus,
    pub(crate) credential: ProviderCredentialStatus,
    pub(crate) http_status: Option<u16>,
    pub(crate) message: String,
}

impl ProviderTestReport {
    pub(crate) fn run(provider: &ProviderConfig) -> Self {
        let credential = provider_credential_status(provider);
        if credential == ProviderCredentialStatus::Missing {
            return Self {
                name: provider.name.clone(),
                protocol: provider.protocol.clone(),
                base_url: provider.base_url.clone(),
                status: ProviderTestStatus::Failed,
                credential,
                http_status: None,
                message: "API key 未配置或环境变量未设置".to_owned(),
            };
        }

        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        ensure_rustls_crypto_provider();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(PROVIDER_TEST_TIMEOUT_MS)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut request = agent.get(&url).header("User-Agent", "Axon");
        if let Some(api_key) = resolved_provider_api_key(provider) {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }

        match request.call() {
            Ok(response) if response.status().is_success() => Self {
                name: provider.name.clone(),
                protocol: provider.protocol.clone(),
                base_url: provider.base_url.clone(),
                status: ProviderTestStatus::Ok,
                credential,
                http_status: Some(response.status().as_u16()),
                message: "provider /models 探测通过".to_owned(),
            },
            Ok(response) => Self {
                name: provider.name.clone(),
                protocol: provider.protocol.clone(),
                base_url: provider.base_url.clone(),
                status: ProviderTestStatus::Failed,
                credential,
                http_status: Some(response.status().as_u16()),
                message: "provider /models 返回非成功状态".to_owned(),
            },
            Err(_) => Self {
                name: provider.name.clone(),
                protocol: provider.protocol.clone(),
                base_url: provider.base_url.clone(),
                status: ProviderTestStatus::Failed,
                credential,
                http_status: None,
                message: "provider /models 探测失败".to_owned(),
            },
        }
    }

    pub(crate) fn to_text(&self) -> String {
        let mut output = format!("provider {}: {}\n", self.name, self.status.as_str());
        let _ = writeln!(output, "- protocol: {}", self.protocol);
        let _ = writeln!(output, "- base_url: {}", self.base_url);
        let _ = writeln!(output, "- credential: {}", self.credential.as_str());
        if let Some(status) = self.http_status {
            let _ = writeln!(output, "- http_status: {status}");
        }
        let _ = writeln!(output, "- message: {}", self.message);
        output
    }
}

fn provider_credential_status(provider: &ProviderConfig) -> ProviderCredentialStatus {
    if provider
        .api_key
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return ProviderCredentialStatus::Present;
    }
    match provider.api_key_env.as_deref() {
        Some(env_name) if provider_credential(env_name).is_some() => {
            ProviderCredentialStatus::Present
        }
        Some(_) => ProviderCredentialStatus::Missing,
        None => ProviderCredentialStatus::NotRequired,
    }
}

fn resolved_provider_api_key(provider: &ProviderConfig) -> Option<String> {
    provider
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            provider
                .api_key_env
                .as_deref()
                .and_then(provider_credential)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderTestStatus {
    Ok,
    Failed,
}

impl ProviderTestStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderCredentialStatus {
    Present,
    Missing,
    NotRequired,
}

impl ProviderCredentialStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::NotRequired => "not_required",
        }
    }
}
