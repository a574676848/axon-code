//! Gateway 外部事件增强选项：provider 内容资产、正文归档与主 turn 唤醒钩子。
//!
//! 事件增强属于 Gateway ingress 的准备阶段，与 durable 入账、实时出站桥分离：
//! 本模块只决定「事件正文如何归一化」以及「入账成功后通知谁」。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axon_agent_runtime::ContentArchiveRuntime;

use crate::application::ExternalEventEnvelope;
use crate::infrastructure::event_scripts::EventProviderRegistry;
use crate::infrastructure::gateway_debounce::GATEWAY_DEBOUNCE_DEFAULT_WINDOW;
use crate::infrastructure::gateway_kernel::GatewayKernelError;

/// 外部事件正文内联上限；超过则归档为 Markdown，Interaction 只保留引用。
pub const EVENT_CONTENT_INLINE_LIMIT_CHARS: usize = 4_000;
/// 外部事件正文归档文件后缀。
const EVENT_CONTENT_ARCHIVE_SUFFIX: &str = ".md";
/// 外部事件正文归档流名。
const EVENT_CONTENT_ARCHIVE_STREAM: &str = "event-content";
/// 外部事件正文归档来源追踪标识。
const EVENT_CONTENT_ARCHIVE_SOURCE_TRACE: &str = "event-content";
/// SOP 引用必须携带的强制重读指令：外部事件复用工作区主会话，
/// 历史里的同名 SOP 副本会过期，主 turn 只有本轮重新读取才能拿到最新作业指导。
pub(crate) const EVENT_SOP_BRIEF_MANDATE: &str = "（本会话历史中的同名文档一律视为过期：第一条动作必须用 builtin.read_file 重新读取本文件，并严格按本次读取到的内容执行）";

/// Gateway 事件增强选项：provider 内容资产、正文归档根与防抖窗口。
#[derive(Clone)]
pub struct GatewayEventOptions {
    /// provider 注册表（`~/.axon/events`）。
    pub providers: Arc<EventProviderRegistry>,
    /// 正文归档根目录（`~/.axon/data`）。
    pub archive_root: PathBuf,
    /// 关联键防抖窗口长度。
    pub debounce_window: Duration,
    /// 外部事件 durable 入账成功后的唤醒钩子。
    ///
    /// Gateway Adapter 跑在独立线程上，无法直接通知 Daemon 的主 turn 循环；
    /// 缺少该钩子时新事件只能等定时兜底才发现（平均多 ~0.5s）。
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl GatewayEventOptions {
    /// 使用默认防抖窗口创建选项。
    #[must_use]
    pub fn new(providers: Arc<EventProviderRegistry>, archive_root: impl Into<PathBuf>) -> Self {
        Self {
            providers,
            archive_root: archive_root.into(),
            debounce_window: GATEWAY_DEBOUNCE_DEFAULT_WINDOW,
            waker: None,
        }
    }

    /// 注册外部事件入账成功后的唤醒钩子。
    #[must_use]
    pub fn with_waker(mut self, waker: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.waker = Some(waker);
        self
    }

    /// 通知 primary turn 有新事件已 durable 入账；唤醒是提示而非事实源。
    pub(super) fn wake(&self) {
        if let Some(waker) = self.waker.as_ref() {
            waker();
        }
    }
}

/// 归一化事件正文：超长正文归档为 Markdown，并补上标题、SOP 引用与事件标识。
pub(super) fn prepare_event_content(
    event: &mut ExternalEventEnvelope,
    options: &GatewayEventOptions,
) -> Result<(), GatewayKernelError> {
    let sop_path = options.providers.sop_path(&event.source_channel);
    let original_chars = event.content.chars().count();
    let archived_path = (original_chars > EVENT_CONTENT_INLINE_LIMIT_CHARS)
        .then(|| archive_event_content(event, options))
        .transpose()?;
    if archived_path.is_none() && event.title.is_none() && sop_path.is_none() {
        return Ok(());
    }
    event.content = event_brief(
        event,
        archived_path.as_deref(),
        sop_path.as_deref(),
        original_chars,
    );
    Ok(())
}

fn archive_event_content(
    event: &ExternalEventEnvelope,
    options: &GatewayEventOptions,
) -> Result<String, GatewayKernelError> {
    let archive_root = options.archive_root.as_path();
    let reference = ContentArchiveRuntime::with_suffix(
        EVENT_CONTENT_INLINE_LIMIT_CHARS,
        EVENT_CONTENT_ARCHIVE_SUFFIX,
    )
    .archive_text_with_preview(
        archive_root,
        EVENT_CONTENT_ARCHIVE_STREAM,
        &event.content,
        EVENT_CONTENT_ARCHIVE_SOURCE_TRACE,
        "",
        true,
    )
    .map_err(|error| GatewayKernelError::Preparation(format!("外部事件正文归档失败：{error}")))?;
    let archive_ref = reference
        .output_ref
        .and_then(|output| output.archive_ref)
        .ok_or_else(|| GatewayKernelError::Preparation("外部事件正文归档未返回引用".to_owned()))?;
    Ok(archive_root
        .join(archive_ref.relative_path)
        .to_string_lossy()
        .into_owned())
}

fn event_brief(
    event: &ExternalEventEnvelope,
    archived_path: Option<&str>,
    sop_path: Option<&Path>,
    original_chars: usize,
) -> String {
    let mut lines = vec![format!("外部事件：{}", event.source_channel)];
    if let Some(title) = event.title.as_deref() {
        lines.push(format!("标题：{title}"));
    }
    match archived_path {
        Some(path) => lines.push(format!(
            "正文归档：{path}（原始 {original_chars} 字符；请用 builtin.read_file 按 offset/limit 分页读取，勿一次性读入）"
        )),
        None => lines.push(format!("正文：{}", event.content)),
    }
    if let Some(path) = sop_path {
        lines.push(format!(
            "流程文档：{}{EVENT_SOP_BRIEF_MANDATE}",
            path.display()
        ));
    }
    lines.push(format!("事件标识：{}", event.event_id));
    lines.join("\n")
}
