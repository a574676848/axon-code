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
    SYNCED_RULE_PRIORITY, local_model_ids, local_provider_names, parse_catalog_providers,
    parse_models_dev_catalog, parse_models_dev_json,
};
use super::pricing_local::local_model_rules;
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

        // 线上目录是「供应商 → 模型」的分层结构。两条通道都从这一次解析出发：
        // 1. 与本机配置同名供应商的条目，按目录 id 原样收录；
        // 2. 本机声明的模型 id，按目录的规范 id 对齐：思考档位后缀、日期戳、
        //    厂商前缀这些代理侧写法都还原到目录里的同一个模型，价格取厂商自营条目。
        // 分层形状给不出可用条目时（自建镜像或测试夹具用的扁平形状、本地没有可对照的
        // 供应商名）再退回扁平解析，两条路都空就如实回退，而不是报「成功写入 0 条」。
        let local_providers = local_provider_names(axon_root);
        let local_ids = local_model_ids(axon_root);
        let mut rules = match parse_catalog_providers(&json_body) {
            Ok(catalog) => {
                let mut rules = parse_models_dev_catalog(&catalog, now_ms, &local_providers);
                rules.extend(local_model_rules(&catalog, &local_ids, now_ms));
                rules
            }
            Err(_) => Vec::new(),
        };
        // 两条通道可能给同一个匹配式各写一条（本机 id 恰好就是目录 id）：只留先到的那条。
        // 目录通道的规则 id 稳定，后续同步继续按 id 覆盖，不会被别名通道反复改写。
        let mut seen_patterns = std::collections::BTreeSet::new();
        rules.retain(|rule| seen_patterns.insert(rule.model_pattern.to_ascii_lowercase()));

        if rules.is_empty() {
            match parse_models_dev_json(&json_body, now_ms) {
                Ok(flat) if !flat.is_empty() => rules = flat,
                Ok(_) => {
                    axon_logger::warning(
                        "Models.dev 目录里没有能对上本机供应商或本机模型的费率条目，费率未写入",
                    );
                    return Ok(SyncOutcome::FallbackToExisting {
                        error: "目录里没有能对上本机供应商或本机模型的费率条目，本地规则保持原样"
                            .to_owned(),
                    });
                }
                Err(e) => {
                    axon_logger::warning(&format!(
                        "Models.dev 响应解析失败，保留本地规则快照并回退：{e}"
                    ));
                    return Ok(SyncOutcome::FallbackToExisting { error: e });
                }
            }
        }

        let mut repository = UsageRepository::open(axon_root)?;
        // 手工规则优先：同一匹配式已经有更高优先级的规则时，同步不再塞一条注定让位的副本，
        // 否则界面上会出现两条同匹配式的行，谁生效还得靠优先级去猜。
        let manually_covered: std::collections::BTreeSet<String> = repository
            .list_pricing_rules()?
            .into_iter()
            .filter(|rule| rule.priority > SYNCED_RULE_PRIORITY)
            .map(|rule| rule.model_pattern.to_ascii_lowercase())
            .collect();
        rules.retain(|rule| !manually_covered.contains(&rule.model_pattern.to_ascii_lowercase()));

        let count = rules.len();
        if count > 0 {
            repository.save_pricing_rules(&rules)?;
        }
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

    /// 目录里的规范模型：供应商名与本机配置都对不上，只能靠别名通道定价。
    const FIXTURE_PROXY_ALIASES: &str = r#"{
        "google": {
            "models": {
                "gemini-4-hydrogen": {
                    "id": "gemini-4-hydrogen",
                    "canonical_model_id": "google/gemini-4-hydrogen",
                    "reasoning_options": [{"type": "effort", "values": ["low", "high", "xhigh"]}],
                    "cost": { "input": 1.5, "output": 7.5, "cache_read": 0.075 }
                }
            }
        },
        "deepseek": {
            "models": {
                "deepseek-flash": {
                    "id": "deepseek-flash",
                    "canonical_model_id": "deepseek/deepseek-v4.1-flash-1017",
                    "cost": { "input": 0.15, "output": 0.6, "cache_read": 0.003 }
                }
            }
        }
    }"#;

    /// 只声明代理侧写法的最小配置：供应商名都不在目录里。
    fn write_proxy_config(root: &Path) {
        std::fs::write(
            root.join("config.toml"),
            r#"[agent]
default_model = "gemini-3.8-flash"
fast_model = "fast_model"

[[models]]
id = "gemini-4-hydrogen-xhigh"
name = "gemini-3.8-flash"
provider = "cpa"

[[models]]
id = "deepseek-v4.1-flash-1017"
name = "fast_model"
provider = "tokenrhythm"

[[providers]]
api_key_env = "CPA_API_KEY"
base_url = "http://127.0.0.1:8317/v1"
name = "cpa"

[[providers]]
api_key_env = "RHYTHM_API_KEY"
base_url = "http://127.0.0.1:8800/v1"
name = "tokenrhythm"
"#,
        )
        .expect("隔离配置应可写入");
    }

    #[tokio::test]
    async fn test_sync_prices_proxy_side_model_aliases() {
        let temp = axon_test_support::test_temp_dir();
        write_proxy_config(temp.path());
        let transport = Box::new(MockTransport {
            response: Ok(FIXTURE_PROXY_ALIASES.to_string()),
        });
        let service = PricingSyncService::with_transport(transport, 0);

        let outcome = service.sync(temp.path(), true).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Success { rules_saved: 2 });

        let repo = UsageRepository::open(temp.path()).unwrap();
        let rules = repo.list_pricing_rules().unwrap();

        // 思考档位后缀：写成 `-xhigh` 的模型按 google 自营价计。
        let hydrogen = rules
            .iter()
            .find(|rule| rule.model_pattern == "gemini-4-hydrogen-xhigh")
            .expect("思考档位后缀应被同步定价");
        assert_eq!(hydrogen.input_price_per_m_micros, 1_500_000);
        assert_eq!(hydrogen.output_price_per_m_micros, 7_500_000);
        assert_eq!(hydrogen.priority, SYNCED_RULE_PRIORITY);

        // 日期戳别名：规范 id 就是带日期的那一个。
        let dated = rules
            .iter()
            .find(|rule| rule.model_pattern == "deepseek-v4.1-flash-1017")
            .expect("日期戳别名应被同步定价");
        assert_eq!(dated.output_price_per_m_micros, 600_000);
    }

    #[tokio::test]
    async fn test_sync_leaves_manually_priced_patterns_alone() {
        let temp = axon_test_support::test_temp_dir();
        write_proxy_config(temp.path());

        // 人手填的规则（优先级 0）与同步同匹配式：同步不应再写一条注定让位的副本。
        let manual = ModelPriceRule {
            id: "pr_manual".to_string(),
            model_pattern: "gemini-4-hydrogen-xhigh".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 1_500_000,
            output_price_per_m_micros: 7_500_000,
            cache_read_price_per_m_micros: 75_000,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        {
            let repo = UsageRepository::open(temp.path()).unwrap();
            repo.save_pricing_rule(&manual).unwrap();
        }

        let transport = Box::new(MockTransport {
            response: Ok(FIXTURE_PROXY_ALIASES.to_string()),
        });
        let service = PricingSyncService::with_transport(transport, 0);
        let outcome = service.sync(temp.path(), true).await.unwrap();
        assert_eq!(outcome, SyncOutcome::Success { rules_saved: 1 });

        let repo = UsageRepository::open(temp.path()).unwrap();
        let rules = repo.list_pricing_rules().unwrap();
        let same_pattern: Vec<&ModelPriceRule> = rules
            .iter()
            .filter(|rule| rule.model_pattern == "gemini-4-hydrogen-xhigh")
            .collect();
        assert_eq!(same_pattern.len(), 1, "同一匹配式不得出现两条规则");
        assert_eq!(same_pattern[0].priority, 0, "保留下来的必须是手工规则");
    }
}
