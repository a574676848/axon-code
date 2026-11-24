//! 运行态库的行解码错误口径。
//!
//! 以前各处 decode 失败一律回 `rusqlite::Error::InvalidQuery`：它的 Display 是
//! 「Query is not read-only」，与真实原因（某一列的取值不认识）毫无关系，
//! 用户看不懂，排查也只能靠猜。
//!
//! 统一走这里：**对外只给一句人话**，字段名与原始取值写日志并留在 `Debug` 里。
//! `tools/verify-architecture.py` 有一条门禁禁止运行态库里再出现 `InvalidQuery`。

use std::fmt;

/// 行解码失败。
///
/// `field` 用「表.列」的形态，日志里据此定位；`value` 传原始取值，传不出取值时传
/// 解析错误本身，二者都能让日志自证。
pub(super) fn decode_error(field: &str, value: impl fmt::Display) -> rusqlite::Error {
    let value = value.to_string();
    axon_logger::error(&format!("运行态库行解码失败: field={field}, value={value}"));
    rusqlite::Error::ToSqlConversionFailure(Box::new(UnrecognizedColumnValue {
        field: field.to_owned(),
        value,
    }))
}

/// JSON 列解码失败。
pub(super) fn decode_json<T: serde::de::DeserializeOwned>(
    raw: String,
    field: &str,
) -> rusqlite::Result<T> {
    serde_json::from_str(&raw).map_err(|error| decode_error(field, error))
}

/// 无法识别的列取值。
struct UnrecognizedColumnValue {
    field: String,
    value: String,
}

impl fmt::Debug for UnrecognizedColumnValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 原始字段与取值只服务日志取证，不进用户可见文案。
        write!(
            formatter,
            "UnrecognizedColumnValue {{ field: {:?}, value: {:?} }}",
            self.field, self.value
        )
    }
}

impl fmt::Display for UnrecognizedColumnValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("本地运行态数据中存在无法识别的记录，可能由其他版本的程序写入")
    }
}

impl std::error::Error for UnrecognizedColumnValue {}

#[cfg(test)]
mod tests {
    use super::{decode_error, decode_json};

    #[test]
    fn decode_error_hides_raw_value_from_display_and_keeps_it_in_debug() {
        let error = decode_error("inbox.interaction_id", "bad-value");

        let message = error.to_string();
        assert!(
            message.contains("无法识别的记录"),
            "用户可见文案必须是人话：{message}"
        );
        assert!(
            !message.contains("bad-value"),
            "原始取值不得进入用户可见文案：{message}"
        );
        assert!(
            !message.contains("read-only"),
            "不得回 rusqlite 的误导文案：{message}"
        );
        let debug = format!("{error:?}");
        assert!(
            debug.contains("bad-value") && debug.contains("inbox.interaction_id"),
            "Debug 必须保留字段与取值供日志取证：{debug}"
        );
    }

    #[test]
    fn decode_json_names_the_column_and_keeps_parser_detail() {
        let error = decode_json::<serde_json::Value>("{oops".to_owned(), "goals.plan_json")
            .expect_err("非法 JSON 必须失败");

        assert!(error.to_string().contains("无法识别的记录"));
        let debug = format!("{error:?}");
        assert!(
            debug.contains("goals.plan_json"),
            "Debug 必须带列名：{debug}"
        );
    }
}
