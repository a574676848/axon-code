//! 会话行模型事实的写入口径。

use super::SqliteRuntimeStore;
use crate::{NewSessionRequest, RuntimeStore, SessionKind};
use axon_domain::DaemonSessionId;

/// 会话行的模型事实：只在模型真的变了时写，且不改活跃时间。
#[test]
fn recording_session_model_rewrites_only_when_the_model_changes() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = DaemonSessionId::new("session-model-record").expect("session");
    store
        .create_session(&session_id, "cwd", 10)
        .expect("创建会话");

    let read = |store: &SqliteRuntimeStore| {
        store
            .with_read_connection(|connection| {
                connection.query_row(
                    "SELECT provider,model,model_name,last_activity_at_ms FROM sessions \
                     WHERE session_id=?1",
                    [session_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
            })
            .expect("读回会话行")
    };

    store
        .record_session_model(&session_id, "openai", "gpt-5.5-id", Some("gpt-5.5"))
        .expect("首次写入");
    let (provider, model, model_name, activity) = read(&store);
    assert_eq!(provider.as_deref(), Some("openai"));
    assert_eq!(model.as_deref(), Some("gpt-5.5-id"));
    assert_eq!(model_name.as_deref(), Some("gpt-5.5"));
    assert_eq!(activity, 10, "模型事实不属于活跃时间");

    // 同一模型再写一次：条件不成立，命中 0 行，活跃时间不变。
    store
        .record_session_model(&session_id, "openai", "gpt-5.5-id", Some("gpt-5.5"))
        .expect("重复写入");
    assert_eq!(read(&store).3, 10);

    store
        .record_session_model(&session_id, "anthropic", "claude-4", None)
        .expect("模型变更");
    let (provider, model, model_name, _) = read(&store);
    assert_eq!(provider.as_deref(), Some("anthropic"));
    assert_eq!(model.as_deref(), Some("claude-4"));
    assert_eq!(model_name, None);
}
/// 轮次标识与会话类型无关：主会话同样记录「当前这一轮是谁」。
#[test]
fn binding_turn_execution_covers_every_session_kind() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let goal = DaemonSessionId::new("session-goal").expect("session");
    let primary = DaemonSessionId::new("session-primary").expect("session");
    for (session, kind) in [(&goal, SessionKind::Goal), (&primary, SessionKind::Primary)] {
        let mut request = NewSessionRequest::primary(session.clone(), "cwd", 10);
        request.kind = kind;
        store.create_session_with(&request).expect("创建会话");
    }

    store
        .bind_session_execution(&goal, "execution-goal", 20)
        .expect("绑定轮次");
    store
        .bind_session_execution(&primary, "execution-primary", 20)
        .expect("主会话同样绑定轮次");

    let read = |session: &DaemonSessionId| {
        store
            .with_read_connection(|connection| {
                connection.query_row(
                    "SELECT execution_id FROM sessions WHERE session_id=?1",
                    [session.as_str()],
                    |row| row.get::<_, Option<String>>(0),
                )
            })
            .expect("读取会话行")
    };
    assert_eq!(read(&goal).as_deref(), Some("execution-goal"));
    assert_eq!(read(&primary).as_deref(), Some("execution-primary"));
}
