//! 车道模型事实的读取测试：只读最新一轮，不遍历历史。

use super::super::{CLI_ROLLOUT_DB_FILE, RolloutSqliteStore, load_latest_session_model_from_root};
use super::record;

/// 车道模型只读最新一轮：两轮不同模型时必须返回后写的那轮，库不存在时给 None。
///
/// 这条约束是性能要求下的正确性保证：界面显示「这条会话现在用哪个模型」只查一条
/// 记录，历史轮次一律不读。
#[test]
fn latest_session_model_reads_only_the_newest_turn() {
    let root = std::env::temp_dir().join(format!(
        "axon_latest_model_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = axon_session::session_store_path(&root).join(CLI_ROLLOUT_DB_FILE);
    std::fs::create_dir_all(db.parent().expect("rollout 目录")).unwrap();
    let store = RolloutSqliteStore::open(&db).unwrap();
    let mut first = record("lane-session", None, 100);
    first.provider = "sm-wenwen-chat".to_owned();
    first.model = "deepseek-v4.1-flash-1017".to_owned();
    store.record_session(&first).unwrap();
    let mut second = record("lane-session", None, 200);
    second.provider = "deng".to_owned();
    second.model = "gpt-6-astra".to_owned();
    store.record_session(&second).unwrap();

    let facts = load_latest_session_model_from_root(&root, "lane-session")
        .unwrap()
        .expect("应读到最新一轮");
    assert_eq!(facts.provider, "deng");
    assert_eq!(facts.model, "gpt-6-astra");

    let missing = root.join("no-such-root");
    assert!(
        load_latest_session_model_from_root(&missing, "lane-session")
            .unwrap()
            .is_none(),
        "账本不存在时必须给 None，而不是报错或造一个模型"
    );
    let _ = std::fs::remove_dir_all(&root);
}
