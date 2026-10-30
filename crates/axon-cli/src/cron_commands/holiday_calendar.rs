//! 中国法定节假日日历：从用户级运行态数据目录加载按年发布的放假与调休安排。
//!
//! 数据来自 `holiday-cn` 数据集（按国务院通知整理），由 [`super::holiday_sync`] 负责抓取并落盘。
//! 未同步的年份不做任何节假日假设，直接按周末规则降级，绝不凭猜测生成日期。

use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::{Datelike as _, NaiveDate, Weekday};
use serde::Deserialize;

/// 运行态数据目录下的节假日缓存子目录。
pub(crate) const HOLIDAY_CACHE_DIR: &str = "holiday-calendar";

static INDEX: RwLock<Option<Arc<HolidayIndex>>> = RwLock::new(None);
static LOADED_AT_MS: AtomicU64 = AtomicU64::new(0);

/// 索引在长驻进程内的最长存活时间；超时后重新读盘，保证 CLI 同步的结果对 Daemon 生效。
const INDEX_TTL_MS: u64 = 3_600_000;

/// 解析节假日缓存目录；运行态根目录不可用时返回 `None`。
pub(crate) fn cache_dir() -> Option<PathBuf> {
    axon_config::ConfigLoader::default_user_axon_root_path()
        .ok()
        .map(|root| {
            root.join(axon_config::AXON_RUNTIME_DATA_DIR)
                .join(HOLIDAY_CACHE_DIR)
        })
}

/// 进程内节假日索引：已放假日、调休上班日与已覆盖年份。
#[derive(Debug, Default)]
pub(crate) struct HolidayIndex {
    off_days: HashSet<NaiveDate>,
    workdays: HashSet<NaiveDate>,
    years: BTreeSet<i32>,
}

impl HolidayIndex {
    fn from_entries(
        off_days: impl IntoIterator<Item = NaiveDate>,
        workdays: impl IntoIterator<Item = NaiveDate>,
    ) -> Self {
        let off_days = off_days.into_iter().collect::<HashSet<_>>();
        let workdays = workdays.into_iter().collect::<HashSet<_>>();
        let years = off_days
            .iter()
            .chain(workdays.iter())
            .map(|date| date.year())
            .collect();
        Self {
            off_days,
            workdays,
            years,
        }
    }

    /// 读取缓存目录内全部按年数据文件；任何单个文件损坏都只跳过该文件。
    fn load(dir: Option<&Path>) -> Self {
        let Some(dir) = dir else {
            return Self::default();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Self::default();
        };
        let mut off_days = BTreeSet::new();
        let mut workdays = BTreeSet::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if !is_year_file(&path) {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(source) = serde_json::from_str::<SourceFile>(&raw) else {
                continue;
            };
            for day in source.days {
                let Ok(date) = NaiveDate::parse_from_str(&day.date, "%Y-%m-%d") else {
                    continue;
                };
                if day.is_off_day {
                    off_days.insert(date);
                } else {
                    workdays.insert(date);
                }
            }
        }
        Self::from_entries(off_days, workdays)
    }

    /// 法定节假日：官方公布的放假日，调休上班日不算。
    pub(crate) fn is_holiday(&self, date: NaiveDate) -> bool {
        self.off_days.contains(&date) && !self.workdays.contains(&date)
    }

    /// 工作日：周一至周五，排除法定节假日，包含调休上班日。
    pub(crate) fn is_workday(&self, date: NaiveDate) -> bool {
        if self.workdays.contains(&date) {
            return true;
        }
        if self.off_days.contains(&date) {
            return false;
        }
        !matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// 休息周末：周六周日，调休上班日不算。
    pub(crate) fn is_rest_weekend(&self, date: NaiveDate) -> bool {
        !self.workdays.contains(&date) && matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// 已同步的年份，升序。
    pub(crate) fn covered_years(&self) -> Vec<i32> {
        self.years.iter().copied().collect()
    }

    /// 已载入的放假日总数。
    pub(crate) fn off_days_count(&self) -> usize {
        self.off_days.len()
    }

    /// 已载入的调休上班日总数。
    pub(crate) fn workdays_count(&self) -> usize {
        self.workdays.len()
    }
}

/// 取得进程内索引，首次访问或超过存活时间时从缓存目录加载。
pub(crate) fn index() -> Arc<HolidayIndex> {
    let now_ms = axon_time::unix_now_ms();
    let cached = INDEX
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|_| now_ms.saturating_sub(LOADED_AT_MS.load(Ordering::Acquire)) < INDEX_TTL_MS);
    if let Some(index) = cached {
        return index;
    }
    let loaded = Arc::new(HolidayIndex::load(cache_dir().as_deref()));
    LOADED_AT_MS.store(now_ms, Ordering::Release);
    if let Ok(mut guard) = INDEX.write() {
        *guard = Some(Arc::clone(&loaded));
    }
    loaded
}

/// 丢弃进程内索引，下次访问重新从缓存目录加载。
pub(crate) fn reload() {
    LOADED_AT_MS.store(0, Ordering::Release);
}

/// 法定节假日判定。
pub(crate) fn is_holiday(date: NaiveDate) -> bool {
    index().is_holiday(date)
}

/// 工作日判定。
pub(crate) fn is_workday(date: NaiveDate) -> bool {
    index().is_workday(date)
}

/// 休息周末判定。
pub(crate) fn is_rest_weekend(date: NaiveDate) -> bool {
    index().is_rest_weekend(date)
}

/// 运行态暂无当前年或次年数据时给出可执行的提示。
pub(crate) fn missing_data_hint() -> Option<&'static str> {
    let index = index();
    let year = chrono::Local::now().year();
    if index.years.contains(&year) && index.years.contains(&(year + 1)) {
        return None;
    }
    Some("节假日年历未覆盖当前年或次年，相关计划按周末规则降级；可执行 axon cron holiday sync 拉取")
}

/// 只接受 `<四位年份>.json`，手工备份或临时文件不得混入索引。
fn is_year_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(year) = name.strip_suffix(".json") else {
        return false;
    };
    year.len() == 4 && year.chars().all(|ch| ch.is_ascii_digit())
}

/// 数据源单年文件的结构。
#[derive(Debug, Deserialize)]
struct SourceFile {
    #[serde(default)]
    days: Vec<SourceDay>,
}

#[derive(Debug, Deserialize)]
struct SourceDay {
    date: String,
    #[serde(rename = "isOffDay")]
    is_off_day: bool,
}

/// 供同步模块复用的解析结果。
pub(crate) fn parse_source_days(raw: &str) -> Result<(Vec<NaiveDate>, Vec<NaiveDate>), String> {
    let source: SourceFile =
        serde_json::from_str(raw).map_err(|_| "节假日数据不是合法 JSON".to_owned())?;
    if source.days.is_empty() {
        return Err("节假日数据缺少 days 字段".to_owned());
    }
    let mut off_days = Vec::new();
    let mut workdays = Vec::new();
    for day in source.days {
        let date = NaiveDate::parse_from_str(&day.date, "%Y-%m-%d")
            .map_err(|_| format!("节假日日期格式无效：{}", day.date))?;
        if day.is_off_day {
            off_days.push(date);
        } else {
            workdays.push(date);
        }
    }
    Ok((off_days, workdays))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("测试日期应有效")
    }

    fn sample_index() -> HolidayIndex {
        HolidayIndex::from_entries(
            [date(2026, 10, 1), date(2026, 10, 2), date(2026, 2, 16)],
            [date(2026, 10, 10), date(2026, 2, 14)],
        )
    }

    #[test]
    fn holiday_and_workday_follow_published_calendar() {
        let index = sample_index();
        assert!(index.is_holiday(date(2026, 10, 1)));
        assert!(!index.is_holiday(date(2026, 10, 10)));
        assert!(index.is_workday(date(2026, 10, 10)));
        assert!(!index.is_rest_weekend(date(2026, 10, 10)));
        assert!(index.is_workday(date(2026, 10, 9)));
        assert!(!index.is_workday(date(2026, 10, 1)));
    }

    #[test]
    fn unsynced_year_degrades_to_weekend_rule() {
        let index = sample_index();
        assert!(!index.is_workday(date(2030, 3, 9)));
        assert!(index.is_workday(date(2030, 3, 5)));
        assert!(!index.is_holiday(date(2030, 10, 1)));
        assert_eq!(index.covered_years(), vec![2026]);
    }

    #[test]
    fn source_payload_splits_off_days_and_makeup_workdays() {
        let raw = r#"{"year":2026,"days":[
            {"name":"元旦","date":"2026-01-01","isOffDay":true},
            {"name":"春节","date":"2026-02-14","isOffDay":false}
        ]}"#;
        let (off_days, workdays) = parse_source_days(raw).expect("数据应可解析");
        assert_eq!(off_days, vec![date(2026, 1, 1)]);
        assert_eq!(workdays, vec![date(2026, 2, 14)]);
        assert!(parse_source_days(r#"{"days":[]}"#).is_err());
    }

    #[test]
    fn only_year_named_files_are_loaded() {
        let dir = axon_test_support::test_temp_dir();
        std::fs::write(
            dir.path().join("2026.json"),
            r#"{"days":[{"date":"2026-10-01","isOffDay":true}]}"#,
        )
        .expect("写入应成功");
        std::fs::write(
            dir.path().join("2026-backup.json"),
            r#"{"days":[{"date":"2026-12-25","isOffDay":true}]}"#,
        )
        .expect("写入应成功");
        std::fs::write(dir.path().join("2027.json"), "not json").expect("写入应成功");

        let index = HolidayIndex::load(Some(dir.path()));
        assert!(index.is_holiday(date(2026, 10, 1)));
        assert!(!index.is_holiday(date(2026, 12, 25)), "非年份文件不应加载");
        assert_eq!(index.covered_years(), vec![2026], "坏 JSON 不应进入索引");
    }
}
