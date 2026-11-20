#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_uses_local_time_and_rejects_utc_suffix() {
        let schedule = parse_schedule("once:2030-01-15T09:30:00").expect("本地时间应可解析");
        let CronSchedule::OneShot { at_ms } = schedule else {
            panic!("应解析为一次性任务");
        };
        let utc = DateTime::<Utc>::from_timestamp_millis(i64::try_from(at_ms).expect("时间应有效"))
            .expect("时间戳应有效");
        let local = utc.with_timezone(&Local);

        assert_eq!((local.hour(), local.minute()), (9, 30));
        assert!(parse_schedule("once:2030-01-15T09:30:00Z").is_err());
    }

    #[test]
    fn cron_expression_matches_local_calendar_components() {
        let now_ms = axon_time::unix_now_ms();
        let candidate_ms = ((now_ms / MILLIS_PER_MINUTE) + 1) * MILLIS_PER_MINUTE;
        let utc = DateTime::<Utc>::from_timestamp_millis(
            i64::try_from(candidate_ms).expect("时间应在范围内"),
        )
        .expect("时间戳应有效");
        let local = utc.with_timezone(&Local);
        let expression = format!(
            "{} {} {} {} {}",
            local.minute(),
            local.hour(),
            local.day(),
            local.month(),
            local.weekday().num_days_from_sunday()
        );
        let schedule = parse_schedule(&expression).expect("本地 cron 应可解析");

        assert_eq!(next_run_after(&schedule, now_ms), Some(candidate_ms));
    }

    fn cron_expr(expression: &str) -> CronExpr {
        let schedule = parse_schedule(expression).expect("表达式应可解析");
        let CronSchedule::Cron { expr } = schedule else {
            panic!("应解析为 cron");
        };
        expr
    }

    #[test]
    fn hour_range_covers_every_hour_inside_bounds() {
        let expr = cron_expr("0 1-9 * * *");
        for hour in 1..=9 {
            assert!(expr.hours.matches(hour), "{hour} 点应在区间内");
        }
        assert!(!expr.hours.matches(0));
        assert!(!expr.hours.matches(10));
    }

    #[test]
    fn mixed_list_supports_ranges_and_single_values() {
        let expr = cron_expr("0 9,18-20 * * *");
        for hour in [9, 18, 19, 20] {
            assert!(expr.hours.matches(hour), "{hour} 点应命中");
        }
        assert!(!expr.hours.matches(17));
    }

    #[test]
    fn step_starts_from_field_lower_bound() {
        let expr = cron_expr("0 0 1 */3 *");
        for month in [1, 4, 7, 10] {
            assert!(expr.months.matches(month), "{month} 月应命中");
        }
        assert!(!expr.months.matches(3));
    }

    #[test]
    fn alias_compresses_time_without_calendar_semantics() {
        let hourly = cron_expr("@hourly");
        assert!(hourly.minutes.matches(0));
        assert!(hourly.hours.matches(0));
        assert!(hourly.hours.matches(13));
        assert_eq!(hourly.calendar, CalendarFilter::Any);

        assert!(parse_schedule("@hourly:09:00").is_err());
        assert!(parse_schedule("@hourly-unknown").is_err());
        assert!(parse_schedule("@workdays").is_err());
    }

    #[test]
    fn calendar_prefix_combines_with_any_cron_fields() {
        let expr = cron_expr("workday:0 8-20 * * *");
        assert_eq!(expr.calendar, CalendarFilter::Workday);
        for hour in 8..=20 {
            assert!(expr.hours.matches(hour), "{hour} 点应命中工作日时段");
        }
        assert!(!expr.hours.matches(7));
        assert!(!expr.hours.matches(21));
    }

    #[test]
    fn calendar_prefix_supports_steps_and_lists() {
        let expr = cron_expr("weekend:*/30 9-18 * * *");
        assert_eq!(expr.calendar, CalendarFilter::Weekend);
        assert!(expr.minutes.matches(0));
        assert!(expr.minutes.matches(30));
        assert!(!expr.minutes.matches(15));
        assert!(expr.hours.matches(9));
        assert!(expr.hours.matches(18));

        let holiday = cron_expr("holiday:0 10,14 * * *");
        assert_eq!(holiday.calendar, CalendarFilter::Holiday);
        assert!(holiday.hours.matches(10));
        assert!(holiday.hours.matches(14));
    }

    #[test]
    fn calendar_prefix_is_recorded_on_expression() {
        assert_eq!(
            cron_expr("workday:0 9 * * *").calendar,
            CalendarFilter::Workday
        );
        assert_eq!(
            cron_expr("weekday:0 9 * * *").calendar,
            CalendarFilter::Weekday
        );
        assert_eq!(
            cron_expr("holiday:0 9 * * *").calendar,
            CalendarFilter::Holiday
        );
        assert_eq!(
            cron_expr("weekend:0 9 * * *").calendar,
            CalendarFilter::Weekend
        );
        assert_eq!(cron_expr("0 9 * * *").calendar, CalendarFilter::Any);
    }

    fn local_ms(hour: u32, minute: u32) -> (u64, DateTime<Local>) {
        let local = Local
            .with_ymd_and_hms(2026, 3, 10, hour, minute, 0)
            .single()
            .expect("测试时间应有效");
        let ms = u64::try_from(local.timestamp_millis()).expect("时间应为正");
        (ms, local)
    }

    fn as_local(ms: u64) -> DateTime<Local> {
        DateTime::<Utc>::from_timestamp_millis(i64::try_from(ms).expect("时间应在范围内"))
            .expect("时间戳应有效")
            .with_timezone(&Local)
    }

    /// 间隔计划按原触发点对齐，不用「现在 + 间隔」：后者是固定延迟，任务体耗时会永久
    /// 累积成漂移（`interval:5m` 的任务跑 1 分钟，下一轮就落在 6 分钟后）。
    #[test]
    fn interval_schedule_aligns_to_the_original_trigger_instead_of_drifting() {
        let schedule = CronSchedule::Interval {
            every_ms: 5 * 60 * 1_000,
        };
        let scheduled_at_ms = 10 * 60 * 60 * 1_000;
        assert_eq!(
            next_run_from(&schedule, scheduled_at_ms, scheduled_at_ms + 60 * 1_000),
            Some(scheduled_at_ms + 5 * 60 * 1_000),
            "跑了一分钟也必须落回 10:05，而不是 10:06"
        );
        assert_eq!(
            next_run_from(
                &schedule,
                scheduled_at_ms,
                scheduled_at_ms + 12 * 60 * 1_000
            ),
            Some(scheduled_at_ms + 15 * 60 * 1_000),
            "错过多个间隔时推进到第一个严格未来时刻"
        );
    }

    #[test]
    fn next_run_respects_hour_range() {
        let schedule = parse_schedule("0 1-9 * * *").expect("区间应可解析");
        let (now_ms, _) = local_ms(10, 30);
        let next = next_run_after(&schedule, now_ms).expect("应有下次执行");
        let next_local = as_local(next);

        assert_eq!((next_local.hour(), next_local.minute()), (1, 0));
        assert!(next_local.date_naive() > as_local(now_ms).date_naive());
    }

    #[test]
    fn next_run_respects_calendar_prefix() {
        let schedule = parse_schedule("weekend:0 9 * * *").expect("周末前缀应可解析");
        let (now_ms, _) = local_ms(12, 0);
        let next = next_run_after(&schedule, now_ms).expect("应有下次执行");
        let next_local = as_local(next);

        assert!(matches!(next_local.weekday(), Weekday::Sat | Weekday::Sun));
        assert_eq!((next_local.hour(), next_local.minute()), (9, 0));
    }

    #[test]
    fn day_and_weekday_match_either_one() {
        let expr = cron_expr("0 9 1 * 1");
        let first_of_month = Local
            .with_ymd_and_hms(2026, 10, 1, 9, 0, 0)
            .single()
            .expect("时间应有效");
        let a_monday = Local
            .with_ymd_and_hms(2026, 10, 5, 9, 0, 0)
            .single()
            .expect("时间应有效");
        let neither = Local
            .with_ymd_and_hms(2026, 10, 6, 9, 0, 0)
            .single()
            .expect("时间应有效");

        assert!(expr.date_matches(&first_of_month), "1 号应命中");
        assert!(expr.date_matches(&a_monday), "周一应命中");
        assert!(!expr.date_matches(&neither), "既非 1 号也非周一时不命中");
    }

    #[test]
    fn range_step_and_question_mark_are_accepted() {
        let expr = cron_expr("0 9-18/2 * * *");
        for hour in [9, 11, 13, 15, 17] {
            assert!(expr.hours.matches(hour), "{hour} 点应命中");
        }
        assert!(!expr.hours.matches(10));
        assert!(!expr.hours.matches(18));

        assert_eq!(cron_expr("? ? ? ? ?").calendar, CalendarFilter::Any);
    }

    #[test]
    fn leap_day_schedule_is_reachable_within_lookahead() {
        let schedule = parse_schedule("0 0 29 2 *").expect("闰日应可解析");
        let (now_ms, _) = local_ms(12, 0);
        let next = require_next_run(&schedule, now_ms).expect("闰日应在视野内可求值");
        let next_local = as_local(next);

        assert_eq!(
            (
                next_local.month(),
                next_local.day(),
                next_local.hour(),
                next_local.minute()
            ),
            (2, 29, 0, 0)
        );
    }

    #[test]
    fn impossible_schedule_reports_actionable_error() {
        let schedule = parse_schedule("0 0 30 2 *").expect("语法合法");
        let (now_ms, _) = local_ms(12, 0);
        let error = require_next_run(&schedule, now_ms).expect_err("2 月 30 日不应有下次执行");
        assert!(error.contains("没有可触发时间"), "{error}");

        let past = parse_schedule("once:2020-01-01T00:00:00").expect("单次应可解析");
        let error = require_next_run(&past, now_ms).expect_err("过期单次应报错");
        assert!(error.contains("晚于当前时间"), "{error}");
    }
}
