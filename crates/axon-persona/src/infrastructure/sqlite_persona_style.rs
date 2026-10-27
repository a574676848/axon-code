//! 人格库的习得风格生命周期：写入、限频发布与退役。

use super::*;

impl PersonaStore {
    /// 写入一条待发布的习得风格。
    ///
    /// 正文超长或为空、与既有条目重复时直接拒绝，不落库。
    ///
    /// # Errors
    ///
    /// 存储不可用时返回 [`PersonaStoreError`]。
    pub fn add_pending_style(
        &mut self,
        owner_id: &str,
        text: &str,
        evidence_ref: &str,
        now_ms: u64,
    ) -> Result<bool, PersonaStoreError> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > crate::constants::MAX_LEARNED_STYLE_CHARS {
            return Ok(false);
        }
        let duplicate: bool = self.connection.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM persona_learned_style
                 WHERE owner_id = ?1 AND text = ?2 AND status != 'retired'
             )",
            params![owner_id, text],
            |row| row.get(0),
        )?;
        if duplicate {
            return Ok(false);
        }
        let style_id = format!("style-{owner_id}-{now_ms}");
        self.connection.execute(
            "INSERT INTO persona_learned_style (
                style_id, owner_id, text, evidence_ref, status, created_at_ms, published_at_ms
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, NULL)",
            params![style_id, owner_id, text, evidence_ref, to_i64(now_ms)?,],
        )?;
        Ok(true)
    }

    /// 按限频发布待生效的习得风格。
    ///
    /// 距上次发布不足 [`crate::constants::STYLE_PUBLISH_INTERVAL_MS`] 时返回 `None`；
    /// 发布成功后把已发布条数收敛到 [`crate::constants::MAX_PUBLISHED_STYLE`]，超出者退役。
    ///
    /// # Errors
    ///
    /// 存储不可用时返回 [`PersonaStoreError`]。
    pub fn publish_style_if_due(
        &mut self,
        owner_id: &str,
        now_ms: u64,
    ) -> Result<Option<u64>, PersonaStoreError> {
        // 限频判定必须在事务内完成：事务外先查后写会让两个并发结算同时越过 24 小时窗口。
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last_published: Option<i64> = transaction
            .query_row(
                "SELECT published_at_ms FROM persona_style_publication WHERE owner_id = ?1",
                params![owner_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(last) = last_published {
            let elapsed = now_ms.saturating_sub(u64::try_from(last).unwrap_or(0));
            if elapsed < crate::constants::STYLE_PUBLISH_INTERVAL_MS {
                transaction.rollback()?;
                return Ok(None);
            }
        }
        let pending = transaction.execute(
            "UPDATE persona_learned_style SET status = 'published', published_at_ms = ?2
             WHERE owner_id = ?1 AND status = 'pending'",
            params![owner_id, to_i64(now_ms)?],
        )?;
        if pending == 0 {
            transaction.rollback()?;
            return Ok(None);
        }
        transaction.execute(
            "UPDATE persona_learned_style SET status = 'retired'
             WHERE owner_id = ?1 AND status = 'published' AND style_id NOT IN (
                 SELECT style_id FROM persona_learned_style
                 WHERE owner_id = ?1 AND status = 'published'
                 ORDER BY published_at_ms DESC, style_id DESC
                 LIMIT ?2
             )",
            params![
                owner_id,
                i64::try_from(crate::constants::MAX_PUBLISHED_STYLE).unwrap_or(i64::MAX)
            ],
        )?;
        let revision: i64 = transaction
            .query_row(
                "SELECT revision FROM persona_style_publication WHERE owner_id = ?1",
                params![owner_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0)
            + 1;
        transaction.execute(
            "INSERT INTO persona_style_publication (owner_id, revision, published_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(owner_id) DO UPDATE SET
                revision = excluded.revision,
                published_at_ms = excluded.published_at_ms",
            params![owner_id, revision, to_i64(now_ms)?],
        )?;
        transaction.commit()?;
        Ok(Some(u64::try_from(revision).unwrap_or(0)))
    }

    /// 列出习得风格；`include_all` 为假时只返回已发布条目。
    ///
    /// # Errors
    ///
    /// 查询失败时返回 [`PersonaStoreError`]。
    pub fn list_styles(
        &self,
        owner_id: &str,
        include_all: bool,
    ) -> Result<Vec<StyleListing>, PersonaStoreError> {
        let mut statement = self.connection.prepare(
            "SELECT style_id, text, status, published_at_ms FROM persona_learned_style
             WHERE owner_id = ?1 AND (?2 OR status = 'published')
             ORDER BY created_at_ms DESC",
        )?;
        let rows = statement.query_map(params![owner_id, include_all], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        let mut listings = Vec::new();
        for row in rows {
            let (style_id, text, status, published_at_ms) = row?;
            listings.push(StyleListing {
                style_id,
                text,
                status,
                published_at_ms: published_at_ms.and_then(|value| u64::try_from(value).ok()),
            });
        }
        Ok(listings)
    }

    /// 退役一条习得风格；条目不存在时返回 `false`。
    ///
    /// # Errors
    ///
    /// 存储不可用时返回 [`PersonaStoreError`]。
    pub fn retire_style(
        &mut self,
        owner_id: &str,
        style_id: &str,
    ) -> Result<bool, PersonaStoreError> {
        let updated = self.connection.execute(
            "UPDATE persona_learned_style SET status = 'retired'
             WHERE owner_id = ?1 AND style_id = ?2 AND status != 'retired'",
            params![owner_id, style_id],
        )?;
        Ok(updated > 0)
    }
}
