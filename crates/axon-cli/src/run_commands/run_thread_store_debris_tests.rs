//! 恢复入口的崩溃现场判据：只剩重写临时文件时必须报错，不得以空历史开跑。

use super::*;

/// 只剩重写临时文件时不得当成「没有可恢复会话」：那是崩溃现场，恢复必须报错。
///
/// 判据与 axon-session 侧的 `thread_has_unfinished_write` 同一口径，这里锁的是 CLI 恢复
/// 入口：`prepare_run_session` 一旦返回 `Ok(None)`，run 会拿空历史开跑，旧历史被孤儿
/// 文件永久埋掉，用户只会看到「会话失忆」。
#[test]
fn prepare_run_session_rejects_rewrite_debris_instead_of_starting_empty() {
    let root = temp_root("only-rewrite-debris");
    std::fs::create_dir_all(&root).unwrap();
    let config_path = root.join("isolated-config.toml");
    std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").unwrap();
    let mut record = session_record("用户问题", "最终回答", 300);
    record.session_id = "debris-session".to_owned();
    record.thread_id = record.session_id.clone();
    mirror_session_to_thread(&config_path, &record).unwrap();

    // 构造「重写崩在替换分片中间」的现场：分片全被挪走，只剩重写临时文件。
    let shards = JsonlThreadEventStore::thread_shard_paths(&root, &record.thread_id)
        .expect("分片路径必须可读");
    assert!(!shards.is_empty(), "镜像后必须至少有一个分片");
    let temporary = {
        let shard = &shards[0];
        let file_name = shard.file_name().unwrap().to_string_lossy().into_owned();
        let prefix = file_name.split('.').next().unwrap().to_owned();
        shard.with_file_name(format!("{prefix}.rewrite0.tmp"))
    };
    for shard in &shards {
        std::fs::remove_file(shard).unwrap();
    }
    std::fs::write(&temporary, b"{}").unwrap();
    assert!(
        JsonlThreadEventStore::thread_has_unfinished_write(&root, &record.thread_id),
        "临时文件必须被识别为未完成写入（{}）",
        temporary.display()
    );

    let error = prepare_run_session(
        &config_path,
        &super::super::super::RunIdentities::new(
            record.session_id.clone(),
            record.thread_id.clone(),
        ),
    )
    .expect_err("只剩重写临时文件时必须报错，不能返回 Ok(None) 用空历史开跑");

    assert!(error.contains("未完成重写"), "{error}");
    assert!(error.contains("拒绝以空历史恢复"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}
