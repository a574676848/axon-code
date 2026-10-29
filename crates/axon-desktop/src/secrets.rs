//! 密钥的可视化管理：把界面输入的密钥写进配置相邻的 `.env`。
//!
//! 复用配置系统既有的 `.env` 补丁原语
//! （`axon_config::setup::render_env_from`），保留文件里的注释与原有顺序，
//! 不重新生成整份文件；落盘走 `axon_config::write_with_backup`：
//! 先做时间戳备份再直接覆写，与 `config.toml` 的写入策略一致。
//!
//! 不用原子替换是有意的：`.env` 常被其它进程以「允许写入、禁止删除」的方式持有，
//! 临时文件 `persist` 会以「拒绝访问 (os error 5)」失败，而覆写可以成功。
//!
//! 安全边界：读取路径只回传「已配置 / 未配置」这一位状态，
//! 密钥明文既不回传界面，也不出现在任何返回值里。

use std::fs;
use std::path::PathBuf;

use axon_config::ConfigLoader;
use axon_config::setup::{EnvKeyChange, SETUP_SECRET_PRESENT, render_env_from, secret_status};

/// 密钥文件相对配置文件目录的名称。
const ENV_FILE_NAME: &str = ".env";

/// 解析密钥文件路径。
fn env_file_path() -> Result<PathBuf, String> {
    let config = ConfigLoader::default_user_config_path()
        .map_err(|error| format!("获取用户配置路径失败：{error:?}"))?;
    let parent = config
        .parent()
        .ok_or_else(|| "无法定位配置目录".to_string())?;
    Ok(parent.join(ENV_FILE_NAME))
}

/// 校验环境变量名：只允许字母、数字与下划线，且不以数字开头。
fn validate_key(key: &str) -> Result<&str, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("环境变量名不能为空".to_string());
    }
    let mut chars = trimmed.chars();
    let first_ok = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
    if !first_ok || !chars.all(|item| item.is_ascii_alphanumeric() || item == '_') {
        return Err("环境变量名只能包含字母、数字与下划线，且不能以数字开头".to_string());
    }
    Ok(trimmed)
}

/// Tauri IPC 命令：查询某个环境变量是否已经配置密钥。
#[tauri::command(async)]
pub fn secret_status_of(key: String) -> Result<bool, String> {
    let path = env_file_path()?;
    Ok(secret_status(&path, key.trim()) == SETUP_SECRET_PRESENT)
}

/// Tauri IPC 命令：写入或更新一个密钥。
#[tauri::command(async)]
pub fn secret_set(key: String, value: String) -> Result<(), String> {
    let key = validate_key(&key)?.to_string();
    if value.trim().is_empty() {
        return Err("密钥值不能为空".to_string());
    }

    let path = env_file_path()?;
    let raw = fs::read(&path).ok();
    let rendered = render_env_from(
        raw.as_deref(),
        &[EnvKeyChange::Set(key.clone(), value.clone())],
    )
    .ok_or_else(|| "密钥内容没有变化".to_string())?;

    axon_config::write_with_backup(&path, &rendered)
        .map_err(|error| format!("写入密钥文件失败：{error:?}"))?;

    // 同步进程内缓存，界面无需重启即可看到「已配置」。
    axon_config::set_provider_credential(key, value);
    Ok(())
}

/// Tauri IPC 命令：清除一个密钥。
#[tauri::command(async)]
pub fn secret_clear(key: String) -> Result<(), String> {
    let key = validate_key(&key)?.to_string();
    let path = env_file_path()?;
    let raw = fs::read(&path).ok();
    let rendered = render_env_from(raw.as_deref(), &[EnvKeyChange::Clear(key)])
        .ok_or_else(|| "该密钥本来就不存在".to_string())?;

    axon_config::write_with_backup(&path, &rendered)
        .map_err(|error| format!("写入密钥文件失败：{error:?}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_key;

    #[test]
    fn env_key_validation_rejects_unsafe_names() {
        assert_eq!(validate_key(" OPENAI_API_KEY ").unwrap(), "OPENAI_API_KEY");
        assert_eq!(validate_key("_KEY1").unwrap(), "_KEY1");
        assert!(validate_key("").is_err());
        assert!(validate_key("1KEY").is_err());
        assert!(validate_key("BAD-KEY").is_err());
        assert!(validate_key("BAD KEY").is_err());
        assert!(validate_key("坏键").is_err());
    }
}
