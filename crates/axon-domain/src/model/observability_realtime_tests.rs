//! 实时窗口报告的序列口径测试。

use super::*;

/// 造一个槽位快照；只填测试关心的字段。
fn slot(slot_id: u64, requests: u64, success: u64, input: u64, output: u64) -> SlotSnapshot {
    SlotSnapshot {
        slot_id,
        request_count: requests,
        success_count: success,
        failure_count: requests.saturating_sub(success),
        total_input_tokens: input,
        output_tokens: output,
        provider_calls: requests,
        measured_ttft_count: requests,
        ..Default::default()
    }
}

/// 一段连续槽位：从 `first_slot` 起 `count` 个，全部承载同一份增量。
fn slots(first_slot: u64, count: u64, requests: u64, input: u64, output: u64) -> Vec<SlotSnapshot> {
    (0..count)
        .map(|offset| slot(first_slot + offset, requests, requests, input, output))
        .collect()
}

#[test]
fn throughput_series_is_per_natural_minute_not_extrapolated() {
    // 一分钟内的 6 个槽位各承载 1000 输入 Token：这一分钟就是 6000，不是 1000×6 的推算。
    let snapshots = slots(600, SLOTS_PER_MINUTE, 1, 1_000, 0);
    let now_ms = 606 * SLIDING_WINDOW_SLOT_DURATION_MS;
    let report = build_realtime_window_report(15, 606, &snapshots);

    assert_eq!(
        report.throughput_series.len(),
        1,
        "6 个槽位合并成 1 个分钟点"
    );
    let point = &report.throughput_series[0];
    assert_eq!(point.token_per_min, 6_000);
    assert_eq!(point.request_per_min, 6, "每分钟请求数是实测条数，不是外推");
    assert_eq!(point.timestamp_ms % 60_000, 0, "时间戳落在自然分钟边界");
    assert_eq!(report.window_total_tokens, 6_000);
    let _ = now_ms;
}

#[test]
fn a_single_long_turn_no_longer_becomes_a_sixfold_spike() {
    // 一次 2200 万 Token 的长轮次落在某一个槽位里：它只算作那一分钟的量。
    let mut snapshots = slots(0, 3, 0, 0, 0);
    snapshots[1] = slot(1, 1, 1, 22_000_000, 0);
    let report = build_realtime_window_report(15, 3, &snapshots);

    assert_eq!(report.throughput_series.len(), 1);
    let peak = report
        .throughput_series
        .iter()
        .map(|point| point.token_per_min)
        .max()
        .unwrap_or(0);
    assert_eq!(peak, 22_000_000, "不再乘以 6 放大");
    assert_eq!(report.window_total_tokens, 22_000_000);
}

#[test]
fn series_and_status_share_one_grid_and_agree_on_counts() {
    // 两个相邻分钟：第一分钟 2 次请求（1 成功 1 失败），第二分钟 3 次全部成功。
    let mut snapshots = Vec::new();
    for offset in 0..SLOTS_PER_MINUTE {
        snapshots.push(slot(
            offset,
            u64::from(offset < 2),
            u64::from(offset == 0),
            10,
            5,
        ));
    }
    for offset in 0..SLOTS_PER_MINUTE {
        snapshots.push(slot(
            SLOTS_PER_MINUTE + offset,
            u64::from(offset < 3),
            u64::from(offset < 3),
            0,
            0,
        ));
    }
    let report = build_realtime_window_report(15, SLOTS_PER_MINUTE * 2 - 1, &snapshots);

    assert_eq!(report.throughput_series.len(), 2);
    assert_eq!(report.request_status_series.len(), 2);
    for (throughput, status) in report
        .throughput_series
        .iter()
        .zip(&report.request_status_series)
    {
        assert_eq!(
            throughput.timestamp_ms, status.timestamp_ms,
            "两条序列必须共用同一套时间戳，否则界面叠不上"
        );
    }
    assert_eq!(report.throughput_series[0].request_per_min, 2);
    assert_eq!(report.request_status_series[0].success_count, 1);
    assert_eq!(report.request_status_series[0].failure_count, 1);
    assert!((report.request_status_series[0].failure_rate - 0.5).abs() < f64::EPSILON);
    assert_eq!(report.throughput_series[1].request_per_min, 3);
    assert_eq!(report.request_status_series[1].success_count, 3);
    assert!((report.request_status_series[1].failure_rate - 0.0).abs() < f64::EPSILON);

    let series_total: u64 = report
        .throughput_series
        .iter()
        .map(|point| point.token_per_min)
        .sum();
    assert_eq!(
        series_total, report.window_total_tokens,
        "序列合计等于窗口总量"
    );
}

#[test]
fn current_reading_covers_the_last_sixty_seconds() {
    // 最近 6 个槽位（60 秒）里有 4 次请求、共 4000 Token，更早的不计入「当前」。
    let mut snapshots = slots(0, 4, 0, 0, 0);
    snapshots.extend(slots(4, 6, 1, 1_000, 0));
    let report = build_realtime_window_report(15, 9, &snapshots);

    assert_eq!(
        report.metrics.current_rpm, 6,
        "最近 60 秒内 6 个槽位各 1 次"
    );
    assert_eq!(report.metrics.current_tpm, 6_000);
    assert_eq!(report.window_request_count, 6);
    assert_eq!(report.metrics.average_rpm, 6.0 / 15.0);
}

#[test]
fn partial_minute_at_the_window_edge_still_reports_its_own_slice() {
    // 窗口边界只截到 3 个槽位：这一桶如实报已有的量，窗口总量也照实累加。
    let snapshots = slots(0, 3, 1, 100, 0);
    let report = build_realtime_window_report(15, 2, &snapshots);
    assert_eq!(report.throughput_series.len(), 1);
    assert_eq!(report.throughput_series[0].token_per_min, 300);
    assert_eq!(report.window_total_tokens, 300);
}

#[test]
fn empty_window_yields_empty_series_and_zero_readings() {
    let report = build_realtime_window_report(60, 1_000, &[]);
    assert!(report.throughput_series.is_empty());
    assert!(report.request_status_series.is_empty());
    assert_eq!(report.window_request_count, 0);
    assert_eq!(report.metrics.current_tpm, 0);
    assert_eq!(report.metrics.current_rpm, 0);
    assert_eq!(report.window, "60m");
}

#[test]
fn reasoning_tokens_count_towards_the_window_total_like_every_other_panel() {
    // 概览、分布与排行榜都把推理 Token 计入总量，实时窗口也必须同口径，
    // 否则同一段时间在「实时」与「概览」两处会给出两个不同的 Token 总量。
    let snapshot = SlotSnapshot {
        slot_id: 600,
        request_count: 1,
        success_count: 1,
        total_input_tokens: 1_000,
        output_tokens: 200,
        reasoning_output_tokens: 300,
        ..Default::default()
    };
    let report = build_realtime_window_report(15, 600, &[snapshot]);

    assert_eq!(report.window_total_tokens, 1_500);
    assert_eq!(report.throughput_series[0].token_per_min, 1_500);
    let composition = report.token_composition;
    assert_eq!(
        composition.uncached_input_tokens
            + composition.cache_read_tokens
            + composition.cache_write_tokens
            + composition.output_tokens
            + composition.reasoning_tokens,
        report.window_total_tokens,
        "构成明细必须与窗口总量对得上"
    );
    assert_eq!(composition.reasoning_tokens, 300);
}
