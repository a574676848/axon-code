//! `axon cron holiday` 子命令：同步与查看法定节假日年历。

use std::fmt::Write as _;

use serde::Serialize;

use crate::{CliRunResult, OutputFormat, management_commands::json_result};

use super::{
    args::{CronHolidayAction, CronHolidayCommand, CronHolidaySyncCommand},
    holiday_calendar, holiday_sync,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HolidayStatusView {
    source: &'static str,
    cache_dir: Option<String>,
    covered_years: Vec<i32>,
    off_days: usize,
    workdays: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HolidaySyncView {
    source: &'static str,
    cache_dir: Option<String>,
    years: Vec<YearSyncView>,
    failures: Vec<YearFailureView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YearSyncView {
    year: i32,
    off_days: usize,
    workdays: usize,
    cache_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct YearFailureView {
    year: i32,
    error: String,
}

pub(super) fn run_holiday(command: &CronHolidayCommand, format: OutputFormat) -> CliRunResult {
    match &command.action {
        CronHolidayAction::Sync(command) => sync(command, format),
        CronHolidayAction::Status => status(format),
    }
}

fn status(format: OutputFormat) -> CliRunResult {
    let index = holiday_calendar::index();
    let view = HolidayStatusView {
        source: holiday_sync::SOURCE_URL_TEMPLATE,
        cache_dir: holiday_calendar::cache_dir().map(|path| path.display().to_string()),
        covered_years: index.covered_years(),
        off_days: index.off_days_count(),
        workdays: index.workdays_count(),
    };
    if matches!(format, OutputFormat::Json) {
        return json_result(&view);
    }
    let mut stdout = format!(
        "Axon cron holiday\n- source: {}\n- cache_dir: {}\n- covered_years: {:?}\n- off_days: {}\n- workdays: {}\n",
        view.source,
        view.cache_dir.as_deref().unwrap_or("unresolved"),
        view.covered_years,
        view.off_days,
        view.workdays
    );
    if let Some(hint) = holiday_calendar::missing_data_hint() {
        let _ = writeln!(stdout, "- hint: {hint}");
    }
    CliRunResult {
        exit_code: 0,
        stdout,
        stderr: String::new(),
    }
}

fn sync(command: &CronHolidaySyncCommand, format: OutputFormat) -> CliRunResult {
    let years = if command.years.is_empty() {
        holiday_sync::default_years()
    } else {
        command.years.clone()
    };
    let mut view = HolidaySyncView {
        source: holiday_sync::SOURCE_URL_TEMPLATE,
        cache_dir: holiday_calendar::cache_dir().map(|path| path.display().to_string()),
        years: Vec::new(),
        failures: Vec::new(),
    };
    for (year, outcome) in holiday_sync::sync_years(&years) {
        match outcome {
            Ok(outcome) => view.years.push(YearSyncView {
                year: outcome.year,
                off_days: outcome.off_days,
                workdays: outcome.workdays,
                cache_path: outcome.cache_path,
            }),
            Err(error) => view.failures.push(YearFailureView { year, error }),
        }
    }
    let failed = !view.failures.is_empty();
    if matches!(format, OutputFormat::Json) {
        let mut result = json_result(&view);
        if failed {
            result.exit_code = 1;
        }
        return result;
    }
    let mut stdout = format!(
        "Axon cron holiday sync\n- source: {}\n- cache_dir: {}\n",
        view.source,
        view.cache_dir.as_deref().unwrap_or("unresolved")
    );
    for year in &view.years {
        let _ = writeln!(
            stdout,
            "synced\t{}\toff_days={}\tworkdays={}\t{}",
            year.year, year.off_days, year.workdays, year.cache_path
        );
    }
    for failure in &view.failures {
        let _ = writeln!(stdout, "failed\t{}\t{}", failure.year, failure.error);
    }
    CliRunResult {
        exit_code: if failed { 1 } else { 0 },
        stdout,
        stderr: String::new(),
    }
}
