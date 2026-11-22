//! 待消费队列库（outbox）的 DDL。
//!
//! 与 [`super::schema`] 的分工：建库流程、版本号与分派留在主模块，各库的 DDL 各自成
//! 模块。主模块已到单文件行数上限，继续把新表堆进同一个常量会让它突破 700 行硬限；
//! 按库拆开后，每个库的表定义仍然是一处，不产生第二份 schema 事实。

/// 待消费队列库：有界、按过期时间整批删除的协作事实。
pub(super) const OUTBOX_DDL: &str = "
        CREATE TABLE IF NOT EXISTS client_presence (
            client_id TEXT PRIMARY KEY NOT NULL,
            workspace_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            destination TEXT NOT NULL,
            registered_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS client_presence_workspace
            ON client_presence(workspace_id, expires_at_ms DESC);
        CREATE INDEX IF NOT EXISTS client_presence_expiry
            ON client_presence(expires_at_ms ASC, client_id ASC);
        CREATE TABLE IF NOT EXISTS notifications (
            notification_id TEXT PRIMARY KEY NOT NULL,
            workspace_id TEXT NOT NULL,
            goal_id TEXT NOT NULL,
            execution_id TEXT NOT NULL UNIQUE,
            content TEXT NOT NULL,
            summary TEXT NOT NULL,
            result_ids_json TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS notifications_state
            ON notifications(state, created_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notifications_workspace
            ON notifications(workspace_id, state, created_at_ms ASC);
        CREATE INDEX IF NOT EXISTS notifications_retention
            ON notifications(state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notifications_expiry
            ON notifications(expires_at_ms ASC, notification_id ASC)
            WHERE state IN ('pending','dispatched');
        CREATE INDEX IF NOT EXISTS notifications_expiry_all
            ON notifications(expires_at_ms ASC, notification_id ASC);
        CREATE TABLE IF NOT EXISTS notification_deliveries (
            notification_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            destination TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY(notification_id, channel, destination),
            FOREIGN KEY(notification_id) REFERENCES notifications(notification_id)
        );
        CREATE INDEX IF NOT EXISTS notification_deliveries_inbox
            ON notification_deliveries(channel, state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notification_deliveries_target_inbox
            ON notification_deliveries(channel, destination, state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notification_deliveries_retention ON notification_deliveries(state, updated_at_ms ASC, notification_id ASC);
        CREATE TABLE IF NOT EXISTS agent_messages (
            message_id TEXT PRIMARY KEY NOT NULL,
            idempotency_key TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            author TEXT NOT NULL,
            recipient TEXT NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            trigger_turn INTEGER NOT NULL,
            continuation_id TEXT,
            state TEXT NOT NULL,
            attempt_count INTEGER NOT NULL,
            last_attempt_at_ms INTEGER,
            retry_at_ms INTEGER,
            last_error TEXT,
            delivered_at_ms INTEGER,
            created_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS agent_messages_idempotency
            ON agent_messages(idempotency_key);
        CREATE INDEX IF NOT EXISTS agent_messages_recipient
            ON agent_messages(owner_id, recipient, state, created_at_ms, message_id);
        CREATE INDEX IF NOT EXISTS agent_messages_retry
            ON agent_messages(state, retry_at_ms, message_id);
        CREATE INDEX IF NOT EXISTS agent_messages_retention ON agent_messages(state, expires_at_ms, message_id);
        CREATE INDEX IF NOT EXISTS agent_messages_cleanup ON agent_messages(expires_at_ms, message_id);
        CREATE TABLE IF NOT EXISTS continuations (
            continuation_id TEXT PRIMARY KEY NOT NULL,
            idempotency_key TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            parent_session_id TEXT NOT NULL,
            child_session_id TEXT NOT NULL,
            agent_path TEXT NOT NULL,
            job_ref TEXT,
            execution_ref TEXT,
            terminal_kind TEXT NOT NULL,
            exit_reason TEXT NOT NULL,
            summary TEXT NOT NULL,
            artifact_path TEXT,
            token_input INTEGER,
            token_output INTEGER,
            token_reasoning INTEGER,
            state TEXT NOT NULL,
            consumed_at_ms INTEGER,
            consumed_by TEXT,
            attempt_count INTEGER NOT NULL,
            last_attempt_at_ms INTEGER,
            retry_at_ms INTEGER,
            last_error TEXT,
            created_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS continuations_idempotency
            ON continuations(idempotency_key);
        CREATE INDEX IF NOT EXISTS continuations_owner
            ON continuations(owner_id, state, created_at_ms, continuation_id);
        CREATE INDEX IF NOT EXISTS continuations_child
            ON continuations(child_session_id, created_at_ms DESC, continuation_id);
        CREATE INDEX IF NOT EXISTS continuations_retention ON continuations(state, expires_at_ms, continuation_id);
        CREATE INDEX IF NOT EXISTS continuations_cleanup ON continuations(expires_at_ms, continuation_id);
        CREATE TABLE IF NOT EXISTS user_questions (
            question_id TEXT PRIMARY KEY NOT NULL,
            idempotency_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            interaction_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            tool_call_id TEXT NOT NULL,
            questions_json TEXT NOT NULL,
            handoff_text TEXT,
            state TEXT NOT NULL,
            deadline_at_ms INTEGER NOT NULL,
            answer_json TEXT,
            answered_at_ms INTEGER,
            answered_channel TEXT,
            delivered INTEGER NOT NULL DEFAULT 0,
            delivery_mode TEXT,
            created_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS user_questions_idempotency
            ON user_questions(idempotency_key);
        CREATE INDEX IF NOT EXISTS user_questions_session_state
            ON user_questions(session_id, state, created_at_ms DESC);
        CREATE INDEX IF NOT EXISTS user_questions_reconcile ON user_questions(state, delivered, expires_at_ms);
        CREATE INDEX IF NOT EXISTS user_questions_cleanup ON user_questions(expires_at_ms, question_id);
";
