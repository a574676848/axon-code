use std::collections::BTreeSet;

use chrono::{
    DateTime, Datelike as _, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone as _,
    Timelike as _, Utc, Weekday,
};

use super::holiday_calendar;

const MILLIS_PER_SECOND: u64 = 1_000;
const MILLIS_PER_MINUTE: u64 = 60 * MILLIS_PER_SECOND;
const MILLIS_PER_HOUR: u64 = 60 * MILLIS_PER_MINUTE;
const MINUTES_PER_HOUR: u32 = 60;
const CRON_LOOKAHEAD_MINUTES: u64 = 5 * 366 * 24 * 60;
const LOCAL_ONCE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";
const ALIAS_HINT: &str = "@hourly、@daily、@midnight、@weekly、@monthly、@yearly、@annually";

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

    /// 是否依赖节假日年历判定；weekday 只看星期，不读年历。
    pub(super) fn needs_holiday_calendar(&self) -> bool {
        match self {
            Self::Cron { expr } => expr.calendar.needs_calendar_data(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CronExpr {
    minutes: CronField,
    hours: CronField,
    days: CronField,
    months: CronField,
    weekdays: CronField,
    calendar: CalendarFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CronField {
    Any,
    Step { start: u32, step: u32 },
    Values(BTreeSet<u32>),
}

/// 日历过滤：在五段 cron 之上再限定只在某类日子触发。
///
/// 只由显式前缀开启，任何前缀都能与任意五段 cron 组合，不存在预置组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalendarFilter {
    Any,
    Workday,
    Weekday,
    Holiday,
    Weekend,
}

impl CalendarFilter {
    const fn needs_calendar_data(self) -> bool {
        matches!(self, Self::Workday | Self::Holiday | Self::Weekend)
    }

    fn matches(self, date: NaiveDate) -> bool {
        match self {
            Self::Any => true,
            Self::Workday => holiday_calendar::is_workday(date),
            Self::Weekday => matches!(
                date.weekday(),
                Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu | Weekday::Fri
            ),
            Self::Holiday => holiday_calendar::is_holiday(date),
            Self::Weekend => holiday_calendar::is_rest_weekend(date),
        }
    }
}

pub(crate) fn parse_schedule(value: &str) -> Result<CronSchedule, String> {
    let value = value.trim();
    if let Some(raw) = value.strip_prefix("interval:") {
        return parse_interval(raw).map(|every_ms| CronSchedule::Interval { every_ms });
    }
    if let Some(raw) = value.strip_prefix("once:") {
        return parse_local_once_ms(raw).map(|at_ms| CronSchedule::OneShot { at_ms });
    }
    let (calendar, remainder) =
        split_calendar_prefix(value).unwrap_or((CalendarFilter::Any, value));
    let mut expr = if remainder.starts_with('@') {
        parse_alias(remainder)?
    } else {
        parse_cron(remainder)?
    };
    expr.calendar = calendar;
    Ok(CronSchedule::Cron { expr })
}

/// 拆出日历前缀，剩余部分原样交给五段 cron 或标准别名解析。
fn split_calendar_prefix(value: &str) -> Option<(CalendarFilter, &str)> {
    let (name, remainder) = value.split_once(':')?;
    let calendar = match name {
        "workday" => CalendarFilter::Workday,
        "weekday" => CalendarFilter::Weekday,
        "holiday" => CalendarFilter::Holiday,
        "weekend" => CalendarFilter::Weekend,
        _ => return None,
    };
    Some((calendar, remainder.trim()))
}

pub(crate) fn next_run_after(schedule: &CronSchedule, now_ms: u64) -> Option<u64> {
    match schedule {
        CronSchedule::Interval { every_ms } => Some(now_ms.saturating_add(*every_ms)),
        CronSchedule::OneShot { at_ms } => (*at_ms > now_ms).then_some(*at_ms),
        CronSchedule::Cron { expr } => next_cron_run(expr, now_ms),
    }
}

/// 已知原触发点时计算下一触发点。
///
/// 间隔计划必须按**原触发点**对齐，不能用「现在 + 间隔」：后者是固定延迟，任务体耗时会
/// 永久累积成漂移（`interval:5m` 的任务跑 1 分钟，下一轮就落在 6 分钟后）。按整数个间隔
/// 从原触发点推进到第一个未来时刻，漂移不跨轮累积。
pub(crate) fn next_run_from(
    schedule: &CronSchedule,
    scheduled_at_ms: u64,
    now_ms: u64,
) -> Option<u64> {
    match schedule {
        CronSchedule::Interval { every_ms } => {
            Some(next_interval_run(scheduled_at_ms, *every_ms, now_ms))
        }
        other => next_run_after(other, now_ms),
    }
}

/// 从 `scheduled_at_ms` 按整数个 `every_ms` 推进到的第一个严格未来时刻。
fn next_interval_run(scheduled_at_ms: u64, every_ms: u64, now_ms: u64) -> u64 {
    if every_ms == 0 {
        return now_ms.saturating_add(1);
    }
    if now_ms < scheduled_at_ms {
        return scheduled_at_ms;
    }
    let elapsed = now_ms.saturating_sub(scheduled_at_ms);
    let steps = elapsed / every_ms + 1;
    scheduled_at_ms.saturating_add(steps.saturating_mul(every_ms))
}

/// 计算下一次执行；无解时给出可执行错误，避免落库成永不触发的任务。
pub(crate) fn require_next_run(schedule: &CronSchedule, now_ms: u64) -> Result<u64, String> {
    next_run_after(schedule, now_ms).ok_or_else(|| {
        if schedule.is_one_shot() {
            "一次性计划的时间必须晚于当前时间".to_owned()
        } else if schedule.needs_holiday_calendar() {
            "该计划按节假日年历判定，但年历未覆盖所需年份；请先执行 axon cron holiday sync"
                .to_owned()
        } else {
            "该计划在五年内没有可触发时间，请检查日期、月份与星期组合".to_owned()
        }
    })
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
            "schedule 仅支持 interval:5m、once:YYYY-MM-DDTHH:MM:SS、预设别名或 5 段 cron 表达式"
                .to_owned(),
        );
    }
    Ok(CronExpr {
        minutes: parse_cron_field(parts[0], 0, 59, "分钟")?,
        hours: parse_cron_field(parts[1], 0, 23, "小时")?,
        days: parse_cron_field(parts[2], 1, 31, "日期")?,
        months: parse_cron_field(parts[3], 1, 12, "月份")?,
        weekdays: parse_cron_field(parts[4], 0, 6, "星期")?,
        calendar: CalendarFilter::Any,
    })
}

fn parse_cron_field(value: &str, min: u32, max: u32, label: &str) -> Result<CronField, String> {
    if value == "*" || value == "?" {
        return Ok(CronField::Any);
    }
    if let Some(step) = value.strip_prefix("*/") {
        return Ok(CronField::Step {
            start: min,
            step: parse_step(step, label)?,
        });
    }
    let mut values = BTreeSet::new();
    for part in value.split(',') {
        parse_cron_field_part(part, min, max, label, &mut values)?;
    }
    if values.is_empty() {
        return Err(format!("cron {label}字段为空"));
    }
    Ok(CronField::Values(values))
}

fn parse_cron_field_part(
    part: &str,
    min: u32,
    max: u32,
    label: &str,
    values: &mut BTreeSet<u32>,
) -> Result<(), String> {
    let (range, step) = match part.split_once('/') {
        Some((range, step)) => (range, Some(parse_step(step, label)?)),
        None => (part, None),
    };
    let (start, end) = match range.split_once('-') {
        Some((start, end)) => (
            parse_bound(start, min, max, label)?,
            parse_bound(end, min, max, label)?,
        ),
        None => {
            let single = parse_bound(range, min, max, label)?;
            match step {
                Some(_) => (single, max),
                None => {
                    values.insert(single);
                    return Ok(());
                }
            }
        }
    };
    if start > end {
        return Err(format!("cron {label}字段区间起点不能大于终点"));
    }
    for value in (start..=end).step_by(step.unwrap_or(1) as usize) {
        values.insert(value);
    }
    Ok(())
}

fn parse_bound(value: &str, min: u32, max: u32, label: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("cron {label}字段 {value} 无效"))?;
    if parsed < min || parsed > max {
        return Err(format!(
            "cron {label}字段 {parsed} 超出范围（合法 {min} 到 {max}）"
        ));
    }
    Ok(parsed)
}

fn parse_step(value: &str, label: &str) -> Result<u32, String> {
    let step = value
        .parse::<u32>()
        .map_err(|_| format!("cron {label}步长 {value} 无效"))?;
    if step == 0 {
        return Err(format!("cron {label}步长 {value} 必须大于 0"));
    }
    Ok(step)
}

/// 标准别名：只做时间压缩，不携带任何日历语义。
struct AliasSpec {
    minute: u32,
    hour: Option<u32>,
    day: &'static str,
    month: &'static str,
    weekday: &'static str,
}

impl AliasSpec {
    fn expression(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.minute,
            self.hour
                .map_or_else(|| "*".to_owned(), |value| value.to_string()),
            self.day,
            self.month,
            self.weekday
        )
    }
}

fn parse_alias(value: &str) -> Result<CronExpr, String> {
    let spec = alias_spec(value)
        .ok_or_else(|| format!("schedule 不支持的预设 {value}，可用 {ALIAS_HINT}"))?;
    parse_cron(&spec.expression())
}

fn alias_spec(name: &str) -> Option<AliasSpec> {
    let spec = match name {
        "@hourly" => AliasSpec {
            minute: 0,
            hour: None,
            day: "*",
            month: "*",
            weekday: "*",
        },
        "@daily" | "@midnight" => AliasSpec {
            minute: 0,
            hour: Some(0),
            day: "*",
            month: "*",
            weekday: "*",
        },
        "@weekly" => AliasSpec {
            minute: 0,
            hour: Some(0),
            day: "*",
            month: "*",
            weekday: "0",
        },
        "@monthly" => AliasSpec {
            minute: 0,
            hour: Some(0),
            day: "1",
            month: "*",
            weekday: "*",
        },
        "@yearly" | "@annually" => AliasSpec {
            minute: 0,
            hour: Some(0),
            day: "1",
            month: "1",
            weekday: "*",
        },
        _ => return None,
    };
    Some(spec)
}

/// 逐级跳跃定位下一次执行：月不匹配就跳月，日不匹配就跳日，时不匹配就跳时。
///
/// 线性逐分钟扫描最坏要走完一年（约 52 万次时区转换），这里把每次迭代压缩到
/// 「天」或「小时」量级，常规表达式只需数百次比较。
fn next_cron_run(expr: &CronExpr, now_ms: u64) -> Option<u64> {
    let mut candidate = ((now_ms / MILLIS_PER_MINUTE) + 1) * MILLIS_PER_MINUTE;
    let deadline = now_ms.saturating_add(CRON_LOOKAHEAD_MINUTES * MILLIS_PER_MINUTE);
    while candidate <= deadline {
        let utc = DateTime::<Utc>::from_timestamp_millis(i64::try_from(candidate).ok()?)?;
        let local = utc.with_timezone(&Local);
        if !expr.months.matches(local.month()) {
            candidate = advance(candidate, next_month_start_ms(&local));
            continue;
        }
        if !expr.date_matches(&local) {
            candidate = advance(candidate, next_day_start_ms(&local));
            continue;
        }
        if !expr.hours.matches(local.hour()) {
            let skip = u64::from(MINUTES_PER_HOUR - local.minute()) * MILLIS_PER_MINUTE;
            candidate = advance(candidate, candidate.checked_add(skip));
            continue;
        }
        if !expr.minutes.matches(local.minute()) {
            candidate = advance(candidate, candidate.checked_add(MILLIS_PER_MINUTE));
            continue;
        }
        return Some(candidate);
    }
    None
}

/// 保证候选时间严格前进；任何跳转失败都退化为下一分钟，避免死循环。
fn advance(candidate: u64, next: Option<u64>) -> u64 {
    next.unwrap_or_else(|| candidate.saturating_add(MILLIS_PER_MINUTE))
        .max(candidate.saturating_add(MILLIS_PER_MINUTE))
}

fn next_day_start_ms(local: &DateTime<Local>) -> Option<u64> {
    local_ms(local.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?)
}

fn next_month_start_ms(local: &DateTime<Local>) -> Option<u64> {
    let (year, month) = (local.year(), local.month());
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    local_ms(NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)?)
}

fn local_ms(naive: NaiveDateTime) -> Option<u64> {
    let resolved = match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => return None,
    };
    u64::try_from(resolved.timestamp_millis()).ok()
}

impl CronExpr {
    /// 日期维度：日与星期按标准 cron 的「任一匹配」规则，再由日历过滤收窄。
    fn date_matches(&self, local: &DateTime<Local>) -> bool {
        let day_matches = self.days.matches(local.day());
        let weekday_matches = self
            .weekdays
            .matches(local.weekday().num_days_from_sunday());
        let day_ok = match (self.days.is_any(), self.weekdays.is_any()) {
            (true, true) => true,
            (true, false) => weekday_matches,
            (false, true) => day_matches,
            (false, false) => day_matches || weekday_matches,
        };
        day_ok && self.calendar.matches(local.date_naive())
    }
}

impl CronField {
    fn matches(&self, value: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Step { start, step } => value >= *start && (value - *start).is_multiple_of(*step),
            Self::Values(values) => values.contains(&value),
        }
    }

    /// 字段是否未做任何限定。
    const fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
