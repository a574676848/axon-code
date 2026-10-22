//! 本机主代理 principal 加载与本机身份补齐。

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use axon_domain::{AXON_OWNER_ID_PREFIX, AxonOwnerId, RunPrincipalContext};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

/// owner identity 相对目录。
pub(crate) const OWNER_IDENTITY_DIRECTORY: &str = "identity";
/// owner identity 文件名。
pub(crate) const OWNER_IDENTITY_FILE: &str = "owner.json";
/// owner identity schema 版本。
const OWNER_IDENTITY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OwnerIdentityFile {
    schema_version: u32,
    owner_id: String,
}

/// 从受信任的 Axon 配置根加载主代理 principal。
///
/// 身份文件缺失时返回 `Ok(None)`：**读路径不产生副作用**，Personal Memory 上下文注入保持
/// 关闭。身份补齐发生在首次真正需要 Personal scope 的写入点
/// （见 [`load_or_provision_owner_id`]），因此不会扰动启动快照指纹。
pub(crate) fn load_run_principal(
    axon_root: &Path,
    channel_ref: &str,
) -> Result<Option<RunPrincipalContext>, String> {
    let path = owner_identity_path(axon_root);
    let Some(owner_id) = read_owner_identity(&path)? else {
        return Ok(None);
    };
    let owner_id = AxonOwnerId::parse(owner_id).map_err(|error| format!("{error:?}"))?;
    Ok(Some(RunPrincipalContext {
        owner_id,
        actor_ref: None,
        channel_ref: channel_ref.to_owned(),
    }))
}

/// 读取本机所有者标识；缺失时按安装器同一合同补齐（`identity/owner.json`，随机
/// `owner_<uuid>`，原子写入且不覆盖既有身份）。
///
/// 补齐是必要的：打包安装器之外的安装路径（`cargo install --path crates/axon-cli`）
/// 从不写这个文件，缺了它 Personal Memory（`profile` / `preference`）会在整台机器上
/// 永久关闭，且没有任何入口可以恢复。
///
/// # Errors
///
/// 既有身份文件损坏、schema 版本不符，或身份补齐失败时返回中文诊断。
pub(crate) fn load_or_provision_owner_id(axon_root: &Path) -> Result<AxonOwnerId, String> {
    let path = owner_identity_path(axon_root);
    let owner_id = match read_owner_identity(&path)? {
        Some(owner_id) => owner_id,
        None => provision_owner_identity(&path)?,
    };
    AxonOwnerId::parse(owner_id).map_err(|error| format!("{error:?}"))
}

fn owner_identity_path(axon_root: &Path) -> PathBuf {
    axon_root
        .join(OWNER_IDENTITY_DIRECTORY)
        .join(OWNER_IDENTITY_FILE)
}

fn read_owner_identity(path: &Path) -> Result<Option<String>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取 Axon owner identity 失败：{error}")),
    };
    let identity: OwnerIdentityFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("解析 Axon owner identity 失败：{error}"))?;
    if identity.schema_version != OWNER_IDENTITY_SCHEMA_VERSION {
        return Err("Axon owner identity schema 版本无效".to_owned());
    }
    Ok(Some(identity.owner_id))
}

/// 原子补齐 `identity/owner.json`；并发补齐时保留先落盘的身份。
fn provision_owner_identity(path: &Path) -> Result<String, String> {
    let directory = path
        .parent()
        .ok_or_else(|| "Axon owner identity 路径缺少父目录".to_owned())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("创建 Axon identity 目录失败：{error}"))?;
    let owner_id = format!("{AXON_OWNER_ID_PREFIX}{}", uuid::Uuid::new_v4());
    let content = serde_json::to_vec(&OwnerIdentityFile {
        schema_version: OWNER_IDENTITY_SCHEMA_VERSION,
        owner_id: owner_id.clone(),
    })
    .map_err(|error| format!("序列化 Axon owner identity 失败：{error}"))?;
    let mut temporary = NamedTempFile::new_in(directory)
        .map_err(|error| format!("创建 Axon owner identity 临时文件失败：{error}"))?;
    temporary
        .write_all(&content)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("写入 Axon owner identity 失败：{error}"))?;
    // 另一个进程可能刚刚补齐了身份：后者读取既有身份而不是覆盖它。
    if let Some(existing) = read_owner_identity(path)? {
        return Ok(existing);
    }
    temporary
        .persist(path)
        .map_err(|error| format!("原子写入 Axon owner identity 失败：{}", error.error))?;
    Ok(owner_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_identity_loads_no_principal_without_side_effects() {
        let root = axon_test_support::test_temp_dir();
        let principal = load_run_principal(root.path(), "cli").unwrap();

        assert!(principal.is_none());
        assert!(
            !owner_identity_path(root.path()).exists(),
            "读路径不得创建身份文件"
        );
    }

    #[test]
    fn missing_identity_is_provisioned_on_demand_with_installer_contract() {
        let root = axon_test_support::test_temp_dir();
        let owner_id = load_or_provision_owner_id(root.path()).expect("缺失身份应按需补齐");
        let raw = fs::read_to_string(owner_identity_path(root.path())).unwrap();
        let written: OwnerIdentityFile = serde_json::from_str(&raw).unwrap();

        assert_eq!(written.schema_version, OWNER_IDENTITY_SCHEMA_VERSION);
        assert_eq!(written.owner_id, owner_id.as_str());
        assert!(written.owner_id.starts_with(AXON_OWNER_ID_PREFIX));
    }

    #[test]
    fn provisioned_identity_is_stable_across_loads() {
        let root = axon_test_support::test_temp_dir();
        let first = load_or_provision_owner_id(root.path()).unwrap();
        let second = load_or_provision_owner_id(root.path()).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn valid_identity_is_loaded() {
        let root = axon_test_support::test_temp_dir();
        let identity_dir = root.path().join(OWNER_IDENTITY_DIRECTORY);
        fs::create_dir_all(&identity_dir).unwrap();
        fs::write(
            identity_dir.join(OWNER_IDENTITY_FILE),
            r#"{"schemaVersion":1,"ownerId":"owner_550e8400-e29b-41d4-a716-446655440000"}"#,
        )
        .unwrap();
        let principal = load_run_principal(root.path(), "cli")
            .expect("应加载 principal")
            .expect("身份存在时应返回 principal");
        assert_eq!(
            principal.owner_id.as_str(),
            "owner_550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn corrupted_identity_reports_diagnostic_instead_of_overwriting() {
        let root = axon_test_support::test_temp_dir();
        let identity_dir = root.path().join(OWNER_IDENTITY_DIRECTORY);
        fs::create_dir_all(&identity_dir).unwrap();
        let path = identity_dir.join(OWNER_IDENTITY_FILE);
        fs::write(&path, b"{\"schemaVersion\":9,\"ownerId\":\"owner_x\"}").unwrap();

        let error = load_or_provision_owner_id(root.path()).expect_err("非法身份必须报错");
        assert!(error.contains("schema 版本无效"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"schemaVersion\":9,\"ownerId\":\"owner_x\"}"
        );
    }
}
