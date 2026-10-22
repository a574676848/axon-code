//! Models.dev 模型费率同步客户端。
//!
//! 这一层只负责「拉取、限流、落库」；目录长什么样、哪些条目该收，
//! 由 [`super::pricing_catalog`] 决定。

use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::Serialize;

use super::pricing_catalog::{
    local_provider_names, parse_models_dev_catalog, parse_models_dev_json,
};
use super::repository::UsageRepository;

/// 手动同步防抖时间间隔（60 秒）。
pub const MANUAL_DEBOUNCE_INTERVAL_MS: u64 = 60_000;
/// 自动同步最小时间间隔（每天最多 1 次，24 小时）。
pub const AUTO_SYNC_INTERVAL_MS: u64 = 86_400_000;
/// 同步 HTTP 超时。
///
/// 线上目录是几兆的 JSON（实测约 6 MB），10 秒的预算在慢链路上必然踩线，
/// 而超时会被读成「上游不可达」。这里按「允许一次完整下载」给预算。
pub const SYNC_HTTP_TIMEOUT: Duration = Duration::from_secs(90);
/// Models.dev 默认 API 端点。
pub const DEFAULT_MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// 同步执行结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncOutcome {
    /// 成功同步并写入规则。
    Success {
        /// 成功保存的规则数量。
        rules_saved: usize,
    },
    /// 手动同步防抖跳过。
    Debounced {
        /// 剩余防抖毫秒数。
        remaining_ms: u64,
    },
    /// 自动同步限流跳过（24小时内已同步）。
    RateLimited,
    /// 同步失败但已安全回退到本地规则快照（绝不阻断）。
    FallbackToExisting {
        /// 失败错误说明。
        error: String,
    },
}
#[async_trait::async_trait]
pub trait SyncHttpTransport: Send + Sync {
    async fn fetch_json(&self, url: &str, timeout: Duration) -> Result<String, String>;
}

/// 生产环境 HTTP 传输实现（使用 ureq 在阻塞线程执行）。
pub struct DefaultSyncHttpTransport;

#[async_trait::async_trait]
impl SyncHttpTransport for DefaultSyncHttpTransport {
    async fn fetch_json(&self, url: &str, timeout: Duration) -> Result<String, String> {
        let url_owned = url.to_owned();
        tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(timeout))
                .build()
                .into();
            agent
                .get(&url_owned)
                .call()
                .map_err(|e| format!("HTTP 请求失败: {e}"))?
                .body_mut()
                .read_to_string()
                .map_err(|e| format!("读取 HTTP 响应失败: {e}"))
        })
        .await
        .map_err(|e| format!("同步任务调度失败: {e}"))?
    }
}

/// 模型价格同步客户端服务。
pub struct PricingSyncService {
    transport: Box<dyn SyncHttpTransport>,
    last_attempt_at_ms: AtomicU64,
    last_success_at_ms: AtomicU64,
    api_url: String,
}

impl PricingSyncService {
    /// 使用默认生产 HTTP 传输创建同步服务。
    #[must_use]
    pub fn new() -> Self {
        Self {
            transport: Box::new(DefaultSyncHttpTransport),
            last_attempt_at_ms: AtomicU64::new(0),
            last_success_at_ms: AtomicU64::new(0),
            api_url: DEFAULT_MODELS_DEV_URL.to_owned(),
        }
    }

    /// 使用自定义传输创建同步服务（专用于单元测试）。
    #[must_use]
    pub fn with_transport(transport: Box<dyn SyncHttpTransport>, last_success_at_ms: u64) -> Self {
        Self {
            transport,
            last_attempt_at_ms: AtomicU64::new(0),
            last_success_at_ms: AtomicU64::new(last_success_at_ms),
            api_url: DEFAULT_MODELS_DEV_URL.to_owned(),
        }
    }

    /// 执行规则同步。
    ///
    /// - `manual: true`：手动触发，受 60s 防抖保护；
    /// - `manual: false`：自动同步，每天最多 1 次；
    /// - 失败时回退并保留本地规则快照，绝不阻断。
    pub async fn sync(&self, axon_root: &Path, manual: bool) -> Result<SyncOutcome, String> {
        let now_ms = axon_time::unix_now_ms();
        let last_attempt = self.last_attempt_at_ms.load(Ordering::Acquire);
        let last_success = self.last_success_at_ms.load(Ordering::Acquire);

        if manual {
            if now_ms.saturating_sub(last_attempt) < MANUAL_DEBOUNCE_INTERVAL_MS {
                let remaining = MANUAL_DEBOUNCE_INTERVAL_MS - (now_ms - last_attempt);
                return Ok(SyncOutcome::Debounced {
                    remaining_ms: remaining,
                });
            }
        } else if now_ms.saturating_sub(last_success) < AUTO_SYNC_INTERVAL_MS {
            return Ok(SyncOutcome::RateLimited);
        }

        self.last_attempt_at_ms.store(now_ms, Ordering::Release);

        let json_body = match self
            .transport
            .fetch_json(&self.api_url, SYNC_HTTP_TIMEOUT)
            .await
        {
            Ok(body) => body,
            Err(e) => {
                axon_logger::warning(&format!("Models.dev 同步失败，保留本地规则快照并回退：{e}"));
                return Ok(SyncOutcome::FallbackToExisting { error: e });
            }
        };

        // 线上目录是「供应商 → 模型」的分层结构，先按它解析并按本地供应商过滤；
        // 分层形状给不出可用条目时（自建镜像或测试夹具用的扁平形状、本地没有可对照的
        // 供应商名）再退回扁平解析，两条路都空就如实回退，而不是报「成功写入 0 条」。
        let local_providers = local_provider_names(axon_root);
        let catalog_rules = parse_models_dev_catalog(&json_body, now_ms, &local_providers)
            .unwrap_or_else(|_| Vec::new());
        let rules = if catalog_rules.is_empty() {
            match parse_models_dev_json(&json_body, now_ms) {
                Ok(rules) if !rules.is_empty() => rules,
                Ok(_) => {
                    axon_logger::warning("Models.dev 目录里没有与本地供应商同名的条目，费率未写入");
                    return Ok(SyncOutcome::FallbackToExisting {
                        error: "目录里没有与本机供应商同名的费率条目，本地规则保持原样".to_owned(),
                    });
                }
                Err(e) => {
                    axon_logger::warning(&format!(
                        "Models.dev 响应解析失败，保留本地规则快照并回退：{e}"
                    ));
                    return Ok(SyncOutcome::FallbackToExisting { error: e });
                }
            }
        } else {
            catalog_rules
        };

        let count = rules.len();
        let mut repository = UsageRepository::open(axon_root)?;
        repository.save_pricing_rules(&rules)?;
        self.last_success_at_ms.store(now_ms, Ordering::Release);

        Ok(SyncOutcome::Success { rules_saved: count })
    }
}

impl Default for PricingSyncService {
    fn default() -> Self {
        Self::new()
    }
}

/// 自动同步入口：由系统 Cron 任务每天调用一次（`manual = false` 走每日限流）。
///
/// 返回值是本次写入的规则条数；同步失败时按 `FallbackToExisting` 回退到本地快照，
/// 这里如实返回 0 而不是把它当成成功。
pub(crate) fn run_pricing_sync_once(config_path: &Path) -> Result<usize, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let service = PricingSyncService::new();
    let outcome = block_on(service.sync(&axon_root, false))??;
    Ok(match outcome {
        SyncOutcome::Success { rules_saved } => rules_saved,
        _ => 0,
    })
}

/// 在同步上下文里执行一个 future。
///
/// Cron 的执行线程不在 Tokio 运行时内，因此按需建一个当前线程运行时；
/// 已经在运行时里时改用 `block_in_place`，避免嵌套建运行时。
fn block_on<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        Ok(tokio::task::block_in_place(|| handle.block_on(future)))
    } else {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("创建 Tokio 运行时失败: {error}"))?;
        Ok(runtime.block_on(future))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{ModelPriceRule, PricingStyle};

    struct MockTransport {
        response: Result<String, String>,
    }

    #[async_trait::async_trait]
    impl SyncHttpTransport for MockTransport {
        async fn fetch_json(&self, _url: &str, _timeout: Duration) -> Result<String, String> {
            self.response.clone()
        }
    }

    const FIXTURE_SUCCESS: &str = r#"[
        {
            "id": "gpt-4o",
            "cost": {
                "input": 2.5,
                "output": 10.0,
                "cache_read": 1.25
            }
        },
        {
            "id": "claude-3-5-sonnet",
            "cost": {
                "input": 3.0,
                "output": 15.0,
                "cache_read": 0.3,
                "cache_write": 3.75
            }
        }
    ]"#;

    const FIXTURE_MISSING_FIELDS: &str = r#"[
        {
            "id": "valid-model",
            "cost": {
                "input": 1.0,
                "output": 2.0
            }
        },
        {
            "id": "missing-output",
            "cost": {
                "input": 1.0
            }
        },
        {
            "id": "missing-input",
            "cost": {
                "output": 2.0
            }
        },
        {
            "id": "negative-cost",
            "cost": {
                "input": -1.0,
                "output": 2.0
            }
        }
    ]"#;

    #[tokio::test]
    async fn test_sync_success_path() {
        let temp = axon_test_support::test_temp_dir();
        let transport = Box::new(MockTransport {
            response: Ok(FIXTURE_SUCCESS.to_string()),
        });
        let service = PricingSyncService::with_transport(transport, 0);

        let outcome = service.sync(temp.path(), true).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Success { rules_saved: 2 });

        // 检查数据库中保存的规则
        let repo = UsageRepository::open(temp.path()).unwrap();
        let rules = repo.list_pricing_rules().unwrap();
        assert_eq!(rules.len(), 2);

        let gpt = rules.iter().find(|r| r.model_pattern == "gpt-4o").unwrap();
        assert_eq!(gpt.pricing_style, PricingStyle::OpenAi);
        assert_eq!(gpt.input_price_per_m_micros, 2_500_000);
        assert_eq!(gpt.output_price_per_m_micros, 10_000_000);
        assert_eq!(gpt.cache_read_price_per_m_micros, 1_250_000);
        assert_eq!(gpt.cache_write_price_per_m_micros, 0);

        let claude = rules
            .iter()
            .find(|r| r.model_pattern == "claude-3-5-sonnet")
            .unwrap();
        assert_eq!(claude.pricing_style, PricingStyle::Claude);
        assert_eq!(claude.cache_write_price_per_m_micros, 3_750_000);
    }

    #[tokio::test]
    async fn test_sync_missing_fields_skipped() {
        let temp = axon_test_support::test_temp_dir();
        let transport = Box::new(MockTransport {
            response: Ok(FIXTURE_MISSING_FIELDS.to_string()),
        });
        let service = PricingSyncService::with_transport(transport, 0);

        let outcome = service.sync(temp.path(), true).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Success { rules_saved: 1 });

        let repo = UsageRepository::open(temp.path()).unwrap();
        let rules = repo.list_pricing_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].model_pattern, "valid-model");
    }

    #[tokio::test]
    async fn test_sync_failure_fallback_to_existing() {
        let temp = axon_test_support::test_temp_dir();

        // 预先写入本地规则
        let pre_rule = ModelPriceRule {
            id: "local-rule".to_string(),
            model_pattern: "local-model".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 1_000_000,
            output_price_per_m_micros: 2_000_000,
            cache_read_price_per_m_micros: 0,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        {
            let repo = UsageRepository::open(temp.path()).unwrap();
            repo.save_pricing_rule(&pre_rule).unwrap();
        }

        // 模拟网络请求超时失败
        let transport = Box::new(MockTransport {
            response: Err("网络超时 10s".to_string()),
        });
        let service = PricingSyncService::with_transport(transport, 0);

        let outcome = service.sync(temp.path(), true).await.unwrap();
        assert!(matches!(outcome, SyncOutcome::FallbackToExisting { .. }));

        // 验证本地原有规则未被破坏或清空
        let repo = UsageRepository::open(temp.path()).unwrap();
        let rules = repo.list_pricing_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].model_pattern, "local-model");
    }

    #[tokio::test]
    async fn test_sync_debounce_and_rate_limit() {
        let temp = axon_test_support::test_temp_dir();
        let transport = Box::new(MockTransport {
            response: Ok(FIXTURE_SUCCESS.to_string()),
        });
        let service = PricingSyncService::with_transport(transport, 0);

        // 第一次手动同步成功
        let outcome1 = service.sync(temp.path(), true).await.unwrap();
        assert!(matches!(outcome1, SyncOutcome::Success { .. }));

        // 立即再次手动同步 -> 触发 60s 防抖
        let outcome2 = service.sync(temp.path(), true).await.unwrap();
        assert!(matches!(outcome2, SyncOutcome::Debounced { .. }));

        // 自动同步在 24h 内 -> 触发 RateLimited
        let outcome3 = service.sync(temp.path(), false).await.unwrap();
        assert_eq!(outcome3, SyncOutcome::RateLimited);
    }
}
