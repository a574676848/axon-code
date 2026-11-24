//! Pricing rules 仓储持久化实现。

use axon_domain::{ModelPriceRule, PricingStyle, compare_rules_priority};
#[cfg(test)]
use rusqlite::OptionalExtension as _;
use rusqlite::{Connection, params};

use super::{db_error, from_i64, to_i64};

/// 保存或更新单个模型费率规则。
pub(crate) fn save_pricing_rule(
    connection: &Connection,
    rule: &ModelPriceRule,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT OR REPLACE INTO pricing_rules (
                id, model_pattern, pricing_style, input_price_per_m_micros,
                output_price_per_m_micros, cache_read_price_per_m_micros,
                cache_write_price_per_m_micros, multiplier_basis_points,
                priority, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                rule.id,
                rule.model_pattern,
                rule.pricing_style.as_str(),
                to_i64(rule.input_price_per_m_micros)?,
                to_i64(rule.output_price_per_m_micros)?,
                to_i64(rule.cache_read_price_per_m_micros)?,
                to_i64(rule.cache_write_price_per_m_micros)?,
                i64::from(rule.multiplier_basis_points),
                i64::from(rule.priority),
                to_i64(rule.created_at_ms)?,
                to_i64(rule.updated_at_ms)?,
            ],
        )
        .map_err(db_error("保存 pricing rule"))?;
    Ok(())
}

/// 批量保存或更新模型费率规则。
pub(super) fn save_pricing_rules(
    connection: &mut Connection,
    rules: &[ModelPriceRule],
) -> Result<(), String> {
    let transaction = connection
        .transaction()
        .map_err(db_error("启动 save_pricing_rules 事务"))?;
    for rule in rules {
        transaction
            .execute(
                "INSERT OR REPLACE INTO pricing_rules (
                    id, model_pattern, pricing_style, input_price_per_m_micros,
                    output_price_per_m_micros, cache_read_price_per_m_micros,
                    cache_write_price_per_m_micros, multiplier_basis_points,
                    priority, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    rule.id,
                    rule.model_pattern,
                    rule.pricing_style.as_str(),
                    to_i64(rule.input_price_per_m_micros)?,
                    to_i64(rule.output_price_per_m_micros)?,
                    to_i64(rule.cache_read_price_per_m_micros)?,
                    to_i64(rule.cache_write_price_per_m_micros)?,
                    i64::from(rule.multiplier_basis_points),
                    i64::from(rule.priority),
                    to_i64(rule.created_at_ms)?,
                    to_i64(rule.updated_at_ms)?,
                ],
            )
            .map_err(db_error("批量写入 pricing rule"))?;
    }
    transaction
        .commit()
        .map_err(db_error("提交 save_pricing_rules 事务"))?;
    Ok(())
}

/// 根据 ID 查询单个模型费率规则。
#[cfg(test)]
pub(crate) fn get_pricing_rule(
    connection: &Connection,
    id: &str,
) -> Result<Option<ModelPriceRule>, String> {
    connection
        .query_row(
            "SELECT id, model_pattern, pricing_style, input_price_per_m_micros,
                    output_price_per_m_micros, cache_read_price_per_m_micros,
                    cache_write_price_per_m_micros, multiplier_basis_points,
                    priority, created_at_ms, updated_at_ms
             FROM pricing_rules WHERE id = ?1",
            [id],
            map_pricing_rule_row,
        )
        .optional()
        .map_err(db_error("查询 pricing rule"))
}

/// 按四级全序（priority DESC -> pattern字面量 DESC -> updated_at_ms DESC -> id ASC）列出所有规则。
pub(super) fn list_pricing_rules(connection: &Connection) -> Result<Vec<ModelPriceRule>, String> {
    let mut stmt = connection
        .prepare(
            "SELECT id, model_pattern, pricing_style, input_price_per_m_micros,
                    output_price_per_m_micros, cache_read_price_per_m_micros,
                    cache_write_price_per_m_micros, multiplier_basis_points,
                    priority, created_at_ms, updated_at_ms
             FROM pricing_rules",
        )
        .map_err(db_error("准备 list_pricing_rules 查询"))?;

    let rows = stmt
        .query_map([], map_pricing_rule_row)
        .map_err(db_error("执行 list_pricing_rules"))?;

    let mut rules = Vec::new();
    for row in rows {
        rules.push(row.map_err(db_error("读取 pricing rule 记录"))?);
    }

    // 在内存中应用四级全序进行最终排序（确保与 compare_rules_priority 严格一致）
    rules.sort_by(compare_rules_priority);
    Ok(rules)
}

/// 删除单个模型费率规则。
pub(crate) fn delete_pricing_rule(connection: &Connection, id: &str) -> Result<bool, String> {
    let affected = connection
        .execute("DELETE FROM pricing_rules WHERE id = ?1", [id])
        .map_err(db_error("删除 pricing rule"))?;
    Ok(affected > 0)
}

/// 清空所有模型费率规则。
#[cfg(test)]
pub(super) fn delete_all_pricing_rules(connection: &Connection) -> Result<usize, String> {
    connection
        .execute("DELETE FROM pricing_rules", [])
        .map_err(db_error("清空 pricing rules"))
}

fn map_pricing_rule_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelPriceRule> {
    let pricing_style_str: String = row.get(2)?;
    let pricing_style = PricingStyle::parse(&pricing_style_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("未知 pricing_style: {pricing_style_str}"),
            )),
        )
    })?;

    let multiplier_raw: i64 = row.get(7)?;
    let multiplier_basis_points = u32::try_from(multiplier_raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("multiplier 超出 u32 范围: {multiplier_raw}"),
            )),
        )
    })?;

    let priority_raw: i64 = row.get(8)?;
    let priority = i32::try_from(priority_raw).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("priority 超出 i32 范围: {priority_raw}"),
            )),
        )
    })?;

    let input_raw: i64 = row.get(3)?;
    let output_raw: i64 = row.get(4)?;
    let read_raw: i64 = row.get(5)?;
    let write_raw: i64 = row.get(6)?;
    let created_raw: i64 = row.get(9)?;
    let updated_raw: i64 = row.get(10)?;

    Ok(ModelPriceRule {
        id: row.get(0)?,
        model_pattern: row.get(1)?,
        pricing_style,
        input_price_per_m_micros: from_i64(input_raw)?,
        output_price_per_m_micros: from_i64(output_raw)?,
        cache_read_price_per_m_micros: from_i64(read_raw)?,
        cache_write_price_per_m_micros: from_i64(write_raw)?,
        multiplier_basis_points,
        priority,
        created_at_ms: from_i64(created_raw)?,
        updated_at_ms: from_i64(updated_raw)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_memory_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::usage_statistics::repository::schema::initialize_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn test_save_get_and_list_pricing_rules_sorted() {
        let mut conn = in_memory_db();
        let rule1 = ModelPriceRule {
            id: "rule_1".to_string(),
            model_pattern: "gpt-4*".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 2_500_000,
            output_price_per_m_micros: 10_000_000,
            cache_read_price_per_m_micros: 1_250_000,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };
        let rule2 = ModelPriceRule {
            id: "rule_2".to_string(),
            model_pattern: "gpt-4o".to_string(),
            pricing_style: PricingStyle::OpenAi,
            input_price_per_m_micros: 2_500_000,
            output_price_per_m_micros: 10_000_000,
            cache_read_price_per_m_micros: 1_250_000,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 0,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };

        save_pricing_rule(&conn, &rule1).unwrap();
        let fetched = get_pricing_rule(&conn, "rule_1").unwrap();
        assert_eq!(fetched, Some(rule1.clone()));

        save_pricing_rules(&mut conn, std::slice::from_ref(&rule2)).unwrap();

        let list = list_pricing_rules(&conn).unwrap();
        assert_eq!(list.len(), 2);
        // rule2 ("gpt-4o", 6 literal chars) 相比 rule1 ("gpt-4*", 5 literal chars) 优先级更高，排在第一位
        assert_eq!(list[0].id, "rule_2");
        assert_eq!(list[1].id, "rule_1");

        assert!(delete_pricing_rule(&conn, "rule_1").unwrap());
        assert!(!delete_pricing_rule(&conn, "rule_1").unwrap());
        assert_eq!(list_pricing_rules(&conn).unwrap().len(), 1);

        assert_eq!(delete_all_pricing_rules(&conn).unwrap(), 1);
        assert!(list_pricing_rules(&conn).unwrap().is_empty());
    }
}
