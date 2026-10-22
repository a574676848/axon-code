//! Agent TUI 时长格式化。

use std::time::Duration;

use axon_time::duration_ms;

/// 将运行耗时压缩为适合输入区下方状态行的中文短文本。
#[must_use]
pub(super) fn format_elapsed(duration: Duration) -> String {
    format_duration_millis_compact(duration_ms(duration))
}

/// 将毫秒时长压缩为适合 transcript/status 的短文本。
#[must_use]
pub(crate) fn format_duration_millis_compact(duration_ms: u64) -> String {
    super::display_units::format_duration_millis_compact(duration_ms)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::format_elapsed;

    #[test]
    fn elapsed_uses_same_unit_conversion() {
        assert_eq!(format_elapsed(Duration::from_millis(1_250)), "1.25s");
    }
}
