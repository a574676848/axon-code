//! Provider 凭据仓库：`.env` 与进程环境的统一读取入口。
//!
//! 凭据只落在 Axon 进程内仓库，不写入真实进程环境：Rust 2024 将
//! `std::env::set_var` 标记为 unsafe，本工作区以 `unsafe_code = "forbid"` 作为质量门禁。
//! 所有凭据读取方必须调用 [`provider_credential`]，不得直接读 `std::env::var`，
//! 否则配置相邻 `.env` 中的密钥不可见。

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static ENV_FILE_KEYS: OnceLock<Mutex<HashMap<PathBuf, HashSet<String>>>> = OnceLock::new();

fn store() -> &'static Mutex<HashMap<String, String>> {
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn env_file_keys() -> &'static Mutex<HashMap<PathBuf, HashSet<String>>> {
    ENV_FILE_KEYS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 写入当前进程内的 provider 凭据。
pub fn set_provider_credential(name: impl Into<String>, value: impl Into<String>) {
    let name = name.into();
    let value = value.into();
    if name.trim().is_empty() || value.is_empty() {
        return;
    }
    if let Ok(mut values) = store().lock() {
        values.insert(name, value);
    }
}

/// 从配置相邻 `.env` 加载当前进程可用的 provider 凭据。
///
/// 调用方 shell 已显式设置的同名变量始终优先，文件内容不会覆盖它们。
/// 重复加载会撤销此前仅由该 `.env` 注入、但当前文件已移除的变量。
pub fn load_provider_credentials_from_env_file(path: &Path) -> Result<usize, String> {
    let text = if path.exists() {
        std::fs::read_to_string(path).map_err(|_| "无法读取 Axon .env 密钥文件".to_owned())?
    } else {
        String::new()
    };
    let mut credentials = HashMap::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!("Axon .env 第 {} 行格式无效", index + 1));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("Axon .env 第 {} 行缺少变量名", index + 1));
        }
        credentials.insert(name.to_owned(), normalize_env_value(value.trim()));
    }

    let mut file_keys = env_file_keys()
        .lock()
        .map_err(|_| "无法更新 Axon 本机凭据".to_owned())?;
    let mut values = store()
        .lock()
        .map_err(|_| "无法更新 Axon 本机凭据".to_owned())?;
    let file_keys = file_keys.entry(path.to_path_buf()).or_default();
    for name in &*file_keys {
        if !credentials.contains_key(name) && std::env::var_os(name).is_none() {
            values.remove(name);
        }
    }

    let mut loaded_count = 0;
    file_keys.clear();
    for (name, value) in credentials {
        if std::env::var_os(&name).is_none() {
            values.insert(name.clone(), value);
            loaded_count += 1;
        }
        file_keys.insert(name);
    }
    Ok(loaded_count)
}

fn normalize_env_value(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let quoted = (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\x27' && bytes[value.len() - 1] == b'\x27');
        if quoted {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

/// 解析 provider 凭据，优先使用调用方 shell 已显式设置的环境变量。
#[must_use]
pub fn provider_credential(name: &str) -> Option<String> {
    std::env::var(name).ok().or_else(|| {
        store()
            .lock()
            .ok()
            .and_then(|values| values.get(name).cloned())
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{load_provider_credentials_from_env_file, provider_credential};

    #[test]
    fn env_reload_removes_a_deleted_file_managed_credential() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("AXON_TEST_ENV_RELOAD_{}_{}", std::process::id(), stamp);
        let path = axon_test_support::test_temp_root().join(format!("axon-env-reload-{stamp}.env"));
        fs::write(&path, format!("{name}=present\n")).unwrap();

        load_provider_credentials_from_env_file(&path).unwrap();
        assert_eq!(provider_credential(&name).as_deref(), Some("present"));

        fs::remove_file(&path).unwrap();
        load_provider_credentials_from_env_file(&path).unwrap();
        assert_eq!(provider_credential(&name), None);
    }

    #[test]
    fn env_loads_from_distinct_files_do_not_clear_each_other() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let first_name = format!("AXON_TEST_ENV_FIRST_{stamp}");
        let second_name = format!("AXON_TEST_ENV_SECOND_{stamp}");
        let first_path =
            axon_test_support::test_temp_root().join(format!("axon-env-first-{stamp}.env"));
        let second_path =
            axon_test_support::test_temp_root().join(format!("axon-env-second-{stamp}.env"));
        fs::write(&first_path, format!("{first_name}=first\n")).unwrap();
        fs::write(&second_path, format!("{second_name}=second\n")).unwrap();

        load_provider_credentials_from_env_file(&first_path).unwrap();
        load_provider_credentials_from_env_file(&second_path).unwrap();

        assert_eq!(provider_credential(&first_name).as_deref(), Some("first"));
        assert_eq!(provider_credential(&second_name).as_deref(), Some("second"));

        fs::remove_file(&first_path).unwrap();
        fs::remove_file(&second_path).unwrap();
        load_provider_credentials_from_env_file(&first_path).unwrap();
        load_provider_credentials_from_env_file(&second_path).unwrap();
    }
}
