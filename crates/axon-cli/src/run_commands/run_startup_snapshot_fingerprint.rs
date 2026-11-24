//! 启动快照的失效指纹。
//!
//! 只做轻量元数据扫描（mtime、大小、条目数），绝不读取文件内容：指纹在 TTL
//! 到期后计算，一致时只续期缓存，避免周期性重跑 MCP 探测。
//!
//! 覆盖会改变快照内容的资源：用户级与工作区 AGENTS.md、`.mcp.json`、Skills 根
//! 目录，以及用户级 Memory 存储目录。

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use axon_crypto::sha256_hex;

/// 目录指纹的递归深度上限；Skills 与 Memory 的实际层级远小于该值。
const FINGERPRINT_MAX_DEPTH: usize = 4;
/// 单次指纹扫描的目录条目上限，防止异常目录拖慢 turn。
const FINGERPRINT_MAX_ENTRIES: usize = 4_096;
/// Workspace 指令链向上回溯的层数上限。
const FINGERPRINT_MAX_ANCESTORS: usize = 16;
/// Workspace 指令文件名。
const WORKSPACE_INSTRUCTIONS_FILE_NAME: &str = "AGENTS.md";
/// 运行时 MCP 配置文件名。
const MCP_CONFIG_FILE_NAME: &str = ".mcp.json";
/// 用户级运行态下的 Memory 存储目录。
const MEMORY_DATA_DIRS: [&str; 2] = ["data", "memory"];

/// 计算启动快照指纹；任何读取失败都退化为稳定的“缺失”标记，不阻断 turn。
pub(crate) fn startup_snapshot_fingerprint(config_path: &Path, cwd: &Path) -> String {
    let mut parts = Vec::new();
    if let Ok(axon_root) = axon_config::runtime_axon_root_path(config_path) {
        parts.push(file_fingerprint(
            &axon_root.join(WORKSPACE_INSTRUCTIONS_FILE_NAME),
        ));
        parts.push(file_fingerprint(&axon_root.join(MCP_CONFIG_FILE_NAME)));
        parts.push(directory_fingerprint(&memory_data_dir(&axon_root)));
    }
    if let Ok(user_home) = axon_config::user_home_dir() {
        parts.push(directory_fingerprint(&skills_dir(&user_home.join(".axon"))));
    }
    for instruction in workspace_instruction_paths(cwd) {
        parts.push(file_fingerprint(&instruction));
    }
    parts.push(file_fingerprint(&cwd.join(MCP_CONFIG_FILE_NAME)));
    parts.push(directory_fingerprint(&skills_dir(&cwd.join(".axon"))));
    sha256_hex(parts.join("\n").as_bytes())
}

fn memory_data_dir(axon_root: &Path) -> PathBuf {
    MEMORY_DATA_DIRS
        .iter()
        .fold(axon_root.to_path_buf(), |path, segment| path.join(segment))
}

fn skills_dir(axon_dir: &Path) -> PathBuf {
    axon_dir.join("skills")
}

/// Workspace 指令链：从当前目录向上收集存在的 AGENTS.md。
fn workspace_instruction_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut current = Some(cwd);
    for _ in 0..FINGERPRINT_MAX_ANCESTORS {
        let Some(directory) = current else {
            break;
        };
        let candidate = directory.join(WORKSPACE_INSTRUCTIONS_FILE_NAME);
        if candidate.is_file() {
            paths.push(candidate);
        }
        current = directory.parent();
    }
    paths.sort();
    paths
}

/// 单文件指纹：修改时间、大小与类型；缺失与读取失败都是稳定标记。
fn file_fingerprint(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(metadata) => format!(
            "{}:{}:{}",
            modified_nanos(&metadata),
            metadata.len(),
            metadata.is_dir()
        ),
        Err(_) => "absent".to_owned(),
    }
}

/// 目录指纹：条目数、最新修改时间与累计字节数，不区分具体条目。
fn directory_fingerprint(root: &Path) -> String {
    let mut entries: u64 = 0;
    let mut newest_nanos: u128 = 0;
    let mut total_bytes: u64 = 0;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = stack.pop() {
        if depth > FINGERPRINT_MAX_DEPTH || entries as usize >= FINGERPRINT_MAX_ENTRIES {
            break;
        }
        let Ok(read_dir) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in read_dir.flatten() {
            if entries as usize >= FINGERPRINT_MAX_ENTRIES {
                break;
            }
            entries = entries.saturating_add(1);
            let path = entry.path();
            match fs::metadata(&path) {
                Ok(metadata) => {
                    newest_nanos = newest_nanos.max(modified_nanos(&metadata));
                    total_bytes = total_bytes.saturating_add(metadata.len());
                    if metadata.is_dir() {
                        stack.push((path, depth + 1));
                    }
                }
                Err(_) => continue,
            }
        }
    }
    format!("{entries}:{newest_nanos}:{total_bytes}")
}

fn modified_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::{startup_snapshot_fingerprint, workspace_instruction_paths};
    use std::fs;

    #[test]
    fn fingerprint_changes_when_workspace_instruction_changes() {
        let root = temp_root("fingerprint-instruction");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("创建工作区");
        let config_path = root.join("config.toml");
        fs::write(&config_path, "").expect("写入配置");
        let instruction = workspace.join("AGENTS.md");
        fs::write(&instruction, "第一版规则").expect("写入指令");

        let first = startup_snapshot_fingerprint(&config_path, &workspace);
        fs::write(&instruction, "第二版规则，内容更长").expect("覆盖指令");
        let second = startup_snapshot_fingerprint(&config_path, &workspace);

        assert_ne!(first, second, "指令内容变化必须改变指纹");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn fingerprint_is_stable_without_changes() {
        let root = temp_root("fingerprint-stable");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("创建工作区");
        let config_path = root.join("config.toml");
        fs::write(&config_path, "").expect("写入配置");

        let first = startup_snapshot_fingerprint(&config_path, &workspace);
        let second = startup_snapshot_fingerprint(&config_path, &workspace);

        assert_eq!(first, second);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn workspace_instruction_paths_walk_up_to_ancestors() {
        let root = temp_root("fingerprint-ancestors");
        let child = root.join("app").join("nested");
        fs::create_dir_all(&child).expect("创建子目录");
        fs::write(root.join("AGENTS.md"), "根规则").expect("写入根规则");
        fs::write(root.join("app").join("AGENTS.md"), "应用规则").expect("写入应用规则");

        let paths = workspace_instruction_paths(&child);

        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&root.join("AGENTS.md")));
        assert!(paths.contains(&root.join("app").join("AGENTS.md")));
        fs::remove_dir_all(&root).ok();
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root()
            .join(format!("axon-cli-{label}-{}-{nonce}", std::process::id()))
    }
}
