//! 90 天审计日志游标分页检索器与 CSV/JSON 流式导出（SSOT §5.5 / §2.1 - M4）。
//!
//! 1. Base64 `(recorded_at_ms, rowid)` 不透明游标，确保无跳漏无重复翻页；
//! 2. `pricing_style` 严格通过 `LEFT JOIN pricing_rules` 派生，绝无冗余列；
//! 3. CSV 严格遵循 RFC4180 转义与 UTF-8 无 BOM；
//! 4. JSON 流式写入，杜绝全量常驻内存。

use std::io::Write;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rusqlite::{Connection, params_from_iter, types::Value};

use super::super::observability_model::{AuditLogEvent, AuditLogsReport};
use super::{db_error, from_i64, to_i64};

/// 审计日志分页默认与最大行数。
pub const AUDIT_LOGS_DEFAULT_LIMIT: usize = 50;
pub const AUDIT_LOGS_MAX_LIMIT: usize = 500;

/// 审计日志检索查询参数。
#[derive(Debug, Clone, Default)]
pub struct AuditLogQuery<'a> {
    pub model: Option<&'a str>,
    pub source: Option<&'a str>,
    pub status: Option<&'a str>,
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub cursor: Option<&'a str>,
    pub limit: usize,
}

/// 编码不透明 Base64 游标 `(recorded_at_ms, rowid)`。
#[must_use]
pub fn encode_cursor(recorded_at_ms: u64, rowid: i64) -> String {
    let payload = format!("{recorded_at_ms}:{rowid}");
    BASE64.encode(payload.as_bytes())
}

/// 解码 Base64 游标为 `(recorded_at_ms, rowid)`。
pub fn decode_cursor(cursor: &str) -> Result<(u64, i64), String> {
    let bytes = BASE64
        .decode(cursor.trim())
        .map_err(|e| format!("无效 Base64 游标: {e}"))?;
    let text = std::str::from_utf8(&bytes).map_err(|e| format!("游标非 UTF-8 字符: {e}"))?;
    let (rec_str, rowid_str) = text
        .split_once(':')
        .ok_or_else(|| "游标格式损坏（缺少分隔符）".to_owned())?;
    let recorded_at: u64 = rec_str
        .parse()
        .map_err(|e| format!("游标 recorded_at_ms 无效: {e}"))?;
    let rowid: i64 = rowid_str
        .parse()
        .map_err(|e| format!("游标 rowid 无效: {e}"))?;
    Ok((recorded_at, rowid))
}

/// 执行审计日志分页检索。
pub fn query_audit_logs(
    connection: &Connection,
    query: &AuditLogQuery<'_>,
) -> Result<AuditLogsReport, String> {
    let limit = if query.limit == 0 {
        AUDIT_LOGS_DEFAULT_LIMIT
    } else {
        query.limit.min(AUDIT_LOGS_MAX_LIMIT)
    };

    let cursor_tuple = match query.cursor {
        Some(c) if !c.trim().is_empty() => Some(decode_cursor(c)?),
        _ => None,
    };

    // 1. 统计符合过滤条件的总记录数 (total_count)
    let total_count = count_audit_logs(connection, query)?;

    // 2. 分页查询，取 limit + 1 条判断 has_more
    let mut sql = String::from(
        "SELECT
            f.rowid, f.recorded_at_ms, f.model, f.upstream_model, f.source,
            f.reasoning_effort, f.speed_mode, f.status, f.request_path,
            f.provider_duration_ms, f.ttft_ms, f.generation_speed_tps,
            f.total_input_tokens, f.output_tokens, f.reasoning_output_tokens,
            f.cache_read_input_tokens, f.cache_write_input_tokens,
            COALESCE(p.pricing_style, 'unpriced') AS pricing_style,
            f.cost_micros
         FROM usage_facts f
         LEFT JOIN pricing_rules p ON f.pricing_rule_id = p.id
         WHERE 1=1",
    );

    let mut params: Vec<Value> = Vec::new();
    append_filter_conditions(&mut sql, &mut params, query)?;

    if let Some((c_time, c_rowid)) = cursor_tuple {
        sql.push_str(" AND (f.recorded_at_ms < ? OR (f.recorded_at_ms = ? AND f.rowid < ?))");
        params.push(Value::Integer(to_i64(c_time)?));
        params.push(Value::Integer(to_i64(c_time)?));
        params.push(Value::Integer(c_rowid));
    }

    sql.push_str(" ORDER BY f.recorded_at_ms DESC, f.rowid DESC LIMIT ?");
    params.push(Value::Integer((limit + 1) as i64));

    let mut stmt = connection
        .prepare(&sql)
        .map_err(db_error("准备 usage_facts 审计日志分页查询"))?;

    let rows = stmt
        .query_map(params_from_iter(params), |row| {
            let rowid: i64 = row.get(0)?;
            let recorded_at_ms = from_i64(row.get(1)?)?;
            let model: String = row.get(2)?;
            let upstream_model: Option<String> = row.get(3)?;
            let source: String = row.get(4)?;
            let reasoning_effort: Option<String> = row.get(5)?;
            let speed_mode: Option<String> = row.get(6)?;
            let status: String = row.get(7)?;
            let request_path: Option<String> = row.get(8)?;
            let provider_duration_ms = from_i64(row.get(9)?)?;
            let ttft_raw: Option<i64> = row.get(10)?;
            let ttft_ms = match ttft_raw {
                Some(v) => Some(from_i64(v)?),
                None => None,
            };
            let generation_speed_tps: Option<f64> = row.get(11)?;
            let total_input_tokens = from_i64(row.get(12)?)?;
            let output_tokens = from_i64(row.get(13)?)?;
            let reasoning_output_tokens = from_i64(row.get(14)?)?;
            let cache_read_tokens = from_i64(row.get(15)?)?;
            let cache_write_tokens = from_i64(row.get(16)?)?;
            let pricing_style: String = row.get(17)?;
            let cost_raw: Option<i64> = row.get(18)?;
            let cost_micros = match cost_raw {
                Some(c) => Some(from_i64(c)?),
                None => None,
            };

            let cache_hit_rate_basis_points = if total_input_tokens > 0 {
                ((cache_read_tokens as u128 * 10_000) / total_input_tokens as u128) as u16
            } else {
                0
            };

            let event = AuditLogEvent {
                recorded_at_ms,
                model,
                upstream_model,
                source,
                reasoning_effort,
                speed_mode,
                status,
                request_path,
                total_duration_ms: provider_duration_ms,
                ttft_ms,
                generation_speed_tps,
                input_tokens: total_input_tokens,
                output_tokens,
                reasoning_tokens: reasoning_output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cache_hit_rate_basis_points,
                pricing_style,
                cost_micros,
            };
            Ok((rowid, event))
        })
        .map_err(db_error("执行 usage_facts 审计日志分页查询"))?;

    let mut fetched: Vec<(i64, AuditLogEvent)> = Vec::new();
    for row in rows {
        fetched.push(row.map_err(db_error("读取审计日志行"))?);
    }

    let has_more = fetched.len() > limit;
    if has_more {
        fetched.truncate(limit);
    }

    let next_cursor = if has_more {
        fetched
            .last()
            .map(|(rowid, event)| encode_cursor(event.recorded_at_ms, *rowid))
    } else {
        None
    };

    let events: Vec<AuditLogEvent> = fetched.into_iter().map(|(_, event)| event).collect();

    Ok(AuditLogsReport {
        total_count,
        next_cursor,
        has_more,
        events,
    })
}

fn count_audit_logs(connection: &Connection, query: &AuditLogQuery<'_>) -> Result<u64, String> {
    let mut sql = String::from("SELECT COUNT(*) FROM usage_facts f WHERE 1=1");
    let mut params: Vec<Value> = Vec::new();
    append_filter_conditions(&mut sql, &mut params, query)?;

    connection
        .query_row(&sql, params_from_iter(params), |row| from_i64(row.get(0)?))
        .map_err(db_error("统计 usage_facts 审计日志总数"))
}

fn append_filter_conditions(
    sql: &mut String,
    params: &mut Vec<Value>,
    query: &AuditLogQuery<'_>,
) -> Result<(), String> {
    if let Some(model) = query.model {
        sql.push_str(" AND f.model = ?");
        params.push(Value::Text(model.to_owned()));
    }
    if let Some(source) = query.source {
        sql.push_str(" AND f.source = ?");
        params.push(Value::Text(source.to_owned()));
    }
    if let Some(status) = query.status {
        sql.push_str(" AND f.status = ?");
        params.push(Value::Text(status.to_owned()));
    }
    if let Some(from_ms) = query.from_ms {
        sql.push_str(" AND f.recorded_at_ms >= ?");
        params.push(Value::Integer(to_i64(from_ms)?));
    }
    if let Some(to_ms) = query.to_ms {
        sql.push_str(" AND f.recorded_at_ms <= ?");
        params.push(Value::Integer(to_i64(to_ms)?));
    }
    Ok(())
}

// ============================================================================
// CSV / JSON 流式导出 (SSOT §5.5 / §2.1)
// ============================================================================

/// RFC4180 CSV 字段转义（若包含逗号、双引号、回车换行，则包裹在双引号中并转义内部引号）。
#[must_use]
pub fn escape_csv_field(val: &str) -> String {
    if val.contains(',') || val.contains('"') || val.contains('\r') || val.contains('\n') {
        format!("\"{}\"", val.replace('"', "\"\""))
    } else {
        val.to_owned()
    }
}

/// 流式导出为 CSV（UTF-8 无 BOM，RFC4180 转义）。
pub fn export_audit_logs_csv<W: Write>(
    connection: &Connection,
    query: &AuditLogQuery<'_>,
    mut writer: W,
) -> Result<u64, String> {
    // 写入 CSV 头部 (UTF-8 无 BOM)
    writeln!(
        writer,
        "recorded_at_ms,model,upstream_model,source,reasoning_effort,speed_mode,status,request_path,total_duration_ms,ttft_ms,generation_speed_tps,input_tokens,output_tokens,reasoning_tokens,cache_read_tokens,cache_write_tokens,cache_hit_rate_basis_points,pricing_style,cost_micros"
    )
    .map_err(|e| format!("写入 CSV 头部失败: {e}"))?;

    let mut current_cursor = query.cursor.map(ToOwned::to_owned);
    let mut total_exported = 0_u64;
    let batch_limit = 200;

    loop {
        let mut sub_query = query.clone();
        sub_query.cursor = current_cursor.as_deref();
        sub_query.limit = batch_limit;

        let page = query_audit_logs(connection, &sub_query)?;
        if page.events.is_empty() {
            break;
        }

        for ev in &page.events {
            writeln!(
                writer,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                ev.recorded_at_ms,
                escape_csv_field(&ev.model),
                escape_csv_field(ev.upstream_model.as_deref().unwrap_or("")),
                escape_csv_field(&ev.source),
                escape_csv_field(ev.reasoning_effort.as_deref().unwrap_or("")),
                escape_csv_field(ev.speed_mode.as_deref().unwrap_or("")),
                escape_csv_field(&ev.status),
                escape_csv_field(ev.request_path.as_deref().unwrap_or("")),
                ev.total_duration_ms,
                ev.ttft_ms.map_or(String::new(), |v| v.to_string()),
                ev.generation_speed_tps
                    .map_or(String::new(), |v| format!("{v:.2}")),
                ev.input_tokens,
                ev.output_tokens,
                ev.reasoning_tokens,
                ev.cache_read_tokens,
                ev.cache_write_tokens,
                ev.cache_hit_rate_basis_points,
                escape_csv_field(&ev.pricing_style),
                ev.cost_micros.map_or(String::new(), |v| v.to_string()),
            )
            .map_err(|e| format!("写入 CSV 数据行失败: {e}"))?;
            total_exported += 1;
        }

        if !page.has_more {
            break;
        }
        current_cursor = page.next_cursor;
    }

    writer
        .flush()
        .map_err(|e| format!("刷新 CSV 缓冲区失败: {e}"))?;
    Ok(total_exported)
}

/// 流式导出为 JSON 数组。
pub fn export_audit_logs_json<W: Write>(
    connection: &Connection,
    query: &AuditLogQuery<'_>,
    mut writer: W,
) -> Result<u64, String> {
    write!(writer, "[").map_err(|e| format!("写入 JSON 头部失败: {e}"))?;

    let mut current_cursor = query.cursor.map(ToOwned::to_owned);
    let mut total_exported = 0_u64;
    let batch_limit = 200;
    let mut first = true;

    loop {
        let mut sub_query = query.clone();
        sub_query.cursor = current_cursor.as_deref();
        sub_query.limit = batch_limit;

        let page = query_audit_logs(connection, &sub_query)?;
        if page.events.is_empty() {
            break;
        }

        for ev in &page.events {
            if !first {
                write!(writer, ",").map_err(|e| format!("写入 JSON 分隔符失败: {e}"))?;
            } else {
                first = false;
            }
            serde_json::to_writer(&mut writer, ev)
                .map_err(|e| format!("序列化 JSON 条目失败: {e}"))?;
            total_exported += 1;
        }

        if !page.has_more {
            break;
        }
        current_cursor = page.next_cursor;
    }

    write!(writer, "]").map_err(|e| format!("写入 JSON 尾部失败: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("刷新 JSON 缓冲区失败: {e}"))?;
    Ok(total_exported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_roundtrip() {
        let original_time = 1790752760000_u64;
        let original_rowid = 123456_i64;
        let cursor = encode_cursor(original_time, original_rowid);
        let (decoded_time, decoded_rowid) = decode_cursor(&cursor).expect("解码游标成功");
        assert_eq!(decoded_time, original_time);
        assert_eq!(decoded_rowid, original_rowid);
    }

    #[test]
    fn test_csv_field_escaping() {
        assert_eq!(escape_csv_field("normal_text"), "normal_text");
        assert_eq!(escape_csv_field("hello, world"), "\"hello, world\"");
        assert_eq!(
            escape_csv_field("hello \"quote\""),
            "\"hello \"\"quote\"\"\""
        );
        assert_eq!(escape_csv_field("line1\nline2"), "\"line1\nline2\"");
    }
}
