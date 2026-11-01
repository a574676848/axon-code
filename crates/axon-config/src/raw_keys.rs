//! 原始配置键的轻量读取。
//!
//! 桌面端的连接端点解析在**每次请求前**都要重新确认网关监听地址：它必须只读
//! `remote.gateway.http_bind` 与 `remote.gateway.websocket_bind` 这两个键，不能走
//! [`crate::ConfigLoader::load_from_path`] 那条「解析全文 + 归一化 + 全量校验」的重路径。
//! 每次请求做一次全量配置校验会让端点解析变成请求路径上最贵的一步，而且校验失败
//! 会让一条本来只是「读端口」的请求整条失败。
//!
//! 本模块因此只做两件事：读文件文本、按点分路径取值。它**不**做默认值填充、
//! 不做派生归一化、不做合法性校验，也不碰任何凭据。需要完整配置的调用方仍然走
//! [`crate::ConfigLoader`]。

use std::fs;
use std::path::Path;

use toml::Value;

/// 一次读取多个点分键，返回与 `dotted_keys` 一一对应的取值。
///
/// 文件缺失、读不出来或 TOML 解析失败时，全部返回 `None`：端点解析属于后台行为，
/// 一次读盘失败只应让调用方落到兜底常量，不应让界面弹错误。
#[must_use]
pub fn raw_string_values(path: &Path, dotted_keys: &[&str]) -> Vec<Option<String>> {
    let Ok(text) = fs::read_to_string(path) else {
        return vec![None; dotted_keys.len()];
    };
    let Ok(document) = toml::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
        return vec![None; dotted_keys.len()];
    };
    dotted_keys
        .iter()
        .map(|key| lookup(&document, key))
        .collect()
}

/// 读取单个点分键的字符串取值。
#[must_use]
pub fn raw_string_value(path: &Path, dotted_key: &str) -> Option<String> {
    raw_string_values(path, &[dotted_key])
        .into_iter()
        .next()
        .flatten()
}

/// 按点分路径逐段下钻；只接受字符串叶子，其它类型一律视为缺失。
fn lookup(document: &Value, dotted_key: &str) -> Option<String> {
    let mut current = document;
    for segment in dotted_key.split('.') {
        current = current.get(segment)?;
    }
    match current {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{lookup, raw_string_values};
    use std::fs;
    use std::path::PathBuf;

    const SAMPLE: &str = r#"
[remote.gateway]
http_bind = "127.0.0.1:4242"
websocket_bind = "127.0.0.1:4243"
api_token_env = "AXON_GATEWAY_API_TOKEN"

[agent]
log_level = "info"
"#;

    /// 建一个只在测试期存在的临时目录；用例结束时调用方自行清理。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "axon-raw-keys-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("临时目录应可创建");
        dir
    }

    #[test]
    fn nested_string_values_are_read_without_full_config_loading() {
        let dir = temp_dir("read");
        let path = dir.join("config.toml");
        fs::write(&path, SAMPLE).expect("样例配置应可写入");

        let values = raw_string_values(
            &path,
            &[
                "remote.gateway.http_bind",
                "remote.gateway.websocket_bind",
                "remote.gateway.missing",
            ],
        );
        assert_eq!(values[0].as_deref(), Some("127.0.0.1:4242"));
        assert_eq!(values[1].as_deref(), Some("127.0.0.1:4243"));
        assert_eq!(values[2], None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_and_broken_toml_are_both_treated_as_missing() {
        let dir = temp_dir("missing");
        let path = dir.join("config.toml");

        // 文件不存在：全部 None。
        assert_eq!(
            raw_string_values(&path, &["remote.gateway.http_bind"]),
            vec![None]
        );

        // 文件存在但不是合法 TOML：同样全部 None。
        fs::write(&path, "this is not toml = = =").expect("坏配置应可写入");
        assert_eq!(
            raw_string_values(&path, &["remote.gateway.http_bind"]),
            vec![None]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_string_leaf_is_treated_as_missing() {
        let document: toml::Value =
            toml::from_str("[remote.gateway]\nhttp_bind = 3143\n").expect("样例应可解析");
        assert_eq!(lookup(&document, "remote.gateway.http_bind"), None);
        // 中间节点不是表时也不能 panic。
        assert_eq!(lookup(&document, "remote.gateway.http_bind.deeper"), None);
        assert_eq!(lookup(&document, "remote.nope"), None);
    }
}
