//! 连接分区的 IPC 面：状态快照、试连接、档案增删改、切换与探测。
//!
//! 这里是界面唯一的数据来源：界面不发第二次探测、不自己拼地址、不缓存健康结论。
//! 「一个事实源 + 明确的唯一入口 + 其余落点只引用不重取」是既有共用件先例
//! （版本更新入口）的同一套组织方式。
//!
//! 形状与文案在 [`super::views`]，本文件只做动作编排。

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::endpoint;
use super::probe;
use super::profile::CredentialSource;
use super::relaunch;
use super::runtime;
use super::secret_store::{self, PendingCredential, SecretStoreStatus};
use super::store;
use super::views::{self, ConnectionSnapshot, ConnectionTestResult, ProfileProbeView};

/// 试连接期的一次性凭据：只在这里存活，界面侧不留副本。
fn pending_credential() -> &'static Mutex<Option<PendingCredential>> {
    static PENDING: OnceLock<Mutex<Option<PendingCredential>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

/// 拿一次临时凭据的锁；中毒时取回内部值继续。
fn pending_guard() -> std::sync::MutexGuard<'static, Option<PendingCredential>> {
    pending_credential()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 惰性探测的总预算。
const PROBE_BUDGET: Duration = Duration::from_millis(1500);
/// 试连接与保存档案的总预算：HTTP 面与事件流面共享这一份。
///
/// 它比惰性探测宽，因为这两条路径由用户点击触发、结果要等人看；但仍然是**一份**，
/// 第二个协议面只能用剩下的时间，不会把整次操作拖成两倍。
const INTERACTIVE_BUDGET: Duration = Duration::from_millis(4_000);
/// 惰性探测的并发路数。
const PROBE_CONCURRENCY: usize = 2;
/// 切档后留给界面显示过渡态的时间。
const RELAUNCH_GRACE: Duration = Duration::from_millis(250);

/// HTTP 面闸门：必须拿到 `/meta` 的 Ready 结论。
///
/// 没有 `/meta` 就没有协议版本与能力位，云档案的界面能力只能靠猜，落盘等于把
/// 「看起来能用」当成「确认能用」。
fn http_gate(outcome: &probe::ProbeOutcome) -> Result<(), String> {
    match outcome {
        probe::ProbeOutcome::Ready(_) => Ok(()),
        probe::ProbeOutcome::Failed(failure) => Err(failure.message()),
    }
}

/// 事件流面闸门：端口必须确认可用。
fn ws_gate(outcome: &probe::WsProbeOutcome) -> Result<(), String> {
    if matches!(outcome, probe::WsProbeOutcome::Ready) {
        Ok(())
    } else {
        Err(views::ws_probe_view(outcome).message)
    }
}

/// 保存档案的完整闸门：两个协议面都过才允许落盘。
fn save_gate(
    outcome: &probe::ProbeOutcome,
    ws_outcome: &probe::WsProbeOutcome,
) -> Result<(), String> {
    http_gate(outcome)?;
    ws_gate(ws_outcome)
}

/// 试连接的输入。
///
/// 只收一个 HTTP 地址：事件流地址由后端在 `/api/v1/system/status` 里自报的
/// `webSocketPort` 自动校正，用户不需要知道网关把两个协议面绑在哪两个端口上。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestInput {
    /// 后端地址（HTTP）。
    pub address: String,
    /// API Key；留空表示沿用既有凭据。
    pub api_key: Option<String>,
}

/// 保存档案的输入。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSaveInput {
    /// 档案名。
    pub label: String,
    /// 后端地址（HTTP）。
    pub address: String,
}

/// 切档结果。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SwitchOutcome {
    /// 是否已进入重启流程。
    pub started: bool,
    /// 过渡态文案。
    pub message: String,
}

/// 读取连接快照。
#[tauri::command]
#[must_use]
pub fn connection_state() -> ConnectionSnapshot {
    views::snapshot()
}

/// 重新探测当前生效档案并返回最新快照。
#[tauri::command]
pub fn connection_retry_probe() -> ConnectionSnapshot {
    runtime().mark_probing();
    let outcome = probe::probe();
    runtime().record_probe(&outcome);
    views::snapshot()
}

/// 试连接：不落档案、不落凭据库，失败即丢弃临时凭据。
#[tauri::command]
#[must_use]
pub fn connection_test(input: ConnectionTestInput) -> ConnectionTestResult {
    let token = input
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let normalized = match endpoint::normalize_cloud_address(&input.address) {
        Ok(address) => address,
        Err(message) => {
            return ConnectionTestResult {
                conclusion: "address".to_owned(),
                message,
                endpoint: None,
                ws_endpoint: None,
                ws_conclusion: "notTested".to_owned(),
                ws_message: "地址不合法，本次没有探测事件流端口".to_owned(),
                backend_version: None,
                latency_ms: None,
                capabilities: Vec::new(),
                certificate: "unknown".to_owned(),
                plaintext_private: false,
            };
        }
    };
    // 事件流地址不接受用户输入：先按 HTTP 地址的同一 authority 试，试不通再用
    // 后端自报的端口校正，用户只需要知道一个 HTTP 地址与端口。
    let derived_ws = normalized.ws_base.clone();
    // 两面共享一份预算：HTTP 面用剩的时间才轮到事件流面。
    let deadline = probe::Deadline::after(INTERACTIVE_BUDGET);
    let outcome = probe::probe_within(&normalized.http_base, token, deadline);
    // HTTP 通了才值得问事件流端口：两个协议面是两条独立监听，端口对不上时
    // 「配置能拉、消息能发、却收不到事件帧」是最难排查的一种状态。
    let (resolved_ws, ws_outcome) = match &outcome {
        probe::ProbeOutcome::Failed(_) => (derived_ws, None),
        _ => {
            let (resolved, ws_outcome) =
                probe::resolve_ws_base_within(&normalized.http_base, token, &derived_ws, deadline);
            (resolved, Some(ws_outcome))
        }
    };
    /*
     * 这把 Key 只有在**两面都确认可用**时才留在内存里，等下一次「保存档案」写进
     * 凭据库。试连接失败即丢弃：失败留下任何可被后续保存复用的凭据，都会让保存
     * 闸门形同虚设。
     */
    let accepted = ws_outcome
        .as_ref()
        .is_some_and(|ws| save_gate(&outcome, ws).is_ok());
    *pending_guard() = if accepted {
        token.map(PendingCredential::new)
    } else {
        None
    };
    let ws_view = match ws_outcome {
        Some(outcome) => views::ws_probe_view(&outcome),
        None => views::ws_probe_not_tested(),
    };
    views::build_test_result(
        &normalized.http_base,
        &resolved_ws,
        normalized.plaintext_private,
        &outcome,
        &ws_view,
    )
}

/// 保存云档案：必须先有一份成功的试连接结论。
///
/// # Errors
///
/// 地址不合法、尚未测试成功、凭据库写入失败或状态文件只读。
#[tauri::command]
pub fn connection_save_profile(input: ConnectionSaveInput) -> Result<ConnectionSnapshot, String> {
    /*
     * 待用凭据在最前面一次性取走：地址校验、探活、闸门任何一步失败，它都随局部变量
     * 被丢弃。「测试失败 → 丢弃对象」因此不是靠某个分支的显式清理，而是所有权保证的。
     */
    let pending = pending_guard().take();
    let normalized = endpoint::normalize_cloud_address(&input.address)?;
    let pending_token = pending
        .as_ref()
        .filter(|credential| !credential.is_empty())
        .map(|credential| credential.expose().to_owned());
    /*
     * 保存闸门的前半步：HTTP 面必须自己给出 Ready 结论。只信「上一次试连接成功了」
     * 不够，只看事件流端口也不够：`/meta` 读不到时协议版本与能力位都无从确认，
     * 落下一份「地址能通、协议对不上」的档案比不让保存更糟。
     */
    let deadline = probe::Deadline::after(INTERACTIVE_BUDGET);
    let outcome = probe::probe_within(&normalized.http_base, pending_token.as_deref(), deadline);
    http_gate(&outcome)?;
    /*
     * 后半步：端口校正必须用**这次传来的那把 Key**，不能退回生效档案的凭据。
     * 首次保存时生效档案还是本机，那份凭据是 `.env` 里的全局超管 token，
     * 把它发往用户键入的任意主机既越权，也会因为对方拒绝而让保存必然失败。
     */
    let (ws_base, ws_outcome) = probe::resolve_ws_base_within(
        &normalized.http_base,
        pending_token.as_deref(),
        &normalized.ws_base,
        deadline,
    );
    /*
     * 事件流端口**没有确认可用**就不落档案：只挡住「端口不对」是不够的，只对公网
     * 开放了 HTTP 端口时，WS 探测会得到「连不上」，同样会留下一份「配置能连、
     * 事件收不到」的记录。
     */
    ws_gate(&ws_outcome)?;
    let label = input.label.trim();
    let label = if label.is_empty() {
        "云端后端"
    } else {
        label
    };

    // 写凭据库：成功才落档案，失败即报错且不留脏条目。
    let credential = match pending {
        Some(pending) if !pending.is_empty() => {
            let key_id = uuid::Uuid::new_v4().to_string();
            pending.persist(&key_id)?;
            CredentialSource::Keychain { key_id }
        }
        _ => match secret_store::status() {
            SecretStoreStatus::Available => {
                return Err("请先测试连接并填写 API Key".to_owned());
            }
            SecretStoreStatus::Unavailable { .. } => CredentialSource::SessionOnly,
        },
    };

    runtime().update_document(|document| {
        document.upsert_cloud_profile(label, &normalized.http_base, &ws_base, credential.clone());
        Ok(())
    })?;
    if matches!(credential, CredentialSource::SessionOnly) {
        runtime().set_session_secret(None);
    }
    Ok(views::snapshot())
}

/// 删除档案；生效中的档案不可删。
///
/// # Errors
///
/// 档案不存在、是生效档案，或状态文件只读。
#[tauri::command]
pub fn connection_delete_profile(id: String) -> Result<ConnectionSnapshot, String> {
    if runtime().active_profile_id().as_deref() == Some(id.as_str()) {
        return Err("生效中的档案不可删除，请先切换到别的档案".to_owned());
    }
    let removed = runtime().update_document(|document| {
        document
            .profiles
            .remove(&id)
            .ok_or_else(|| format!("档案不存在：{id}"))
    })?;
    // 删除档案时同步删除凭据库条目，避免留下孤儿凭据。
    if let Some(key_id) = removed.meta.credential.keychain_key()
        && let Err(error) = secret_store::delete(key_id)
    {
        eprintln!("WARNING: 删除凭据库条目失败：{error}");
    }
    Ok(views::snapshot())
}

/// 重命名档案。
///
/// # Errors
///
/// 档案不存在、名称为空或状态文件只读。
#[tauri::command]
pub fn connection_rename_profile(id: String, label: String) -> Result<ConnectionSnapshot, String> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err("档案名称不能为空".to_owned());
    }
    runtime().update_document(|document| {
        let section = document
            .profiles
            .get_mut(&id)
            .ok_or_else(|| format!("档案不存在：{id}"))?;
        section.meta.label = trimmed.to_owned();
        Ok(())
    })?;
    Ok(views::snapshot())
}

/// 记录本次会话内输入的凭据（仅本次会话有效的档案用，不落盘）。
#[tauri::command]
#[must_use]
pub fn connection_set_session_secret(secret: String) -> ConnectionSnapshot {
    let pending = PendingCredential::new(secret);
    if pending.is_empty() {
        runtime().set_session_secret(None);
        runtime().mark_credential_missing();
    } else {
        runtime().set_session_secret(Some(pending.expose().to_owned()));
        // 凭据已在内存里，回到「尚未探测」：用户点重试即可发起第一次带鉴权请求。
        runtime().mark_idle();
    }
    views::snapshot()
}

/// 切换档案：写 id、起中继、退出本进程。
///
/// 前置检查是唯一允许「拒绝切档」的位置：其余任何失败都必须能回到可操作状态。
///
/// # Errors
///
/// 档案不存在或写盘失败。前置检查与 spawn 失败不是错误，而是 `started=false` 的正常返回。
#[tauri::command]
pub fn connection_switch_profile(
    app: tauri::AppHandle,
    id: String,
) -> Result<SwitchOutcome, String> {
    if let Err(reason) = relaunch::resolve_launcher() {
        // 解析失败：不写 lastProfileId、不退出、不拆连接。
        return Ok(SwitchOutcome {
            started: false,
            message: format!("无法自动重启，请手动重启应用（{reason}）"),
        });
    }
    let previous = runtime().active_profile_id();
    runtime().activate_profile(&id)?;
    // 切档前先拆掉旧档案的会话事件流：中继要等本进程退出，而退出前这段时间里
    // 旧连接不该继续往 WebView 投递另一个后端的事件帧。
    crate::conversation::stream::shared().close();
    if let Err(reason) = relaunch::spawn_relay(std::process::id()) {
        // spawn 失败：回写 lastProfileId、恢复连接、留在原进程，绝不退出。
        if let Some(previous) = previous {
            let _ = runtime().activate_profile(&previous);
        }
        // 事件流按需重建：界面恢复后第一次 watch 会重新建连，因此这里只需要
        // 让健康结论回到「正在探测」并重新探一次当前档案。
        runtime().mark_probing();
        let outcome = probe::probe();
        runtime().record_probe(&outcome);
        return Ok(SwitchOutcome {
            started: false,
            message: format!("无法自动重启，已取消切换（{reason}）"),
        });
    }
    // 给界面一点时间显示过渡态，再用 app.exit(0) 退出：它是唯一会走窗口几何
    // 退出保存的路径，std::process::exit 会丢掉最后一次窗口几何。
    let exit_handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(RELAUNCH_GRACE);
        exit_handle.exit(0);
    });
    Ok(SwitchOutcome {
        started: true,
        message: "正在重启以切换后端".to_owned(),
    })
}

/// 记录引导完成。
///
/// # Errors
///
/// 状态文件只读或写盘失败。
#[tauri::command]
pub fn connection_complete_onboarding() -> Result<ConnectionSnapshot, String> {
    runtime().complete_onboarding()?;
    Ok(views::snapshot())
}

/// 跳过引导：进入未连接态，但不白屏、不崩。
///
/// # Errors
///
/// 状态文件只读或写盘失败。
#[tauri::command]
pub fn connection_skip_onboarding() -> Result<ConnectionSnapshot, String> {
    runtime().complete_onboarding()?;
    Ok(views::snapshot())
}

/// 惰性探测全部档案：只在切换面板展开时调用，最多两路并发，总预算 1500 毫秒。
#[tauri::command]
#[must_use]
pub fn connection_probe_profiles() -> Vec<ProfileProbeView> {
    let profiles = runtime().profiles();
    let started = Instant::now();
    let mut results = Vec::with_capacity(profiles.len());
    let mut index = 0;
    while index < profiles.len() {
        let batch_end = (index + PROBE_CONCURRENCY).min(profiles.len());
        // 每一路只用「剩余预算」做超时：预算按总量算，不是按批各给一份。
        let remaining = PROBE_BUDGET.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            for entry in &profiles[index..] {
                results.push(over_budget_view(&entry.id));
            }
            break;
        }
        let per_probe = remaining.min(endpoint::PROBE_TIMEOUT);
        let handles: Vec<_> = profiles[index..batch_end]
            .iter()
            .map(|entry| {
                let entry = entry.clone();
                let token = token_for(&entry.profile.credential);
                std::thread::spawn(move || {
                    let endpoints = endpoint::resolve(&entry.profile.spec);
                    (
                        entry.id,
                        probe::probe_base_with_token_timeout(
                            &endpoints.http_base,
                            token.as_deref(),
                            per_probe,
                        ),
                    )
                })
            })
            .collect();
        for (offset, handle) in handles.into_iter().enumerate() {
            let profile_id = profiles[index + offset].id.clone();
            if started.elapsed() >= PROBE_BUDGET {
                results.push(over_budget_view(&profile_id));
                continue;
            }
            match handle.join() {
                Ok((id, outcome)) => results.push(views::probe_view(id, &outcome)),
                Err(_) => results.push(ProfileProbeView {
                    id: profile_id,
                    conclusion: "unreachable".to_owned(),
                    message: "探测线程异常退出".to_owned(),
                    latency_ms: None,
                }),
            }
        }
        index = batch_end;
    }
    results
}

/// 超过探测预算的档案读数。
fn over_budget_view(id: &str) -> ProfileProbeView {
    ProfileProbeView {
        id: id.to_owned(),
        conclusion: "overBudget".to_owned(),
        message: "超过探测预算，按未连接呈现".to_owned(),
        latency_ms: None,
    }
}

/// 取某条档案的凭据。
fn token_for(source: &CredentialSource) -> Option<String> {
    match source {
        CredentialSource::LocalEnvFile => endpoint::api_token(),
        // 非生效档案的会话凭据不存在：探测会因为 401 落在「凭据无效」，这是真话。
        CredentialSource::SessionOnly => None,
        CredentialSource::Keychain { key_id } => secret_store::load(key_id).ok().flatten(),
    }
}

/// 启动期消费失败记录并回滚档案。
///
/// 回滚目标可能已被删除，因此回滚前必须校验其存在；不存在时保持当前档案不变，
/// 绝不把 `lastProfileId` 指向一个不存在的 id。
#[must_use]
pub fn apply_pending_relaunch_failure() -> Option<String> {
    let record = relaunch::consume_pending_failure()?;
    match runtime().rollback_profile() {
        Ok(true) => Some(format!(
            "上次切换到「{}」时无法自动重启，已回到原档案",
            record.target_profile_id
        )),
        Ok(false) => Some("上次切换未完成，但原档案已不存在，已保持当前档案".to_owned()),
        Err(error) => Some(format!("上次切换未完成，回滚失败：{error}")),
    }
}

/// 状态文件路径，供排查使用。
#[must_use]
pub fn state_file_display() -> String {
    store::state_file_path().display().to_string()
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod commands_tests;
