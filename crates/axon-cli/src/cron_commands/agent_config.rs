//! Cron 会话首次使用时固化模型与思考强度，后续交互复用同一配置。
use rusqlite::{OptionalExtension as _, params};
use std::path::Path;

use super::{CRON_AGENT_DESTINATION_PREFIX, repository::CronRepository};

pub(crate) fn cron_agent_config(
    config_path: &Path,
    destination: &str,
) -> Result<Option<(String, String)>, String> {
    let Some(job_id) = destination.strip_prefix(CRON_AGENT_DESTINATION_PREFIX) else {
        return Ok(None);
    };
    let (_, context) = crate::remote_commands::load_remote_context(Some(config_path))?;
    let adapter = crate::remote_commands::FeishuCronAdapter::new(context);
    let repository = CronRepository::open(super::CronExecutionAdapter::axon_root(&adapter))?;
    if repository.find_job_any_status(job_id)?.is_none() {
        return Ok(None);
    }
    if let Some(fixed) = repository.read_agent_config(job_id)? {
        return Ok(Some(fixed));
    }
    let config = axon_config::ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("{error:?}"))?;
    repository
        .agent_config(
            job_id,
            &config.config.agent.default_model,
            &config.config.agent.reasoning_effort,
        )
        .map(Some)
}

impl CronRepository {
    pub(crate) fn read_agent_config(
        &self,
        job_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.connection()?
            .query_row(
                "SELECT model, reasoning_effort FROM cron_agent_config WHERE job_id = ?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| format!("读取 Cron 模型配置失败：{error}"))
    }
    pub(crate) fn agent_config(
        &self,
        job_id: &str,
        model: &str,
        effort: &str,
    ) -> Result<(String, String), String> {
        let connection = self.connection()?;
        connection
            .execute(
                "INSERT OR IGNORE INTO cron_agent_config VALUES (?1, ?2, ?3)",
                params![job_id, model, effort],
            )
            .map_err(|error| format!("保存 Cron 模型配置失败：{error}"))?;
        connection
            .query_row(
                "SELECT model, reasoning_effort FROM cron_agent_config WHERE job_id = ?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| format!("读取 Cron 模型配置失败：{error}"))?
            .ok_or_else(|| "Cron 模型配置不存在".to_owned())
    }
}
