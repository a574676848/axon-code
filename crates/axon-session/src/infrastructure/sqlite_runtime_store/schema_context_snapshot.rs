//! 控制面库中「会话级上下文快照」投影表的 DDL。
//!
//! 单列成模块的理由与 [`super::schema_outbox`] 相同：主 schema 模块承载建库流程与版本
//! 分派，表定义按领域拆开，谁的表谁写在一处。本表是投影：一行对应一条会话，缺失的行
//! 表示这条会话还没有出现过可用的上下文读数，读侧必须把它与「字段未知」区分开。

/// 会话级上下文快照表。
///
/// 列全部可空：未知就是未知，绝不用上一次的读数填进来。`source` 记录写入这份读数的帧
/// 类别，`updated_at_ms` 记录写入时刻，界面据此显示「更新于 N 秒前」。
pub(super) const CONTEXT_SNAPSHOT_DDL: &str = "
        CREATE TABLE IF NOT EXISTS context_snapshots (
            session_id TEXT PRIMARY KEY NOT NULL,
            current_context_tokens INTEGER,
            context_window INTEGER,
            composition_json TEXT,
            rollover_before_tokens INTEGER,
            rollover_after_tokens INTEGER,
            source TEXT,
            updated_at_ms INTEGER,
            FOREIGN KEY(session_id) REFERENCES sessions(session_id)
        );
";
