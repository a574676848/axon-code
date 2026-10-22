use super::*;
use std::sync::Arc;
use std::thread;

#[test]
fn test_heatmap_levels_boundary() {
    assert_eq!(calculate_heatmap_level(0, 0), 0);
    assert_eq!(calculate_heatmap_level(0, 100), 0);
    assert_eq!(calculate_heatmap_level(10, 0), 0);

    // max_value = 100
    assert_eq!(calculate_heatmap_level(25, 100), 1);
    assert_eq!(calculate_heatmap_level(26, 100), 2);
    assert_eq!(calculate_heatmap_level(50, 100), 2);
    assert_eq!(calculate_heatmap_level(51, 100), 3);
    assert_eq!(calculate_heatmap_level(75, 100), 3);
    assert_eq!(calculate_heatmap_level(76, 100), 4);
    assert_eq!(calculate_heatmap_level(100, 100), 4);

    // 单点最大
    let buckets = build_activity_heatmap_buckets(&[
        ("2026-09-01".to_string(), 0),
        ("2026-09-02".to_string(), 100),
    ]);
    assert_eq!(buckets[0].level, 0);
    assert_eq!(buckets[1].level, 4);

    // 全零
    let zero_buckets = build_activity_heatmap_buckets(&[
        ("2026-09-01".to_string(), 0),
        ("2026-09-02".to_string(), 0),
    ]);
    assert_eq!(zero_buckets[0].level, 0);
    assert_eq!(zero_buckets[1].level, 0);
}

#[test]
fn test_health_classification_boundary() {
    // 无请求跳过
    assert_eq!(classify_health(0, 0), None);
    assert_eq!(classify_health(0, 5), None);

    // 全成功
    let (status, rate) = classify_health(10, 0).expect("需产出");
    assert_eq!(status, HealthStatus::Healthy);
    assert!((rate - 1.0).abs() < f64::EPSILON);

    // 部分失败
    let (status, rate) = classify_health(10, 3).expect("需产出");
    assert_eq!(status, HealthStatus::Degraded);
    assert!((rate - 0.7).abs() < f64::EPSILON);

    // 全失败
    let (status, rate) = classify_health(10, 10).expect("需产出");
    assert_eq!(status, HealthStatus::Failed);
    assert!((rate - 0.0).abs() < f64::EPSILON);

    // 单条边界
    let (status, rate) = classify_health(1, 0).expect("需产出");
    assert_eq!(status, HealthStatus::Healthy);
    assert!((rate - 1.0).abs() < f64::EPSILON);

    let (status, rate) = classify_health(1, 1).expect("需产出");
    assert_eq!(status, HealthStatus::Failed);
    assert!((rate - 0.0).abs() < f64::EPSILON);
}

#[test]
fn test_window_epoch_clearing_across_rounds() {
    let window = SlidingWindowRingBuffer::new();
    let delta = MetricDelta {
        request_count: 10,
        total_input_tokens: 500,
        cost_micros: 200,
        ..Default::default()
    };

    // 写入初始槽位 0 (0ms)
    window.record(0, &delta);
    let snap1 = window.snapshot(0, 15);
    assert_eq!(snap1.window_request_count, 10);
    assert_eq!(snap1.window_total_tokens, 500);

    // 推进 361 个槽位（3610 秒 > 3600 秒满轮）
    let slot_360_time = 360 * SLIDING_WINDOW_SLOT_DURATION_MS;
    let delta2 = MetricDelta {
        request_count: 7,
        total_input_tokens: 300,
        cost_micros: 150,
        ..Default::default()
    };

    // 写入复用的槽位 0，必须清除上一轮残留的 10 次请求，只包含 7 次！
    window.record(slot_360_time, &delta2);

    let slot_index = 0;
    let slot_snap = window.slots[slot_index]
        .snapshot_at(360)
        .expect("必须命中新 epoch");
    assert_eq!(slot_snap.request_count, 7);
    assert_eq!(slot_snap.total_input_tokens, 300);
    assert_eq!(slot_snap.cost_micros, 150);

    // 再次断言旧 epoch 360 槽之前的快照不应混入
    assert_eq!(window.slots[slot_index].snapshot_at(0), None);
}

#[test]
fn test_concurrent_slot_writes_without_locks() {
    let window = Arc::new(SlidingWindowRingBuffer::new());
    let threads_count = 10;
    let writes_per_thread = 500;
    let target_timestamp = 1_790_000_000_000_u64;

    let mut handles = Vec::with_capacity(threads_count);
    for _ in 0..threads_count {
        let window_clone = Arc::clone(&window);
        handles.push(thread::spawn(move || {
            let delta = MetricDelta {
                request_count: 1,
                success_count: 1,
                total_input_tokens: 10,
                output_tokens: 5,
                cost_micros: 2,
                ..Default::default()
            };
            for _ in 0..writes_per_thread {
                window_clone.record(target_timestamp, &delta);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("并发写入线程不应 panic");
    }

    let snap = window.snapshot(target_timestamp, 15);
    let expected_requests = (threads_count * writes_per_thread) as u64;
    assert_eq!(snap.window_request_count, expected_requests);
    assert_eq!(snap.window_total_tokens, expected_requests * 15);
    assert_eq!(snap.window_cost_micros, expected_requests * 2);
}

#[test]
fn test_read_snapshot_during_overwrite_does_not_panic() {
    let window = Arc::new(SlidingWindowRingBuffer::new());
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let window_writer = Arc::clone(&window);
    let stop_writer = Arc::clone(&stop_flag);
    let writer = thread::spawn(move || {
        let mut slot_id = 0_u64;
        let delta = MetricDelta {
            request_count: 1,
            total_input_tokens: 100,
            cost_micros: 50,
            ..Default::default()
        };
        while !stop_writer.load(std::sync::atomic::Ordering::Relaxed) {
            let timestamp = slot_id * SLIDING_WINDOW_SLOT_DURATION_MS;
            window_writer.record(timestamp, &delta);
            slot_id = slot_id.wrapping_add(1);
        }
    });

    let window_reader = Arc::clone(&window);
    let stop_reader = Arc::clone(&stop_flag);
    let reader = thread::spawn(move || {
        let mut read_count = 0_u32;
        while !stop_reader.load(std::sync::atomic::Ordering::Relaxed) && read_count < 200 {
            let now_ms = 1_000 * SLIDING_WINDOW_SLOT_DURATION_MS;
            let report = window_reader.snapshot(now_ms, 15);
            assert!(!report.window.is_empty());
            read_count += 1;
        }
    });

    reader.join().expect("读者线程不应 panic");
    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    writer.join().expect("写者线程不应 panic");
}
