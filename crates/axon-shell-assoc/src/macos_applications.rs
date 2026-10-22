//! macOS LaunchServices 查询结果的**纯解析**（不启动任何进程、不触碰系统）。
//!
//! 与 `macos.rs` 分开是为了可验证性：形状校验、分组去重与版本比较只吃 JSON 文本，
//! 因此能在任意平台（含开发机上的 Windows 与 CI）用内联 fixture 直接单测；
//! `macos.rs` 里真正去调 `osascript` / `open` 的部分只能在 macOS 上跑。
//!
//! 规则与既有工作台产品逐条对齐：
//!
//! 1. **形状校验从严**：`id` 必须是非空字符串、`name` 必须是字符串、`default` 必须是布尔、
//!    `icon` 必须是 `null` 或 PNG/SVG 的 data URL。一条不合法即整批判为失败：
//!    宁可不显示，也不把一份半懂的结果摆给用户。
//! 2. **折叠同一应用的多份拷贝**：键是「bundle 标识 + 显示名」。自更新留下的暂存副本、
//!    按版本并列安装的同一应用都落进同一组，组内保留**系统默认**优先、否则**版本更高**的那份，
//!    并留在该组**首次出现的位置**。
//! 3. **显示名不同就不折叠**：`Xcode.app` 与 `Xcode-26.0.0.app` 是两个条目。
//! 4. 没有 bundle 标识的条目不参与分组：它证明不了自己与别人是同一个应用。

use std::cmp::Ordering;
use std::collections::HashMap;

/// `osascript` 交出的一条应用条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MacApplication {
    /// 应用包的绝对路径；回传「用哪个应用打开」时用的就是它。
    pub(crate) id: String,
    /// 本地化的显示名。
    pub(crate) name: String,
    /// 是否为 LaunchServices 认定的默认应用。
    pub(crate) default: bool,
    /// 32×32 PNG 的 data URL；取不到时为 `None`。
    pub(crate) icon: Option<String>,
    /// bundle 标识；缺失或为空时不参与分组。
    pub(crate) bundle: Option<String>,
    /// `CFBundleShortVersionString`；缺失时版本最低。
    pub(crate) version: Option<String>,
}

/// 解析 `osascript` 输出的 JSON；形状不对时返回 `None`，由调用方按「查询失败」处理。
pub(crate) fn parse_applications(json: &str) -> Option<Vec<MacApplication>> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let entries = value.as_array()?;
    let mut applications = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = entry.as_object()?;
        let id = object.get("id")?.as_str()?.to_owned();
        if id.is_empty() {
            return None;
        }
        let name = object.get("name")?.as_str()?.to_owned();
        let default = object.get("default")?.as_bool()?;
        let icon = match object.get("icon") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(value)) if is_icon_data_url(value) => {
                Some(value.clone())
            }
            _ => return None,
        };
        applications.push(MacApplication {
            id,
            name,
            default,
            icon,
            bundle: optional_text(object.get("bundle")),
            version: optional_text(object.get("version")),
        });
    }
    Some(applications)
}

/// 图标必须是 PNG 或 SVG 的 data URL；其他形状一律判为不合法。
fn is_icon_data_url(value: &str) -> bool {
    let Some(rest) = value
        .strip_prefix("data:image/png;base64,")
        .or_else(|| value.strip_prefix("data:image/svg+xml;base64,"))
    else {
        return false;
    };
    !rest.is_empty()
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

/// 取一个可选的文本字段；`null`、缺失与非文本都当成「没有」。
fn optional_text(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// 折叠同一应用的多份拷贝，并保持各组首次出现的位置。
pub(crate) fn dedupe_applications(applications: Vec<MacApplication>) -> Vec<MacApplication> {
    let mut winners: Vec<MacApplication> = Vec::with_capacity(applications.len());
    let mut positions: HashMap<String, usize> = HashMap::new();
    for application in applications {
        let Some(key) = group_key(&application) else {
            winners.push(application);
            continue;
        };
        match positions.get(&key) {
            Some(index) => {
                if preferred_over(&application, &winners[*index]) {
                    winners[*index] = application;
                }
            }
            None => {
                positions.insert(key, winners.len());
                winners.push(application);
            }
        }
    }
    winners
}

/// 分组键：同一个应用包的同一个显示名算一组；没有 bundle 标识时不分组。
fn group_key(application: &MacApplication) -> Option<String> {
    let bundle = application
        .bundle
        .as_deref()
        .filter(|value| !value.is_empty())?;
    Some(format!("{bundle}\u{0}{}", application.name))
}

/// 组内优先级：系统默认压过一切，其次比版本号。
fn preferred_over(candidate: &MacApplication, current: &MacApplication) -> bool {
    if candidate.default != current.default {
        return candidate.default;
    }
    compare_versions(candidate.version.as_deref(), current.version.as_deref()) == Ordering::Greater
}

/// 比较点分版本号：逐段按数值比，段数少的一方缺段按 0 计，非数字段当 0。
///
/// 缺失版本**最低**；两个都缺时相等。
pub(crate) fn compare_versions(left: Option<&str>, right: Option<&str>) -> Ordering {
    let (left, right) = match (left, right) {
        (Some(left), Some(right)) => (left, right),
        (Some(_), None) => return Ordering::Greater,
        (None, Some(_)) => return Ordering::Less,
        (None, None) => return Ordering::Equal,
    };
    let mut left_parts = left.split('.');
    let mut right_parts = right.split('.');
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return Ordering::Equal,
            (left_part, right_part) => {
                let left_value = left_part
                    .and_then(|part| part.parse::<u64>().ok())
                    .unwrap_or(0);
                let right_value = right_part
                    .and_then(|part| part.parse::<u64>().ok())
                    .unwrap_or(0);
                if left_value != right_value {
                    return left_value.cmp(&right_value);
                }
            }
        }
    }
}
