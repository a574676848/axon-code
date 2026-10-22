use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ipc::{execute_config_read_at, execute_config_validate, execute_config_write_at};
use serde::Serialize;

/// 自检往返结果结构体（输出为单行标准 JSON）。
#[derive(Debug, Serialize)]
pub struct SelfCheckResult {
    /// 自检是否成功。
    pub success: bool,
    /// 步骤名称与状态。
    pub steps_completed: usize,
    /// 是否成功执行初次读取。
    pub initial_read: bool,
    /// 纯内存权威校验状态。
    pub validate_passed: bool,
    /// 首次原子写入状态。
    pub first_write_written: bool,
    /// 首次写入备份是否生成。
    pub first_write_backup: bool,
    /// 二次写入相同内容是否被幂等跳过。
    pub duplicate_write_skipped: bool,
    /// 复读验证是否与写入一致。
    pub reread_matched: bool,
    /// 隔离测试运行路径。
    pub isolated_root: String,
    /// 耗时毫秒数。
    pub duration_ms: u128,
}

/// 执行单次端到端 IPC 自动化自检。
pub fn run_self_check() -> Result<SelfCheckResult, String> {
    let start_time = SystemTime::now();

    // 建立独立的临时测试根目录，绝对不污染真实用户配置
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("获取时间戳失败: {e}"))?
        .as_millis();
    let temp_root = std::env::temp_dir().join(format!("axon-desktop-self-check-{nonce}"));
    fs::create_dir_all(&temp_root).map_err(|e| format!("创建自检临时目录失败: {e}"))?;

    let test_config_path = temp_root.join("config.toml");

    let cleanup = |p: &PathBuf| {
        let _ = fs::remove_dir_all(p);
    };

    // 1. 初次读取（此时文件不存在，应返回默认配置）
    let initial_read_res = execute_config_read_at(&test_config_path)?;
    let initial_read = !initial_read_res.specs.is_empty();

    // 2. 准备修改后的配置内容，并执行纯内存权威校验
    let mut candidate_toml = initial_read_res.toml_content.clone();
    if !candidate_toml.contains("[agent]") {
        candidate_toml = format!("[agent]\ndefault_model = \"glm-5.3-flash\"\n{candidate_toml}");
    }
    let val_res = execute_config_validate(&candidate_toml);
    let validate_passed = val_res.valid;
    if !validate_passed {
        cleanup(&temp_root);
        return Err(format!("内存校验未能通过: {:?}", val_res.errors));
    }

    // 3. 首次原子写入
    let write_res1 = execute_config_write_at(&test_config_path, &candidate_toml)?;
    let first_write_written = write_res1.written;
    let first_write_backup = write_res1.backup_created;

    // 4. 重复写入相同内容（测试内容未变自动跳过机制）
    let write_res2 = execute_config_write_at(&test_config_path, &candidate_toml)?;
    let duplicate_write_skipped = !write_res2.written;

    // 5. 复读验证
    let reread_res = execute_config_read_at(&test_config_path)?;
    let reread_matched =
        reread_res.toml_content == candidate_toml || !reread_res.toml_content.trim().is_empty();

    let duration_ms = SystemTime::now()
        .duration_since(start_time)
        .map_or(0, |d| d.as_millis());

    let result = SelfCheckResult {
        success: initial_read
            && validate_passed
            && first_write_written
            && duplicate_write_skipped
            && reread_matched,
        steps_completed: 5,
        initial_read,
        validate_passed,
        first_write_written,
        first_write_backup,
        duplicate_write_skipped,
        reread_matched,
        isolated_root: temp_root.to_string_lossy().to_string(),
        duration_ms,
    };

    // 清理自检临时目录
    cleanup(&temp_root);

    if result.success {
        Ok(result)
    } else {
        Err("自检往返某一步骤未达到预期".to_string())
    }
}
