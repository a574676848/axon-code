use super::*;

#[test]
fn test_channel_affinity_cache() {
    let mut cache = ChannelAffinityCache::new();

    assert_eq!(cache.get("gpt-4o", "session-1", 1000), None);
    cache.record("gpt-4o", "session-1", "openai", 1000, 5000);
    assert_eq!(cache.get("gpt-4o", "session-1", 2000), Some("openai"));
    assert_eq!(cache.get("gpt-4o", "session-1", 6999), Some("openai"));
    assert_eq!(cache.get("gpt-4o", "session-1", 12000), None);
    assert_eq!(cache.get("gpt-4o", "session-2", 2000), None);
}

#[test]
fn health_registry_singleton_lazy_inits_once() {
    let _singleton_guard = channel_singleton_test_guard();
    reset_channel_singletons_for_test();
    let policy = BreakerPolicy {
        failure_threshold: 1,
        cooldown_ms: 5_000,
    };
    let key = axon_agent_runtime::ChannelKey {
        model_name: "gpt-4o".to_owned(),
        provider: "p-high".to_owned(),
    };
    with_health_registry(policy, |health| health.record_failure(&key, 1_000));
    let available = with_health_registry(policy, |health| health.is_available(&key, 1_100));
    assert!(!available);
    reset_channel_singletons_for_test();
}

#[test]
fn affinity_cache_is_shared_across_threads() {
    let _singleton_guard = channel_singleton_test_guard();
    reset_channel_singletons_for_test();
    with_affinity_cache(|cache| {
        cache.record("gpt-4o", "session-cross-thread", "openai", 1_000, 5_000);
    });
    let handle = std::thread::spawn(|| {
        with_affinity_cache(|cache| {
            cache
                .get("gpt-4o", "session-cross-thread", 2_000)
                .map(str::to_owned)
        })
    });
    assert_eq!(
        handle
            .join()
            .expect("跨线程读取亲和缓存不应失败")
            .as_deref(),
        Some("openai")
    );
    reset_channel_singletons_for_test();
}

#[test]
fn channel_metrics_query_applies_all_filters_before_aggregation() {
    const SAMPLE_TIMESTAMP_MS: u64 = 1_725_321_600_000;
    let sample = PersistedChannelSample {
        channel_key: "gpt-5.6-terra|openai".to_owned(),
        latency_ms: 100,
        success: true,
        input_tokens: 10,
        output_tokens: 5,
        cache_hit_rate_basis_points: 8_000,
        timestamp_ms: SAMPLE_TIMESTAMP_MS,
        event_type: None,
        connection_reused: None,
        connection_age_ms: None,
        fallback_activated: false,
    };

    assert!(channel_sample_matches(
        &sample,
        Some(SAMPLE_TIMESTAMP_MS),
        Some("gpt-5.6-terra"),
        Some("openai")
    ));
    assert!(!channel_sample_matches(
        &sample,
        Some(SAMPLE_TIMESTAMP_MS + 1),
        Some("gpt-5.6-terra"),
        Some("openai")
    ));
    assert!(!channel_sample_matches(
        &sample,
        Some(SAMPLE_TIMESTAMP_MS),
        Some("gpt-4o"),
        None
    ));
    assert!(!channel_sample_matches(
        &sample,
        Some(SAMPLE_TIMESTAMP_MS),
        None,
        Some("anthropic")
    ));
}

#[test]
fn channel_metrics_query_skips_old_date_shards() {
    let old_path = PathBuf::from("samples-2025-01-01.jsonl");
    let current_path = PathBuf::from("samples-2025-01-02.jsonl");
    let cutoff_ms = chrono::DateTime::parse_from_rfc3339("2025-01-02T00:00:00Z")
        .expect("固定测试时间应有效")
        .timestamp_millis() as u64;

    assert!(shard_is_before_cutoff(&old_path, Some(cutoff_ms)));
    assert!(!shard_is_before_cutoff(&current_path, Some(cutoff_ms)));
    assert!(!shard_is_before_cutoff(&old_path, None));
}
