//! Axon 日志系统。

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::PathBuf,
    str::FromStr,
    sync::{LazyLock, Mutex},
};

use axon_domain::AxonResult;
use chrono::Local;

/// 日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Debug 级别。
    Debug = 0,
    /// Info 级别。
    Info = 1,
    /// Warning 级别。
    Warning = 2,
    /// Error 级别。
    Error = 3,
}

impl LogLevel {
    /// 返回日志级别字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
        }
    }
}

impl FromStr for LogLevel {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_lowercase().as_str() {
            // 记录器只实现 debug/info/warning/error 四档，文档中的 trace 按最详细档接受。
            "trace" | "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warning" | "warn" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            _ => Err(()),
        }
    }
}

/// 日志记录器配置。
#[derive(Debug, Clone)]
pub struct LoggerConfig {
    /// 日志级别。
    pub level: LogLevel,
    /// 日志目录；未显式指定文件时按日期在其中滚动。
    pub log_dir: PathBuf,
    /// 显式日志文件路径；为空时按日期滚动到 `log_dir`。
    pub log_file: Option<PathBuf>,
}

/// 全局日志记录器。
static LOGGER: LazyLock<Mutex<Option<Logger>>> = LazyLock::new(|| Mutex::new(None));

/// 日志记录器。
#[derive(Debug)]
struct Logger {
    config: LoggerConfig,
    current_log_file: Option<PathBuf>,
}

impl Logger {
    fn new(config: LoggerConfig) -> Self {
        Self {
            config,
            current_log_file: None,
        }
    }

    fn log(&mut self, level: LogLevel, message: &str) {
        if level < self.config.level {
            return;
        }

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let log_line = format!("[{}] [{}] {}\n", timestamp, level.as_str(), message);

        let _ = self.write_to_file(&log_line);
    }

    fn write_to_file(&mut self, content: &str) -> AxonResult<()> {
        let log_file = self.resolve_log_file()?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|_| axon_domain::AxonError::runtime_failure("无法打开日志文件"))?;

        file.write_all(content.as_bytes())
            .map_err(|_| axon_domain::AxonError::runtime_failure("无法写入日志"))?;

        Ok(())
    }

    fn resolve_log_file(&mut self) -> AxonResult<PathBuf> {
        let log_file = self.config.log_file.clone().unwrap_or_else(|| {
            let today = Local::now().format("%Y-%m-%d").to_string();
            self.config.log_dir.join(format!("axon-{today}.log"))
        });

        if self.current_log_file.as_ref() != Some(&log_file) {
            let directory = log_file
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(&self.config.log_dir);
            fs::create_dir_all(directory)
                .map_err(|_| axon_domain::AxonError::runtime_failure("无法创建日志目录"))?;
            self.current_log_file = Some(log_file.clone());
        }

        Ok(log_file)
    }
}

/// 初始化日志记录器。
pub fn init_logger(config: LoggerConfig) -> AxonResult<()> {
    let mut logger = LOGGER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *logger = Some(Logger::new(config));
    Ok(())
}

/// 记录 debug 日志。
pub fn debug(message: &str) {
    log_message(LogLevel::Debug, message);
}

/// 记录 info 日志。
pub fn info(message: &str) {
    log_message(LogLevel::Info, message);
}

/// 记录 warning 日志。
pub fn warning(message: &str) {
    log_message(LogLevel::Warning, message);
}

/// 记录 error 日志。
pub fn error(message: &str) {
    log_message(LogLevel::Error, message);
}

fn log_message(level: LogLevel, message: &str) {
    let mut logger = LOGGER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(logger) = logger.as_mut() {
        logger.log(level, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_parsing() {
        assert_eq!("debug".parse::<LogLevel>(), Ok(LogLevel::Debug));
        assert_eq!("DEBUG".parse::<LogLevel>(), Ok(LogLevel::Debug));
        assert_eq!("trace".parse::<LogLevel>(), Ok(LogLevel::Debug));
        assert_eq!("info".parse::<LogLevel>(), Ok(LogLevel::Info));
        assert_eq!("warning".parse::<LogLevel>(), Ok(LogLevel::Warning));
        assert_eq!("warn".parse::<LogLevel>(), Ok(LogLevel::Warning));
        assert_eq!("error".parse::<LogLevel>(), Ok(LogLevel::Error));
        assert!("invalid".parse::<LogLevel>().is_err());
    }

    #[test]
    fn log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Error);
    }

    #[test]
    fn logger_filters_by_level() {
        let temp_dir = axon_test_support::test_temp_dir();
        let config = LoggerConfig {
            level: LogLevel::Warning,
            log_dir: temp_dir.path().to_path_buf(),
            log_file: None,
        };

        init_logger(config).unwrap();

        debug("debug message");
        info("info message");
        warning("warning message");
        error("error message");

        let today = Local::now().format("%Y-%m-%d").to_string();
        let log_file = temp_dir.path().join(format!("axon-{}.log", today));
        let content = fs::read_to_string(log_file).unwrap();

        assert!(!content.contains("debug message"));
        assert!(!content.contains("info message"));
        assert!(content.contains("warning message"));
        assert!(content.contains("error message"));
    }

    #[test]
    fn explicit_log_file_overrides_rotating_name() {
        let temp_dir = axon_test_support::test_temp_dir();
        let log_file = temp_dir.path().join("nested").join("axon-explicit.log");
        let mut logger = Logger::new(LoggerConfig {
            level: LogLevel::Debug,
            log_dir: temp_dir.path().join("unused"),
            log_file: Some(log_file.clone()),
        });

        logger.log(LogLevel::Debug, "explicit file message");

        let content = fs::read_to_string(log_file).unwrap();
        assert!(content.contains("explicit file message"));
    }

    #[test]
    fn log_file_falls_back_to_daily_rotation_in_log_dir() {
        let temp_dir = axon_test_support::test_temp_dir();
        let mut logger = Logger::new(LoggerConfig {
            level: LogLevel::Debug,
            log_dir: temp_dir.path().to_path_buf(),
            log_file: None,
        });

        logger.log(LogLevel::Debug, "rotating message");

        let today = Local::now().format("%Y-%m-%d").to_string();
        let rotated = temp_dir.path().join(format!("axon-{today}.log"));
        assert!(fs::read_to_string(rotated)
            .unwrap()
            .contains("rotating message"));
    }
}
