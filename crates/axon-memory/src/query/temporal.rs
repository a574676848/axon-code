//! 相对时间窗解析：把「昨天 / 上周 / 近3天」这类可判定词转成闭区间毫秒时间窗。
//!
//! 设计约束（M1-2）：
//! * 只识别能唯一确定区间的词；命中不了返回 `None`（宁可漏，不可错）。
//! * 「最近 / 之前 / 上次 / 那阵子」等模糊词一律不解析。
//! * 纯整数运算，不引第三方时间库，不做 IO，不 panic。

/// 一天的毫秒数。
const DAY_MS: i64 = 86_400_000;
/// 「近N天 / N天前」支持的最大 N。
const MAX_RELATIVE_DAYS: i64 = 30;

/// 闭区间时间窗，单位毫秒（UTC epoch）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    /// 区间起点（含）。
    pub start_ms: i64,
    /// 区间终点（含）。
    pub end_ms: i64,
}

impl TimeRange {
    /// 构造闭区间；起点大于终点时返回 `None`。
    #[must_use]
    pub const fn new(start_ms: i64, end_ms: i64) -> Option<Self> {
        if start_ms > end_ms {
            None
        } else {
            Some(Self { start_ms, end_ms })
        }
    }

    /// 时间戳是否落在区间内。
    #[must_use]
    pub const fn contains(&self, at_ms: i64) -> bool {
        at_ms >= self.start_ms && at_ms <= self.end_ms
    }

    /// 区间长度，单位毫秒。
    #[must_use]
    pub const fn len_ms(&self) -> i64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// 解析文本中的相对时间提示。
///
/// `now_ms` 为当前 UTC 毫秒，`tz_offset_min` 为本地时区相对 UTC 的分钟偏移
/// （东八区为 `480`）。未命中白名单词表时返回 `None`。
#[must_use]
pub fn parse_temporal_hint(text: &str, now_ms: i64, tz_offset_min: i32) -> Option<TimeRange> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let tz_ms = i64::from(tz_offset_min) * 60_000;
    let today = now_ms.saturating_add(tz_ms).div_euclid(DAY_MS);

    // 更具体的词优先：大前天 > 前天 > 昨天 > 今天。
    if text.contains("大前天") {
        return Some(day_range(today - 3, tz_ms));
    }
    if text.contains("前天") {
        return Some(day_range(today - 2, tz_ms));
    }
    if text.contains("昨天") {
        return Some(day_range(today - 1, tz_ms));
    }
    if text.contains("今天") {
        return Some(day_range(today, tz_ms));
    }
    if text.contains("本周") || text.contains("这周") {
        return Some(week_range(today, tz_ms));
    }
    if text.contains("上周") {
        return Some(week_range(today - 7, tz_ms));
    }
    if text.contains("本月") || text.contains("这个月") {
        return Some(month_range(today, tz_ms, 0));
    }
    if text.contains("上月") {
        return Some(month_range(today, tz_ms, -1));
    }
    if let Some(days) = match_recent_days(text) {
        let span = days.saturating_mul(DAY_MS);
        return TimeRange::new(now_ms.saturating_sub(span), now_ms);
    }
    if let Some(days) = match_days_before(text) {
        return Some(day_range(today - days, tz_ms));
    }
    None
}

/// 单日 00:00:00.000 ~ 23:59:59.999（本地时区）。
fn day_range(day: i64, tz_ms: i64) -> TimeRange {
    let start = day.saturating_mul(DAY_MS).saturating_sub(tz_ms);
    TimeRange {
        start_ms: start,
        end_ms: start.saturating_add(DAY_MS - 1),
    }
}

/// 含 `day` 的周一 00:00 ~ 周日 23:59:59.999（本地时区）。
fn week_range(day: i64, tz_ms: i64) -> TimeRange {
    let weekday = (day + 4).rem_euclid(7); // 0=周日, 1=周一 … 6=周六
    let monday = day - (weekday + 6).rem_euclid(7);
    let start = monday.saturating_mul(DAY_MS).saturating_sub(tz_ms);
    TimeRange {
        start_ms: start,
        end_ms: start.saturating_add(7 * DAY_MS - 1),
    }
}

/// 含 `day` 的自然月，`month_offset` 为相对月偏移（-1 表示上一月）。
fn month_range(day: i64, tz_ms: i64, month_offset: i64) -> TimeRange {
    let (year, month, _) = civil_from_days(day);
    let mut index = year.saturating_mul(12).saturating_add(i64::from(month) - 1) + month_offset;
    let start_year = index.div_euclid(12);
    index = index.rem_euclid(12);
    let start_month = u32::try_from(index + 1).unwrap_or(1);
    let (next_year, next_month) = if start_month == 12 {
        (start_year + 1, 1)
    } else {
        (start_year, start_month + 1)
    };
    let start_day = days_from_civil(start_year, start_month, 1);
    let next_day = days_from_civil(next_year, next_month, 1);
    let start = start_day.saturating_mul(DAY_MS).saturating_sub(tz_ms);
    let end = next_day
        .saturating_mul(DAY_MS)
        .saturating_sub(tz_ms)
        .saturating_sub(1);
    TimeRange {
        start_ms: start,
        end_ms: end,
    }
}

/// 匹配「N天前」，返回 N（1..=30）。不认「最近」等模糊前缀。
fn match_days_before(text: &str) -> Option<i64> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index + 1 < chars.len() {
        if chars[index] == '天' && chars[index + 1] == '前' {
            let mut digits = Vec::new();
            let mut cursor = index;
            while cursor > 0 && chars[cursor - 1].is_ascii_digit() {
                cursor -= 1;
                digits.push(chars[cursor]);
            }
            if !digits.is_empty() {
                let value = digits
                    .into_iter()
                    .rev()
                    .collect::<String>()
                    .parse::<i64>()
                    .ok()?;
                if (1..=MAX_RELATIVE_DAYS).contains(&value) {
                    return Some(value);
                }
                return None;
            }
        }
        index += 1;
    }
    None
}

/// 匹配「近N天」，返回 N（1..=30）。「最近」不算命中。
fn match_recent_days(text: &str) -> Option<i64> {
    let chars = text.chars().collect::<Vec<_>>();
    for (index, ch) in chars.iter().enumerate() {
        if *ch != '近' {
            continue;
        }
        if index > 0 && chars[index - 1] == '最' {
            continue;
        }
        let mut cursor = index + 1;
        let mut digits = String::new();
        while cursor < chars.len() && chars[cursor].is_ascii_digit() {
            digits.push(chars[cursor]);
            cursor += 1;
        }
        if digits.is_empty() || cursor >= chars.len() || chars[cursor] != '天' {
            continue;
        }
        let value = digits.parse::<i64>().ok()?;
        if (1..=MAX_RELATIVE_DAYS).contains(&value) {
            return Some(value);
        }
        return None;
    }
    None
}

/// 自 1970-01-01 起的天数转公历年月日（Howard Hinnant 算法）。
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// 公历年月日转自 1970-01-01 起的天数（Howard Hinnant 算法）。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{TimeRange, days_from_civil, parse_temporal_hint};

    const TZ_EIGHT: i32 = 480;

    fn local_ms(year: i64, month: u32, day: u32, hour: i64, minute: i64) -> i64 {
        days_from_civil(year, month, day) * 86_400_000
            + (hour * 3_600 + minute * 60) * 1_000
            - i64::from(TZ_EIGHT) * 60_000
    }

    fn local_day(year: i64, month: u32, day: u32) -> i64 {
        days_from_civil(year, month, day)
    }

    fn expect_day(range: Option<TimeRange>, year: i64, month: u32, day: u32) {
        let range = range.expect("应命中相对时间词");
        let start = local_day(year, month, day) * 86_400_000 - i64::from(TZ_EIGHT) * 60_000;
        assert_eq!(range.start_ms, start, "区间起点不是目标日 00:00");
        assert_eq!(
            range.end_ms,
            start + 86_400_000 - 1,
            "区间终点不是目标日 23:59:59.999"
        );
    }

    fn now_ms() -> i64 {
        // 2026-11-12（周四）12:00 本地时间。
        local_ms(2026, 11, 12, 12, 0)
    }

    #[test]
    fn resolves_today() {
        expect_day(
            parse_temporal_hint("今天", now_ms(), TZ_EIGHT),
            2026,
            11,
            12,
        );
    }

    #[test]
    fn resolves_today_inside_sentence() {
        expect_day(
            parse_temporal_hint("今天天气怎么样", now_ms(), TZ_EIGHT),
            2026,
            11,
            12,
        );
    }

    #[test]
    fn resolves_yesterday() {
        expect_day(
            parse_temporal_hint("昨天的日志", now_ms(), TZ_EIGHT),
            2026,
            11,
            11,
        );
    }

    #[test]
    fn resolves_day_before_yesterday() {
        expect_day(
            parse_temporal_hint("前天的改动", now_ms(), TZ_EIGHT),
            2026,
            11,
            10,
        );
    }

    #[test]
    fn resolves_three_days_ago_word() {
        expect_day(
            parse_temporal_hint("大前天", now_ms(), TZ_EIGHT),
            2026,
            11,
            9,
        );
    }

    #[test]
    fn three_days_ago_word_wins_over_two_days_ago_substring() {
        expect_day(
            parse_temporal_hint("大前天部署的那版", now_ms(), TZ_EIGHT),
            2026,
            11,
            9,
        );
    }

    #[test]
    fn resolves_this_week_monday_to_sunday() {
        let range = parse_temporal_hint("本周的提交", now_ms(), TZ_EIGHT).expect("应命中本周");
        let monday = local_day(2026, 11, 9);
        assert_eq!(
            range.start_ms,
            monday * 86_400_000 - i64::from(TZ_EIGHT) * 60_000
        );
        assert_eq!(range.len_ms(), 7 * 86_400_000 - 1);
    }

    #[test]
    fn resolves_this_week_when_today_is_sunday() {
        let sunday_noon = local_ms(2026, 11, 15, 12, 0);
        let range = parse_temporal_hint("这周做了什么", sunday_noon, TZ_EIGHT).expect("应命中这周");
        let monday = local_day(2026, 11, 9);
        assert_eq!(
            range.start_ms,
            monday * 86_400_000 - i64::from(TZ_EIGHT) * 60_000
        );
    }

    #[test]
    fn resolves_last_week() {
        let range = parse_temporal_hint("上周三那个方案", now_ms(), TZ_EIGHT).expect("应命中上周");
        let monday = local_day(2026, 11, 2);
        assert_eq!(
            range.start_ms,
            monday * 86_400_000 - i64::from(TZ_EIGHT) * 60_000
        );
        assert_eq!(range.len_ms(), 7 * 86_400_000 - 1);
    }

    #[test]
    fn resolves_this_month() {
        let range = parse_temporal_hint("本月的花费", now_ms(), TZ_EIGHT).expect("应命中本月");
        assert_eq!(
            range.start_ms,
            local_day(2026, 11, 1) * 86_400_000 - i64::from(TZ_EIGHT) * 60_000
        );
        assert_eq!(
            range.end_ms,
            (local_day(2026, 12, 1) - 1) * 86_400_000 + 86_400_000 - 1
                - i64::from(TZ_EIGHT) * 60_000
        );
    }

    #[test]
    fn resolves_last_month() {
        let range = parse_temporal_hint("上月的报表", now_ms(), TZ_EIGHT).expect("应命中上月");
        assert_eq!(
            range.start_ms,
            local_day(2026, 10, 1) * 86_400_000 - i64::from(TZ_EIGHT) * 60_000
        );
        assert_eq!(
            range.end_ms,
            local_day(2026, 11, 1) * 86_400_000 - i64::from(TZ_EIGHT) * 60_000 - 1
        );
    }

    #[test]
    fn resolves_last_month_across_year_boundary() {
        let january = local_ms(2027, 1, 15, 9, 0);
        let range = parse_temporal_hint("上月总结", january, TZ_EIGHT).expect("应命中上月");
        assert_eq!(
            range.start_ms,
            local_day(2026, 12, 1) * 86_400_000 - i64::from(TZ_EIGHT) * 60_000
        );
        assert_eq!(
            range.end_ms,
            local_day(2027, 1, 1) * 86_400_000 - i64::from(TZ_EIGHT) * 60_000 - 1
        );
    }

    #[test]
    fn resolves_recent_days_window_ending_now() {
        let range = parse_temporal_hint("近3天", now_ms(), TZ_EIGHT).expect("应命中近3天");
        assert_eq!(range.end_ms, now_ms());
        assert_eq!(range.start_ms, now_ms() - 3 * 86_400_000);
    }

    #[test]
    fn resolves_recent_one_day() {
        let range = parse_temporal_hint("近1天", now_ms(), TZ_EIGHT).expect("应命中近1天");
        assert_eq!(range.start_ms, now_ms() - 86_400_000);
    }

    #[test]
    fn resolves_recent_thirty_days_upper_bound() {
        let range = parse_temporal_hint("近30天", now_ms(), TZ_EIGHT).expect("应命中近30天");
        assert_eq!(range.start_ms, now_ms() - 30 * 86_400_000);
    }

    #[test]
    fn rejects_recent_zero_days() {
        assert_eq!(parse_temporal_hint("近0天", now_ms(), TZ_EIGHT), None);
    }

    #[test]
    fn rejects_recent_beyond_thirty_days() {
        assert_eq!(parse_temporal_hint("近31天", now_ms(), TZ_EIGHT), None);
    }

    #[test]
    fn resolves_days_before() {
        expect_day(
            parse_temporal_hint("3天前的记录", now_ms(), TZ_EIGHT),
            2026,
            11,
            9,
        );
    }

    #[test]
    fn resolves_one_day_before() {
        expect_day(
            parse_temporal_hint("1天前", now_ms(), TZ_EIGHT),
            2026,
            11,
            11,
        );
    }

    #[test]
    fn resolves_thirty_days_before_across_month() {
        let range = parse_temporal_hint("30天前", now_ms(), TZ_EIGHT).expect("应命中30天前");
        let target = local_day(2026, 11, 12) - 30;
        assert_eq!(
            range.start_ms,
            target * 86_400_000 - i64::from(TZ_EIGHT) * 60_000
        );
    }

    #[test]
    fn rejects_days_before_beyond_thirty() {
        assert_eq!(parse_temporal_hint("31天前", now_ms(), TZ_EIGHT), None);
    }

    #[test]
    fn rejects_vague_reference_words() {
        for text in ["之前那个", "上次说的方案", "那阵子", "最近", "后来呢"] {
            assert_eq!(
                parse_temporal_hint(text, now_ms(), TZ_EIGHT),
                None,
                "{text} 不应被解析成时间窗"
            );
        }
    }

    #[test]
    fn rejects_zuijin_prefix_even_with_digits() {
        assert_eq!(parse_temporal_hint("最近3天", now_ms(), TZ_EIGHT), None);
    }

    #[test]
    fn rejects_empty_and_blank() {
        assert_eq!(parse_temporal_hint("", now_ms(), TZ_EIGHT), None);
        assert_eq!(parse_temporal_hint("   ", now_ms(), TZ_EIGHT), None);
    }

    #[test]
    fn resolves_local_day_boundary_under_negative_timezone() {
        // UTC-5：本地 2026-11-12 00:30 对应 UTC 2026-11-12 05:30。
        let tz = -300;
        let utc_now = days_from_civil(2026, 11, 12) * 86_400_000 + 5 * 3_600_000 + 30 * 60_000;
        let range = parse_temporal_hint("今天", utc_now, tz).expect("应命中今天");
        let expected_start =
            days_from_civil(2026, 11, 12) * 86_400_000 + 300 * 60_000;
        assert_eq!(range.start_ms, expected_start);
    }

    #[test]
    fn range_contains_respects_inclusive_bounds() {
        let range = TimeRange::new(100, 200).expect("合法区间");
        assert!(range.contains(100));
        assert!(range.contains(200));
        assert!(!range.contains(99));
        assert!(!range.contains(201));
    }

    #[test]
    fn range_new_rejects_inverted_bounds() {
        assert_eq!(TimeRange::new(200, 100), None);
    }

    #[test]
    fn days_roundtrip_around_epoch_and_leap_years() {
        for (year, month, day) in [
            (1970, 1, 1),
            (2000, 2, 29),
            (2026, 11, 12),
            (2024, 12, 31),
            (2100, 3, 1),
        ] {
            let days = days_from_civil(year, month, day);
            assert_eq!(super::civil_from_days(days), (year, month, day));
        }
    }

    #[test]
    fn today_range_covers_now() {
        let range = parse_temporal_hint("今天", now_ms(), TZ_EIGHT).expect("应命中今天");
        assert!(range.contains(now_ms()));
    }
}
