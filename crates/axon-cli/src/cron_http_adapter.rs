use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_domain::{CronJob, CronJobInput, CronJobStatus, CronJobUpdate, CronRunRecord};
use axon_session::{WorkspaceResolver, session_lane_id};
use serde::Deserialize;
use std::path::PathBuf;

use crate::cron_commands::{
    CRON_AGENT_CHANNEL, MEMORY_CONSOLIDATION_CRON_JOB_ID, SESSION_CLEANUP_CRON_JOB_ID,
    execute_and_persist, is_system_job, local_cron_session_key, parse_wake,
    repository::CronRepository,
    schedule::{parse_schedule, require_next_run},
    suggested_feishu_chat_id, wake_text,
};
use crate::remote_commands::{FeishuCronAdapter, load_remote_context};
use crate::usage_statistics::USAGE_ROLLUP_CRON_JOB_ID;

const CRON_PATH: &str = "/api/v1/crons";
const DEFAULT_HISTORY_LIMIT: usize = 50;
const MAX_HISTORY_LIMIT: usize = 200;
const MAX_LIST_LIMIT: usize = 500;
const DEFAULT_LIST_LIMIT: usize = 20;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CronCreateRequest {
    name: String,
    schedule: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    skills: Vec<String>,
    /// 唤醒目标：`feishu:<chat_id>` 或 `session:<channel>:<destination>`；省略时不投递。
    wake: Option<String>,
    workdir: Option<String>,
    #[serde(default)]
    no_agent: bool,
    script: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CronUpdateRequest {
    name: Option<String>,
    schedule: Option<String>,
    prompt: Option<String>,
    skills: Option<Vec<String>>,
    /// 新的唤醒目标。
    wake: Option<String>,
    /// 是否替换唤醒目标；传 `wake` 时自动视为替换。
    #[serde(default)]
    replace_wake: bool,
    workdir: Option<String>,
    #[serde(default)]
    replace_workdir: bool,
    no_agent: Option<bool>,
    script: Option<String>,
    #[serde(default)]
    replace_script: bool,
}

pub struct CronHttpAdapter {
    axon_root: PathBuf,
    config_path: PathBuf,
}

impl CronHttpAdapter {
    pub fn new(axon_root: impl Into<PathBuf>, config_path: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
            config_path: config_path.into(),
        }
    }

    fn repository(&self) -> Result<CronRepository, String> {
        CronRepository::open(&self.axon_root)
    }

    fn response(result: Result<serde_json::Value, CronHttpError>) -> GatewayHttpResponse {
        match result {
            Ok(payload) => GatewayHttpResponse::json(200, &payload),
            Err(error) => GatewayHttpResponse::json(
                error.status(),
                &serde_json::json!({ "error": error.message() }),
            ),
        }
    }

    fn list(&self, query: Option<&str>) -> Result<serde_json::Value, CronHttpError> {
        let page_size = query_value(query, "pageSize")
            .or_else(|| query_value(query, "limit"))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .clamp(1, MAX_LIST_LIMIT);
        let page = query_value(query, "page")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let status = query_value(query, "status").unwrap_or_default();
        if !matches!(status.as_str(), "" | "active" | "paused" | "completed") {
            return Err(CronHttpError::bad_request("任务状态筛选无效"));
        }
        let search = query_value(query, "search").unwrap_or_default();
        let (jobs, total) = self
            .repository()?
            .jobs_page(&status, search.trim(), page_size, offset)
            .map_err(CronHttpError::internal)?;
        let jobs = jobs
            .into_iter()
            .map(|read| self.render_job(read.job, read.latest_run, read.running))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(serde_json::json!({
            "jobs": jobs, "total": total, "page": page, "pageSize": page_size,
            "hasMore": offset.saturating_add(jobs.len()) < total,
        }))
    }

    fn get(&self, job_id: &str) -> Result<serde_json::Value, CronHttpError> {
        let job = self
            .repository()?
            .find_job(job_id)
            .map_err(CronHttpError::internal)?
            .ok_or_else(|| CronHttpError::not_found("未找到 Cron 任务"))?;
        self.job_payload(job)
    }

    fn job_payload(&self, job: CronJob) -> Result<serde_json::Value, CronHttpError> {
        let latest_run = self
            .repository()?
            .latest_run_for_job(&job.job_id)
            .map_err(CronHttpError::internal)?;
        let running = self
            .repository()?
            .is_running(&job.job_id, axon_time::unix_now_ms())?;
        self.render_job(job, latest_run, running)
    }

    fn render_job(
        &self,
        job: CronJob,
        latest_run: Option<CronRunRecord>,
        running: bool,
    ) -> Result<serde_json::Value, CronHttpError> {
        let system = is_system_job(&job.job_id);
        let workspace_path = job
            .workdir
            .as_deref()
            .map(std::path::Path::new)
            .unwrap_or(&self.axon_root);
        let workspace = WorkspaceResolver
            .resolve(workspace_path, axon_time::unix_now_ms())
            .map_err(|error| CronHttpError::internal(format!("解析 Cron 工作区失败：{error}")))?;
        let destination = local_cron_session_key(&job);
        let fixed = if job.no_agent {
            None
        } else {
            crate::cron_commands::cron_agent_config(&self.config_path, &destination)?
        };
        let session_id = session_lane_id(
            workspace.workspace_id.as_str(),
            &axon_domain::AudienceContext::new(
                axon_domain::AudienceKind::Private,
                axon_domain::DisclosurePolicy::PrivateOnly,
            ),
            CRON_AGENT_CHANNEL,
            &destination,
        );
        let mut payload =
            serde_json::to_value(&job).map_err(|e| CronHttpError::internal(e.to_string()))?;
        if let Some(object) = payload.as_object_mut() {
            // 对外契约与 `--wake` 同款字符串；domain 的序列化形态是带标签对象，
            // 直接透出会让前端与 CLI 拿到两种形状。
            object.insert(
                "wake".to_owned(),
                match job.wake.as_ref() {
                    Some(wake) => serde_json::Value::String(wake_text(wake)),
                    None => serde_json::Value::Null,
                },
            );
            object.insert("system".to_owned(), serde_json::Value::Bool(system));
            object.insert(
                "sessionId".to_owned(),
                serde_json::Value::String(session_id),
            );
            object.insert(
                "workspaceId".to_owned(),
                serde_json::Value::String(workspace.workspace_id.as_str().to_owned()),
            );
            object.insert(
                "workspacePath".to_owned(),
                serde_json::json!(workspace_path),
            );
            object.insert("destination".to_owned(), serde_json::json!(destination));
            object.insert("running".to_owned(), serde_json::json!(running));
            object.insert(
                "model".to_owned(),
                serde_json::json!(fixed.as_ref().map(|(model, _)| model)),
            );
            object.insert(
                "reasoningEffort".to_owned(),
                serde_json::json!(fixed.as_ref().map(|(_, effort)| effort)),
            );
            object.insert("canDelete".to_owned(), serde_json::Value::Bool(!system));
            object.insert(
                "lastRunSuccess".to_owned(),
                latest_run
                    .as_ref()
                    .map(|run| serde_json::Value::Bool(run.success))
                    .unwrap_or(serde_json::Value::Null),
            );
            object.insert(
                "lastRunError".to_owned(),
                latest_run
                    .and_then(|run| run.error_summary)
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
        }
        Ok(payload)
    }

    fn create(&self, body: &[u8]) -> Result<serde_json::Value, CronHttpError> {
        let request: CronCreateRequest = serde_json::from_slice(body)
            .map_err(|e| CronHttpError::bad_request(format!("解析 Cron 创建请求失败：{e}")))?;
        let now = axon_time::unix_now_ms();
        let schedule = parse_schedule(&request.schedule).map_err(CronHttpError::bad_request)?;
        let job_id = format!(
            "axon.user.{}",
            axon_crypto::sha256_hex_prefix(format!("{}:{now}", request.name.trim()).as_bytes(), 20)
        );
        let wake = request
            .wake
            .as_deref()
            .map(parse_wake)
            .transpose()
            .map_err(CronHttpError::bad_request)?;
        let job = CronJob::new(
            CronJobInput {
                job_id,
                name: request.name,
                schedule: request.schedule,
                prompt: request.prompt,
                skills: request.skills,
                wake,
                workdir: request.workdir,
                no_agent: request.no_agent,
                script: request.script,
            },
            now,
            Some(require_next_run(&schedule, now).map_err(CronHttpError::bad_request)?),
        )
        .map_err(|e| CronHttpError::bad_request(e.to_string()))?;
        self.repository()?
            .save_job(&job)
            .map_err(CronHttpError::internal)?;
        self.job_payload(job)
    }

    fn update(&self, job_id: &str, body: &[u8]) -> Result<serde_json::Value, CronHttpError> {
        if is_system_job(job_id) {
            return Err(CronHttpError::conflict("系统 Cron 只能由 Axon 管理"));
        }
        let request: CronUpdateRequest = serde_json::from_slice(body)
            .map_err(|e| CronHttpError::bad_request(format!("解析 Cron 更新请求失败：{e}")))?;
        let repository = self.repository()?;
        let current = repository
            .find_job(job_id)
            .map_err(CronHttpError::internal)?
            .ok_or_else(|| CronHttpError::not_found("未找到 Cron 任务"))?;
        let schedule_text = request.schedule.as_deref().unwrap_or(&current.schedule);
        let schedule = parse_schedule(schedule_text).map_err(CronHttpError::bad_request)?;
        let has_schedule = request.schedule.is_some();
        let wake = request
            .wake
            .as_deref()
            .map(parse_wake)
            .transpose()
            .map_err(CronHttpError::bad_request)?;
        let update = CronJobUpdate {
            name: request.name,
            schedule: request.schedule,
            prompt: request.prompt,
            skills: request.skills,
            wake,
            replace_wake: request.replace_wake || request.wake.is_some(),
            workdir: request.workdir,
            replace_workdir: request.replace_workdir,
            no_agent: request.no_agent,
            script: request.script,
            replace_script: request.replace_script,
        };
        let now = axon_time::unix_now_ms();
        let next_run_at_ms = if has_schedule {
            Some(require_next_run(&schedule, now).map_err(CronHttpError::bad_request)?)
        } else {
            current.next_run_at_ms
        };
        let updated = current
            .with_update(update, now, next_run_at_ms)
            .map_err(|e| CronHttpError::bad_request(e.to_string()))?;
        repository
            .save_job(&updated)
            .map_err(CronHttpError::internal)?;
        self.job_payload(updated)
    }

    fn set_status(
        &self,
        job_id: &str,
        status: CronJobStatus,
    ) -> Result<serde_json::Value, CronHttpError> {
        let repository = self.repository()?;
        let current = repository
            .find_job(job_id)
            .map_err(CronHttpError::internal)?
            .ok_or_else(|| CronHttpError::not_found("未找到 Cron 任务"))?;
        let updated = current.with_status(status, axon_time::unix_now_ms());
        repository
            .save_job(&updated)
            .map_err(CronHttpError::internal)?;
        self.job_payload(updated)
    }

    fn remove(&self, job_id: &str) -> Result<serde_json::Value, CronHttpError> {
        if is_system_job(job_id) {
            return Err(CronHttpError::conflict("系统 Cron 无法删除"));
        }
        self.set_status(job_id, CronJobStatus::Removed)
    }

    fn runs(&self, job_id: &str, query: Option<&str>) -> Result<serde_json::Value, CronHttpError> {
        let page_size = query_value(query, "pageSize")
            .or_else(|| query_value(query, "limit"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_HISTORY_LIMIT)
            .clamp(1, MAX_HISTORY_LIMIT);
        let page = query_value(query, "page")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let repository = self.repository()?;
        if repository
            .find_job_any_status(job_id)
            .map_err(CronHttpError::internal)?
            .is_none()
        {
            return Err(CronHttpError::not_found("未找到 Cron 任务"));
        }
        let (runs, total) = repository
            .runs_page(job_id, page_size, offset)
            .map_err(CronHttpError::internal)?;
        Ok(serde_json::json!({
            "jobId": job_id,
            "runs": runs,
            "total": total,
            "page": page,
            "pageSize": page_size,
            "hasMore": offset.saturating_add(runs.len()) < total,
        }))
    }

    fn run_now(&self, job_id: &str) -> Result<serde_json::Value, CronHttpError> {
        let repository = self.repository()?;
        let (_, context) =
            load_remote_context(Some(&self.config_path)).map_err(CronHttpError::internal)?;
        let adapter = FeishuCronAdapter::new(context);
        let claim = repository
            .claim_manual(job_id, axon_time::unix_now_ms())
            .map_err(CronHttpError::conflict)?;
        let job = self.job_payload(claim.job.clone())?;
        std::thread::spawn(move || {
            if let Err(error) = execute_and_persist(&adapter, &repository, &claim, false) {
                axon_logger::error(&format!("手动 Cron 执行收尾失败：{error}"));
            }
        });
        Ok(serde_json::json!({ "accepted": true, "job": job }))
    }

    fn config(&self) -> Result<serde_json::Value, CronHttpError> {
        Ok(serde_json::json!({
            "axonRoot": self.axon_root,
            "configPath": self.config_path,
            "storePath": self.repository()?.path(),
            "systemJobIds": [
                MEMORY_CONSOLIDATION_CRON_JOB_ID,
                SESSION_CLEANUP_CRON_JOB_ID,
                USAGE_ROLLUP_CRON_JOB_ID,
            ],
            // 仅作提示：任务的实际投递目标永远来自它自己声明的 wake。
            "defaultNotificationChatId": suggested_feishu_chat_id(&self.config_path),
        }))
    }
}

impl GatewayHttpRouteHandler for CronHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let suffix = path.strip_prefix(CRON_PATH)?;
        let parts = suffix
            .trim_matches('/')
            .split('/')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        let result = match (method, parts.as_slice()) {
            ("GET", []) => self.list(query),
            ("GET", ["config"]) => self.config(),
            ("POST", []) => self.create(body),
            ("GET", [id]) => self.get(id),
            ("POST", [id]) => self.update(id, body),
            ("DELETE", [id]) => self.remove(id),
            ("GET", [id, "runs"]) => self.runs(id, query),
            ("POST", [id, "pause"]) => self.set_status(id, CronJobStatus::Paused),
            ("POST", [id, "resume"]) => self.set_status(id, CronJobStatus::Active),
            ("POST", [id, "run"]) => self.run_now(id),
            _ => return None,
        };
        if method == "POST" && matches!(parts.as_slice(), [_, "run"]) {
            return Some(match result {
                Ok(payload) => GatewayHttpResponse::json(202, &payload),
                Err(error) => Self::response(Err(error)),
            });
        }
        Some(Self::response(result))
    }
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

#[derive(Debug)]
struct CronHttpError {
    status: u16,
    message: String,
}
impl CronHttpError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: message.into(),
        }
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: 409,
            message: message.into(),
        }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            message: message.into(),
        }
    }
    fn status(&self) -> u16 {
        self.status
    }
    fn message(&self) -> &str {
        &self.message
    }
}
impl From<String> for CronHttpError {
    fn from(message: String) -> Self {
        Self::internal(message)
    }
}
