use std::path::PathBuf;

pub(super) fn temp_config_path(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root()
        .join(format!("axon-cli-{label}-{}-{nonce}", std::process::id()))
        .join("config.toml")
}
