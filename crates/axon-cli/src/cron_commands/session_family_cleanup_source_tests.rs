//! 源码安装真机夹具的构造与核验用例。
//!
//! 与常规回归用例分开成文件，是因为这三条用例不构造临时根：它们在真实安装目录上
//! 先造夹具、跑安装版 dry-run，再核验安装版 cleanup 的结果，属于需要显式授权才跑的
//! 真机通道。混在临时根用例里会让「跑全部测试」看起来覆盖了真机，实际两条路都不成立。

use super::*;

/// 真机隔离根：必须由调用者显式给出，脚本默认不指向任何真实目录。
const REAL_DEVICE_ROOT_ENV: &str = "AXON_WP6_C2_FIXTURE_ROOT";

/// 隔离根的配置：指向本机回环上的假 provider，避免真机夹具误连真实渠道。
const ISOLATION_CONFIG: &str = r#"[agent]
default_model = "local-model"
fast_model = "local-model"

[[models]]
id = "local-model"
name = "local-model"
provider = "local"

[[providers]]
api_key_env = "AXON_WP6_C2_KEY"
base_url = "http://127.0.0.1:39123/v1"
name = "local"
protocol = "openai-chatcompletions"
"#;

#[test]
#[ignore = "真机隔离 fixture：需显式设置 AXON_WP6_C2_FIXTURE_ROOT"]
fn seed_source_installed_cleanup_fixture() {
    let root = real_device_root();
    std::fs::create_dir_all(&root).expect("隔离根应可创建");
    std::fs::write(root.join("config.toml"), ISOLATION_CONFIG).expect("隔离配置应可写入");
    create_runtime_family(&root, "idle", false);
    create_governance(&root);
    create_thread(&root, STALE_ROOT, None, 100);
    create_thread(&root, RETAINED_SESSION, None, RETAINED_UPDATED_AT_MS);
    create_context(&root, STALE_ROOT);
    create_retained_context(&root);
}

#[test]
#[ignore = "真机隔离 dry-run 校验：需先运行安装版 dry-run"]
fn verify_source_installed_dry_run_fixture() {
    let root = real_device_root();
    assert!(runtime_exists(&root, STALE_ROOT));
    assert!(runtime_exists(&root, STALE_CHILD));
    assert!(thread_exists(&root, STALE_ROOT));
    assert!(root.join("sessions").join(STALE_ROOT).exists());
}

#[test]
#[ignore = "真机隔离校验：需先运行安装版 cleanup"]
fn verify_source_installed_cleanup_fixture() {
    let root = real_device_root();
    assert!(!runtime_exists(&root, STALE_ROOT));
    assert!(!runtime_exists(&root, STALE_CHILD));
    assert!(!thread_exists(&root, STALE_ROOT));
    assert!(!root.join("sessions").join(STALE_ROOT).exists());
    assert!(
        thread_exists(&root, RETAINED_SESSION),
        "保留会话的 thread 事件不得被清理"
    );
    assert_eq!(
        std::fs::read(
            root.join("sessions")
                .join(RETAINED_SESSION)
                .join("retained.txt")
        )
        .expect("retained context 应可读"),
        b"retained-session-bytes\n"
    );
}

fn real_device_root() -> PathBuf {
    std::env::var_os(REAL_DEVICE_ROOT_ENV)
        .map(PathBuf::from)
        .expect("必须设置 AXON_WP6_C2_FIXTURE_ROOT")
}
