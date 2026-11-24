//! 网关入站的会话落点解析：显式会话、按 Workspace 的活跃会话、外部事件的固定会话。
//!
//! 与桥接器主体分开：这里只回答「这一轮落到哪条会话、它属于哪个 Workspace」，
//! 不参与事件归一化、订阅与投递。`.axon` 与项目目录都是正经 Workspace，用户选了哪个
//! 就落哪个；只有没有 Workspace 的外部 webhook 才落到服务端固定会话。

use axon_domain::DaemonSessionId;
use axon_session::{NewSessionRequest, RuntimeStore};
use axon_session_runtime::RuntimeClock;

use super::{DaemonGatewayBridge, GATEWAY_EVENT_SESSION_ID, session_cache::SessionAdmission};
use crate::infrastructure::gateway_kernel::GatewayKernelError;

impl<S, C> DaemonGatewayBridge<S, C>
where
    S: RuntimeStore + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
{
    /// 取一条会话的放行判定：先看缓存，未命中才查一次主键并记下。
    pub(super) fn admission_of(
        &self,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<SessionAdmission, GatewayKernelError> {
        if let Some(admission) = self.admissions.get(session_id.as_str(), now_ms) {
            return Ok(admission);
        }
        let session = self
            .dependencies
            .store
            .get_session(session_id)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let admission = SessionAdmission {
            workspace_id: session.workspace_id,
            trust_scope: session.trust_scope,
        };
        self.admissions.put(session_id.as_str(), &admission, now_ms);
        Ok(admission)
    }

    /// 取（必要时登记）网关外部事件的固定会话标识。
    ///
    /// 外部事件没有会话标识可携带，也不能按投递目标派生：它们全部落在一条固定会话上，
    /// 首次到达时创建，此后复用，因此 webhook 历史始终连续。放行判定走缓存：这条会话
    /// 一旦登记就不再变化，后续入站不必每帧回表。
    ///
    /// 这条会话**绑定默认 Workspace**：外部事件的处理根固定是 Axon 根目录
    /// `~/.axon`（provider 脚本在 `~/.axon/events`、正文归档在 `~/.axon/data`），
    /// 因此 `workspace_id` 与 `cwd` 都取该 Workspace：会话清单按归属筛选，留空会让
    /// 桌面端在同一条会话被别端续聊后把它从列表里漏掉；Provider 也需要可访问的工作目录。
    pub(super) fn gateway_event_session(
        &self,
        now_ms: u64,
    ) -> Result<DaemonSessionId, GatewayKernelError> {
        let session_id = DaemonSessionId::new(GATEWAY_EVENT_SESSION_ID)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        if self.admissions.get(session_id.as_str(), now_ms).is_some() {
            return Ok(session_id);
        }
        let session = match self.dependencies.store.get_session(&session_id) {
            Ok(session) => session,
            Err(_) => self
                .dependencies
                .store
                .create_session_with(&axon_session::NewSessionRequest {
                    session_id: session_id.clone(),
                    kind: axon_session::SessionKind::Primary,
                    trust_scope: axon_session::SessionTrustScope::Private,
                    workspace_id: self.default_workspace_id.clone(),
                    root_session_id: None,
                    agent_path: None,
                    goal_id: None,
                    execution_id: None,
                    cwd: self.default_workspace_cwd(),
                    now_ms,
                })
                .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?,
        };
        self.admissions.put(
            session_id.as_str(),
            &SessionAdmission {
                workspace_id: session.workspace_id,
                trust_scope: session.trust_scope,
            },
            now_ms,
        );
        Ok(session_id)
    }

    /// 默认 Workspace 的根目录；解析不到时留空，由 turn 运行路径按所属 Workspace 兜底。
    pub(super) fn default_workspace_cwd(&self) -> String {
        let Some(workspace_id) = self.default_workspace_id.as_deref() else {
            return String::new();
        };
        let Ok(workspace_id) = axon_domain::WorkspaceId::new(workspace_id.to_owned()) else {
            return String::new();
        };
        self.dependencies
            .store
            .get_workspace(&workspace_id)
            .ok()
            .flatten()
            .map_or_else(String::new, |workspace| workspace.workspace_root)
    }

    /// 工作台客户端指定 Workspace 时新建一条主会话，并登记放行缓存。
    ///
    /// `.axon` 与项目目录都是正经 Workspace：用户在下拉里选了哪个，会话就属于哪个，
    /// 会话清单随后按同一列筛选，不会被别端的续聊改写归属。这里只新建、不复用该
    /// Workspace 已有的会话：复用会把两次互不相干的入站悄悄并进同一段历史。
    pub(super) fn create_workspace_session(
        &self,
        workspace_id: &str,
        now_ms: u64,
    ) -> Result<DaemonSessionId, GatewayKernelError> {
        let workspace_id = axon_domain::WorkspaceId::new(workspace_id.to_owned())
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        let workspace = self
            .dependencies
            .store
            .get_workspace(&workspace_id)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?
            .ok_or_else(|| {
                GatewayKernelError::Ingress(format!("Workspace 未登记：{}", workspace_id.as_str()))
            })?;
        let mut created = NewSessionRequest::primary(
            axon_session::new_session_id(),
            workspace.workspace_root.as_str(),
            now_ms,
        );
        created.workspace_id = Some(workspace.workspace_id.as_str().to_owned());
        let session = self
            .dependencies
            .store
            .create_session_with(&created)
            .map_err(|error| GatewayKernelError::Ingress(error.to_string()))?;
        self.admissions.put(
            session.session_id.as_str(),
            &SessionAdmission {
                workspace_id: session.workspace_id.clone(),
                trust_scope: session.trust_scope,
            },
            now_ms,
        );
        Ok(session.session_id)
    }
}
