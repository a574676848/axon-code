use std::collections::BTreeSet;

use chrono::{
    DateTime, Datelike as _, Local, LocalResult, NaiveDateTime, TimeZone as _, Timelike as _, Utc,
};

const MILLIS_PER_SECOND: u64 = 1_000;
const MILLIS_PER_MINUTE: u64 = 60 * MILLIS_PER_SECOND;
const MILLIS_PER_HOUR: u64 = 60 * MILLIS_PER_MINUTE;
const CRON_LOOKAHEAD_MINUTES: u64 = 366 * 24 * 60;
const LOCAL_ONCE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CronSchedule {
    Interval { every_ms: u64 },
    OneShot { at_ms: u64 },
    Cron { expr: CronExpr },
}

impl CronSchedule {
    pub(super) const fn is_one_shot(&self) -> bool {
        matches!(self, Self::OneShot { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CronExpr {
    minutes: CronField,
    hours: CronField,
    days: CronField,
    months: CronField,
    weekdays: CronField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CronField {
    Any,
    Step(u32),
    Values(BTreeSet<u32>),
}

pub(crate) fn parse_schedule(value: &str) -> Result<CronSchedule, String> {
    let value = value.trim();
    if let Some(raw) = value.strip_prefix("interval:") {
        return parse_interval(raw).map(|every_ms| CronSchedule::Interval { every_ms });
    }
    if let Some(raw) = value.strip_prefix("once:") {
        return parse_local_once_ms(raw).map(|at_ms| CronSchedule::OneShot { at_ms });
    }
    parse_cron(value).map(|expr| CronSchedule::Cron { expr })
}

pub(crate) fn next_run_after(schedule: &CronSchedule, now_ms: u64) -> Option<u64> {
    match schedule {
        CronSchedule::Interval { every_ms } => Some(now_ms.saturating_add(*every_ms)),
        CronSchedule::OneShot { at_ms } => (*at_ms > now_ms).then_some(*at_ms),
        CronSchedule::Cron { expr } => next_cron_run(expr, now_ms),
    }
}

fn parse_interval(value: &str) -> Result<u64, String> {
    let value = value.trim();
    let number_len = value.chars().take_while(char::is_ascii_digit).count();
    if number_len == 0 || number_len == value.len() {
        return Err("interval schedule 格式应为 interval:5m".to_owned());
    }
    let number = value[..number_len]
        .parse::<u64>()
        .map_err(|_| "interval schedule 数值无效".to_owned())?;
    if number == 0 {
        return Err("interval schedule 必须大于 0".to_owned());
    }
    match &value[number_len..] {
        "ms" => Ok(number),
        "s" => Ok(number.saturating_mul(MILLIS_PER_SECOND)),
        "m" => Ok(number.saturating_mul(MILLIS_PER_MINUTE)),
        "h" => Ok(number.saturating_mul(MILLIS_PER_HOUR)),
        _ => Err("interval schedule 仅支持 ms、s、m、h 单位".to_owned()),
    }
}

fn parse_local_once_ms(value: &str) -> Result<u64, String> {
    let local = NaiveDateTime::parse_from_str(value.trim(), LOCAL_ONCE_FORMAT)
        .map_err(|_| "一次性 schedule 必须使用本地时间格式 once:YYYY-MM-DDTHH:MM:SS".to_owned())?;
    let resolved = match Local.from_local_datetime(&local) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => return Err("一次性 schedule 对应本地时区不存在的时间".to_owned()),
    };
    u64::try_from(resolved.timestamp_millis())
        .map_err(|_| "一次性 schedule 不能早于 Unix epoch".to_owned())
}

fn parse_cron(value: &str) -> Result<CronExpr, String> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 5 {
        return Err(
            "schedule 仅支持 interval:5m、once:YYYY-MM-DDTHH:MM:SS 或 5 段 cron 表达式".to_owned(),
        );
    }
    Ok(CronExpr {
        minutes: parse_cron_field(parts[0], 0, 59, "分钟")?,
        hours: parse_cron_field(parts[1], 0, 23, "小时")?,
        days: parse_cron_field(parts[2], 1, 31, "日期")?,
        months: parse_cron_field(parts[3], 1, 12, "月份")?,
        weekdays: parse_cron_field(parts[4], 0, 6, "星期")?,
    })
}

fn parse_cron_field(value: &str, min: u32, max: u32, label: &str) -> Result<CronField, String> {
    if value == "*" {
        return Ok(CronField::Any);
    }
    if let Some(step) = value.strip_prefix("*/") {
        let step = step
            .parse::<u32>()
            .map_err(|_| format!("cron {label}步长无效"))?;
        if step == 0 {
            return Err(format!("cron {label}步长必须大于 0"));
        }
        return Ok(CronField::Step(step));
    }
    let mut values = BTreeSet::new();
    for part in value.split(',') {
        let parsed = part
            .parse::<u32>()
            .map_err(|_| format!("cron {label}字段无效"))?;
        if parsed < min || parsed > max {
            return Err(format!("cron {label}字段超出范围"));
        }
        values.insert(parsed);
    }
    Ok(CronField::Values(values))
}

fn next_cron_run(expr: &CronExpr, now_ms: u64) -> Option<u64> {
    let mut candidate = ((now_ms / MILLIS_PER_MINUTE) + 1) * MILLIS_PER_MINUTE;
    for _ in 0..CRON_LOOKAHEAD_MINUTES {
        let utc = DateTime::<Utc>::from_timestamp_millis(i64::try_from(candidate).ok()?)?;
        let local = utc.with_timezone(&Local);
        if expr.matches(&local) {
            return Some(candidate);
        }
        candidate = candidate.saturating_add(MILLIS_PER_MINUTE);
    }
    None
}

impl CronExpr {
    fn matches(&self, local: &DateTime<Local>) -> bool {
        self.minutes.matches(local.minute())
            && self.hours.matches(local.hour())
            && self.days.matches(local.day())
            && self.months.matches(local.month())
            && self
                .weekdays
                .matches(local.weekday().num_days_from_sunday())
    }
}

impl CronField {
    fn matches(&self, value: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Step(step) => value.is_multiple_of(*step),
            Self::Values(values) => values.contains(&value),
        }
    }
}

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
}
