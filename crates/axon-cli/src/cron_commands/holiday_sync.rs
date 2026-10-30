//! 节假日日历同步：从 holiday-cn 数据集抓取按年安排并写入运行态缓存。
//!
//! 单年失败只影响该年，其余年份照常落盘；抓取结果会立即触发日历索引重载。

use std::{fs, io::Write as _, path::Path, time::Duration};

use chrono::Datelike as _;

use axon_rustls_provider::ensure_rustls_crypto_provider;

use super::holiday_calendar;

/// 数据源单年文件地址模板。
pub(crate) const SOURCE_URL_TEMPLATE: &str =
    "https://raw.githubusercontent.com/NateScarlet/holiday-cn/master/{year}.json";
/// 单年抓取超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// 单年同步结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct YearSyncOutcome {
    /// 年份。
    pub(crate) year: i32,
    /// 写入的放假日数量。
    pub(crate) off_days: usize,
    /// 写入的调休上班日数量。
    pub(crate) workdays: usize,
    /// 缓存文件路径。
    pub(crate) cache_path: String,
}

/// 默认同步范围：当前年与次一年。
pub(crate) fn default_years() -> Vec<i32> {
    let year = chrono::Local::now().year();
    vec![year, year + 1]
}

/// 逐年同步，返回每年一条结果；单年失败不阻断其余年份。
pub(crate) fn sync_years(years: &[i32]) -> Vec<(i32, Result<YearSyncOutcome, String>)> {
    let Some(dir) = holiday_calendar::cache_dir() else {
        return years
            .iter()
            .map(|year| (*year, Err("无法解析运行态数据目录".to_owned())))
            .collect();
    };
    if let Err(error) = fs::create_dir_all(&dir) {
        return years
            .iter()
            .map(|year| (*year, Err(format!("创建节假日缓存目录失败：{error}"))))
            .collect();
    }
    ensure_rustls_crypto_provider();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into();
    let mut outcomes = Vec::with_capacity(years.len());
    for year in years {
        outcomes.push((*year, sync_one(&agent, &dir, *year)));
    }
    holiday_calendar::reload();
    outcomes
}

fn sync_one(agent: &ureq::Agent, dir: &Path, year: i32) -> Result<YearSyncOutcome, String> {
    let url = SOURCE_URL_TEMPLATE.replace("{year}", &year.to_string());
    let body = agent
        .get(&url)
        .call()
        .map_err(|error| format!("抓取 {year} 年节假日失败：{error}"))?
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("读取 {year} 年节假日响应失败：{error}"))?;
    let (off_days, workdays) = holiday_calendar::parse_source_days(&body)?;
    let cache_path = dir.join(format!("{year}.json"));
    write_atomically(&cache_path, &body)?;
    Ok(YearSyncOutcome {
        year,
        off_days: off_days.len(),
        workdays: workdays.len(),
        cache_path: cache_path.display().to_string(),
    })
}

fn write_atomically(target: &Path, body: &str) -> Result<(), String> {
    let temp = target.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut file =
        fs::File::create(&temp).map_err(|error| format!("写入节假日缓存失败：{error}"))?;
    file.write_all(body.as_bytes())
        .map_err(|error| format!("写入节假日缓存失败：{error}"))?;
    file.sync_all()
        .map_err(|error| format!("刷新节假日缓存失败：{error}"))?;
    drop(file);
    fs::rename(&temp, target).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("替换节假日缓存失败：{error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_years_covers_current_and_next() {
        let years = default_years();
        assert_eq!(years.len(), 2);
        assert_eq!(years[1], years[0] + 1);
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = axon_test_support::test_temp_dir();
        let target = dir.path().join("2026.json");
        write_atomically(&target, r#"{"days":[]}"#).expect("写入应成功");

        assert!(target.exists());
        let leftovers = std::fs::read_dir(dir.path())
            .expect("目录应可读")
            .flatten()
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("tmp")
            })
            .count();
        assert_eq!(leftovers, 0, "原子写不应残留临时文件");
        assert_eq!(
            std::fs::read_to_string(&target).expect("读取应成功"),
            r#"{"days":[]}"#
        );
    }
}
