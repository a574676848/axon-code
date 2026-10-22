use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_config::{AxonConfig, ConfigLoader};

/// 创建临时测试根目录
pub fn test_root(test_name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-cli-test-{test_name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

/// 写入测试配置文件
pub fn write_config(root: &Path, config: &AxonConfig) -> PathBuf {
    fs::create_dir_all(root).expect("测试配置目录应可创建");
    let config_path = root.join("config.toml");
    ConfigLoader::save_to_path(&config_path, config).expect("测试配置应可写入");
    config_path
}

/// 写入默认配置
pub fn write_default_config(root: &Path) -> PathBuf {
    write_config(root, &AxonConfig::default())
}