use std::path::Path;

/// .env 中检测到非空凭据时返回的状态值。
pub const SETUP_SECRET_PRESENT: &str = "present";
/// .env 中未检测到凭据时返回的状态值。
pub const SETUP_SECRET_MISSING: &str = "missing";

/// 对 .env 中一个环境变量执行的变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvKeyChange {
    /// 设置变量名和值。
    Set(String, String),
    /// 清除变量名及其现有值。
    Clear(String),
}

#[derive(Debug, Clone)]
struct ParsedEnvLine {
    raw_line: String,
    prefix: String,
    key: Option<String>,
    newline: String,
}

fn parse_env_line(line_with_ending: &str) -> ParsedEnvLine {
    let newline = if line_with_ending.ends_with("\r\n") {
        "\r\n".to_owned()
    } else if line_with_ending.ends_with('\n') {
        "\n".to_owned()
    } else {
        String::new()
    };
    let content = line_with_ending
        .strip_suffix("\r\n")
        .or_else(|| line_with_ending.strip_suffix('\n'))
        .unwrap_or(line_with_ending);
    let trimmed = content.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return ParsedEnvLine {
            raw_line: content.to_owned(),
            prefix: String::new(),
            key: None,
            newline,
        };
    }
    let (prefix, rest) = if let Some(stripped) = trimmed.strip_prefix("export ") {
        ("export ".to_owned(), stripped)
    } else {
        (String::new(), trimmed)
    };
    if let Some((k, _)) = rest.split_once('=') {
        let key_name = k.trim().to_owned();
        if !key_name.is_empty() {
            return ParsedEnvLine {
                raw_line: content.to_owned(),
                prefix,
                key: Some(key_name),
                newline,
            };
        }
    }
    ParsedEnvLine {
        raw_line: content.to_owned(),
        prefix: String::new(),
        key: None,
        newline,
    }
}

/// 应用 .env 补丁；无实际变化时返回 None。
pub fn render_env_from(raw: Option<&[u8]>, changes: &[EnvKeyChange]) -> Option<Vec<u8>> {
    if changes.is_empty() {
        return None;
    }
    let raw_bytes = raw.unwrap_or(&[]);
    let text = String::from_utf8_lossy(raw_bytes);
    let mut lines = Vec::new();
    if !text.is_empty() {
        let mut start = 0;
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                lines.push(parse_env_line(&text[start..=i]));
                start = i + 1;
            }
        }
        if start < text.len() {
            lines.push(parse_env_line(&text[start..]));
        }
    }
    let mut modified = false;
    for change in changes {
        match change {
            EnvKeyChange::Set(key, val) => {
                let matches: Vec<usize> = lines
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, line)| {
                        (line.key.as_deref() == Some(key.as_str())).then_some(idx)
                    })
                    .collect();
                if matches.is_empty() {
                    if let Some(last) = lines.last_mut()
                        && last.newline.is_empty()
                    {
                        last.newline = "\n".to_owned();
                    }
                    lines.push(ParsedEnvLine {
                        raw_line: format!("{key}={val}"),
                        prefix: String::new(),
                        key: Some(key.clone()),
                        newline: "\n".to_owned(),
                    });
                    modified = true;
                } else {
                    let item = &mut lines[*matches.last().unwrap()];
                    let new_raw = format!("{}{key}={val}", item.prefix);
                    if item.raw_line != new_raw {
                        item.raw_line = new_raw;
                        modified = true;
                    }
                }
            }
            EnvKeyChange::Clear(key) => {
                for line in &mut lines {
                    if line.key.as_deref() == Some(key.as_str()) {
                        line.key = None;
                        line.raw_line.clear();
                        modified = true;
                    }
                }
            }
        }
    }
    if !modified {
        return None;
    }
    let mut out = Vec::new();
    for line in lines {
        if line.key.is_none() && line.raw_line.is_empty() {
            continue;
        }
        out.extend_from_slice(line.raw_line.as_bytes());
        out.extend_from_slice(line.newline.as_bytes());
    }
    if raw.is_some() && out == raw_bytes {
        None
    } else {
        Some(out)
    }
}

/// 查询环境变量或 .env 文件中的凭据状态。
pub fn secret_status(path: &Path, key: &str) -> &'static str {
    if std::env::var(key).is_ok_and(|value| !value.trim().is_empty()) {
        return SETUP_SECRET_PRESENT;
    }
    if !path.exists() {
        return SETUP_SECRET_MISSING;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return SETUP_SECRET_MISSING;
    };
    let mut last_val = None;
    for line in String::from_utf8_lossy(&bytes).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        if let Some((name, value)) = rest.split_once('=')
            && name.trim() == key
        {
            last_val = Some(value.trim().to_owned());
        }
    }
    if last_val.is_some_and(|value| !value.is_empty()) {
        SETUP_SECRET_PRESENT
    } else {
        SETUP_SECRET_MISSING
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_crlf_export_and_duplicates() {
        let raw = b"API_KEY=old\r\nexport API_KEY=older\r\nOTHER=1\r\n";
        let out = render_env_from(
            Some(raw),
            &[EnvKeyChange::Set("API_KEY".into(), "new".into())],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "API_KEY=old\r\nexport API_KEY=new\r\nOTHER=1\r\n"
        );
    }
    #[test]
    fn clear_preserves_comments() {
        let raw = b"# comment\nAPI_KEY=val\nOTHER=2\n";
        let out = render_env_from(Some(raw), &[EnvKeyChange::Clear("API_KEY".into())]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "# comment\nOTHER=2\n");
    }
}
