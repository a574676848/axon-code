//! 会话标题生成：每满三轮用快速模型总结最近用户消息。
//!
//! 生成发生在轮后独立线程里，不占回合延迟；读取端只认 `sessions.title`，三端因此
//! 同源。失败一律静默：标题是装饰性事实，不能因为一次模型抖动把成功的一轮变成报错。

use std::path::Path;

use axon_agent_runtime::{AgentPath, ChildExecutionModelIntent};
use rusqlite::OptionalExtension as _;

use super::{run_agent_store, run_request_subagents::execute_standalone_subagent};

/// 触发间隔：每满三轮对话更新一次标题。
const SESSION_TITLE_TURN_INTERVAL: u64 = 3;
/// 送给模型的用户消息上限；超过时取最新这些条。
const SESSION_TITLE_MAX_MESSAGES: usize = 10;
/// 单条消息进入提示词的截断长度，避免一次超长粘贴把提示词撑爆。
const SESSION_TITLE_MESSAGE_CLIP_CHARS: usize = 400;
/// 标题字符上限，与读取端口的上限保持一致。
const SESSION_TITLE_MAX_CHARS: usize = 24;
/// 生成器角色说明；随提示词一起下发，避免为一条首轮就返回的任务另开 system 通道。
const SESSION_TITLE_TASK_GUIDE: &str = "你是 AXON 的会话标题生成器。根据下面这组用户消息，概括这条对话到底在做什么。只输出标题本身：不加引号、不加书名号、不加句末标点、不写解释、不写「用户」「对话」「关于」这类空词。标题长度 8 到 24 个汉字，尽量包含具体对象（文件、模块、命令、错误、需求名）。";

/// 轮后判定是否满足生成条件；满足则起后台线程并返回 `true`。
///
/// 计数、取消息与模型调用**全部**在后台线程里完成：回合收口路径上只留一次线程创建，
/// 不给主回合加任何库读取或模型等待。
pub(super) fn maybe_spawn_session_title_generation(
    config_path: &Path,
    session_id: &str,
    cwd: &Path,
    agent_is_root: bool,
) -> bool {
    // 子代理的轮次不是用户的轮次：拿它生成只会把会话标题改成子任务名。
    if !agent_is_root || session_id.is_empty() {
        return false;
    }
    let config_path = config_path.to_path_buf();
    let cwd = cwd.to_path_buf();
    let session_id = session_id.to_owned();
    std::thread::spawn(move || {
        if let Err(error) = run_session_title_job(&config_path, &cwd, &session_id) {
            axon_logger::warning(&format!(
                "会话标题生成跳过: session_id={session_id}, error={error}"
            ));
        }
    });
    true
}

/// 后台任务主体：先按轮次判断要不要生成，再取消息、调模型、写回。
fn run_session_title_job(config_path: &Path, cwd: &Path, session_id: &str) -> Result<(), String> {
    let store = run_agent_store::shared_runtime_store(config_path)?;
    // 入站消息、会话标题与轮数都挂在会话身份上：这里收到的就是会话标识，不再换算。
    let turn_count = count_user_messages(&store, session_id)?;
    if turn_count < SESSION_TITLE_TURN_INTERVAL {
        return Ok(());
    }
    // 首次补跑：标题还空着且轮次已够就先补一次。否则安装前就存在的旧会话必须等到
    // 下一个三的倍数才会更新，用户看到的是「明明早就超过三轮却一直没有标题」。
    let pending_first =
        stored_session_title(&store, session_id)?.is_none_or(|title| title.trim().is_empty());
    if !pending_first && turn_count % SESSION_TITLE_TURN_INTERVAL != 0 {
        return Ok(());
    }
    generate_session_title(config_path, cwd, session_id, turn_count)
}

/// 读取控制面已登记的标题；会话行不存在或标题为空都返回 `None`。
fn stored_session_title(
    store: &axon_session::SqliteRuntimeStore,
    session_id: &str,
) -> Result<Option<String>, String> {
    store
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT title FROM sessions WHERE session_id=?1",
                    [session_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
        })
        .map(|title| title.flatten())
        .map_err(|error| error.to_string())
}

/// 该车道下用户消息条数；走 `inbox_session_recent` 的索引前缀，不扫正文。
fn count_user_messages(
    store: &axon_session::SqliteRuntimeStore,
    session_id: &str,
) -> Result<u64, String> {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM inbox WHERE session_id=?1",
                [session_id],
                |row| row.get::<_, i64>(0),
            )
        })
        .map(|count| u64::try_from(count).unwrap_or(0))
        .map_err(|error| error.to_string())
}

/// 最近若干条用户消息，按时间先后返回。
fn recent_user_messages(
    store: &axon_session::SqliteRuntimeStore,
    session_id: &str,
    limit: usize,
) -> Result<Vec<String>, String> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut messages = store
        .with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT content FROM inbox WHERE session_id=?1 \
                 ORDER BY received_at_ms DESC, interaction_id DESC LIMIT ?2",
            )?;
            let rows = statement.query_map(rusqlite::params![session_id, limit], |row| {
                row.get::<_, String>(0)
            })?;
            rows.collect::<rusqlite::Result<Vec<String>>>()
        })
        .map_err(|error| error.to_string())?;
    messages.reverse();
    Ok(messages)
}

/// 生成并写回标题。
fn generate_session_title(
    config_path: &Path,
    cwd: &Path,
    session_id: &str,
    turn_count: u64,
) -> Result<(), String> {
    let store = run_agent_store::shared_runtime_store(config_path)?;
    let messages = recent_user_messages(&store, session_id, SESSION_TITLE_MAX_MESSAGES)?;
    if messages.is_empty() {
        return Err("没有可用的用户消息".to_owned());
    }
    let Some(prompt) = build_prompt(&messages) else {
        return Err("用户消息全部为空".to_owned());
    };
    // 任务路径带轮次：同一会话会多次生成，路径必须唯一，否则控制面会拒绝重复注册。
    let task_path = AgentPath::parse(format!("/root/session-title-{turn_count}"))
        .map_err(|error| format!("任务路径非法：{error:?}"))?;
    let output = execute_standalone_subagent(
        config_path,
        cwd,
        session_id,
        task_path,
        &prompt,
        axon_time::unix_now_ms(),
        // 不挂会话 sink：标题生成是内部任务，不该在界面里留下一条逐字稿。
        None,
        ChildExecutionModelIntent::FastDelegate,
    )?;
    if output.status != axon_agent_runtime::ChildExecutionStatus::Completed {
        return Err(format!("生成状态非 Completed: {:?}", output.status));
    }
    let title = clean_title(&output.summary).ok_or_else(|| "模型输出没有可用标题".to_owned())?;
    store
        .update_session_title(session_id, &title)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// 拼接生成提示词；全部消息都为空时返回 `None`。
fn build_prompt(messages: &[String]) -> Option<String> {
    let mut prompt = String::from(SESSION_TITLE_TASK_GUIDE);
    prompt.push_str("\n\n按时间先后排列的用户消息：\n");
    let mut appended = 0_usize;
    for message in messages {
        let clipped = clip_message(message);
        if clipped.is_empty() {
            continue;
        }
        appended += 1;
        prompt.push_str(&format!("{appended}. {clipped}\n"));
    }
    if appended == 0 {
        return None;
    }
    prompt.push_str("\n只输出标题。");
    Some(prompt)
}

/// 压掉空白并把单条消息截断到上限。
fn clip_message(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized
        .chars()
        .take(SESSION_TITLE_MESSAGE_CLIP_CHARS)
        .collect()
}

/// 清洗模型输出：取首个非空行，剥掉包裹符号与句末标点，再截断到上限。
fn clean_title(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .trim_start_matches(['#', '>', '-', '*', '+', '"', '\'', '“', '《', '「'])
        .trim_end_matches([
            '"', '\'', '”', '》', '」', '。', '.', '！', '!', '？', '?', '：', ':',
        ])
        .trim();
    let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = normalized
        .chars()
        .take(SESSION_TITLE_MAX_CHARS)
        .collect::<String>();
    if title.is_empty() { None } else { Some(title) }
}

#[cfg(test)]
mod tests {
    use super::{
        build_prompt, clean_title, clip_message, count_user_messages, recent_user_messages,
        stored_session_title,
    };
    use axon_session::RuntimeStore as _;

    fn seeded_store(messages: usize) -> axon_session::SqliteRuntimeStore {
        seeded_lane("session-title", "session-title", messages)
    }

    /// 车道标识与执行会话标识不同的真实形态：`run_session_id` 才是宿主交给生成器的值。
    fn seeded_lane(lane: &str, run_id: &str, messages: usize) -> axon_session::SqliteRuntimeStore {
        let directory = axon_test_support::test_temp_dir();
        let store = axon_session::SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        store.initialize().expect("初始化运行态库");
        let mut batch = format!(
            "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms,created_at_ms,updated_at_ms)
                 VALUES('{lane}','{run_id}',NULL,'private','primary','/w','idle',NULL,NULL,0,0,0,0,1,1,1);"
        );
        for index in 1..=messages {
            batch.push_str(&format!(
                "INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,received_at_ms,updated_at_ms,session_id)
                     VALUES('i-{index}','tui','m-{index}','{{}}','{{}}','{{}}','m{index}',{index},{index},'{lane}');"
            ));
        }
        store
            .with_read_connection(|connection| connection.execute_batch(&batch))
            .expect("插入入站消息");
        store
    }

    #[test]
    fn counts_user_messages_for_the_lane() {
        let store = seeded_store(4);
        assert_eq!(count_user_messages(&store, "session-title").unwrap(), 4);
        assert_eq!(count_user_messages(&store, "session-unknown").unwrap(), 0);
    }

    #[test]
    fn recent_messages_take_the_latest_window_in_chronological_order() {
        let store = seeded_store(12);
        let messages = recent_user_messages(&store, "session-title", 10).expect("读取消息");
        assert_eq!(messages.len(), 10);
        // 超上限时保留最新十条，并按时间先后交给模型。
        assert_eq!(messages.first().map(String::as_str), Some("m3"));
        assert_eq!(messages.last().map(String::as_str), Some("m12"));
    }

    #[test]
    fn stored_title_distinguishes_first_fill_from_regeneration() {
        let store = seeded_store(3);
        // 会话行存在但标题为空：属于首次补跑，不能当成「已经生成过」。
        assert_eq!(stored_session_title(&store, "session-title").unwrap(), None);
        assert_eq!(
            stored_session_title(&store, "session-unknown").unwrap(),
            None
        );

        store
            .update_session_title("session-title", "已有标题")
            .expect("写入标题");
        assert_eq!(
            stored_session_title(&store, "session-title")
                .unwrap()
                .as_deref(),
            Some("已有标题")
        );
    }

    #[test]
    fn clean_title_strips_wrapping_and_trailing_punctuation() {
        assert_eq!(
            clean_title("“修复 Provider 超时问题。”\n说明：...").as_deref(),
            Some("修复 Provider 超时问题")
        );
        assert_eq!(clean_title("   \n  "), None);
    }

    #[test]
    fn clean_title_bounds_length() {
        let long = clean_title(&"超长标题".repeat(20)).expect("应得到标题");
        assert_eq!(long.chars().count(), 24);
    }

    #[test]
    fn prompt_numbers_messages_and_skips_blank_ones() {
        let messages = vec!["第一条".to_owned(), "   ".to_owned(), "第三条".to_owned()];
        let prompt = build_prompt(&messages).expect("应有提示词");
        assert!(prompt.contains("1. 第一条"));
        assert!(prompt.contains("2. 第三条"));
        assert!(prompt.ends_with("只输出标题。"));
        assert_eq!(build_prompt(&["  ".to_owned()]), None);
    }

    #[test]
    fn message_clip_collapses_whitespace() {
        assert_eq!(clip_message("  a \n\n b  "), "a b");
    }
}
