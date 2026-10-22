//! Feishu 媒体端到端流程。
//!
//! 本模块把媒体下载、上传与消息可消费内容串起来，仍只停留在 Feishu adapter 层。

use std::{fs, io::Read, path::PathBuf};

use axon_domain::{AxonError, AxonResult};

use crate::{
    RemoteFeishuBinaryHttpResponse, RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse,
    RemoteFeishuIncomingMessage, RemoteFeishuMediaDownloadExecutionInput,
    RemoteFeishuMediaDownloadExecutionReport, RemoteFeishuMediaKind,
    RemoteFeishuMediaMessageResourceDownloadInput, RemoteFeishuMediaTransferRuntime,
    RemoteFeishuMediaUploadInput, RemoteFeishuMediaUploadPlan, RemoteFeishuOapiExecutorRuntime,
    RemoteFeishuOutboundMessagePlan,
};

const CONTENT_TYPE_HEADER: &str = "Content-Type";
const MULTIPART_BOUNDARY_PREFIX: &str = "axon-feishu-media";
const FORM_FIELD_IMAGE_TYPE: &str = "image_type";
const FORM_FIELD_FILE_TYPE: &str = "file_type";
const FORM_FIELD_FILE_NAME: &str = "file_name";
const FORM_FIELD_IMAGE: &str = "image";
const FORM_FIELD_FILE: &str = "file";
const OCTET_STREAM: &str = "application/octet-stream";

/// Feishu 入站媒体下载输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuInboundMediaDownloadInput {
    /// 原始入站消息。
    pub message: RemoteFeishuIncomingMessage,
    /// tenant_access_token。
    pub tenant_access_token: String,
    /// OpenAPI 基址。
    pub api_base_url: Option<String>,
    /// 本地媒体缓存根目录。
    pub cache_root: PathBuf,
}

/// Feishu 入站媒体下载报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuInboundMediaDownloadReport {
    /// 注入本地附件路径后的消息。
    pub message: RemoteFeishuIncomingMessage,
    /// 下载报告。
    pub downloads: Vec<RemoteFeishuMediaDownloadExecutionReport>,
}

/// Feishu 媒体上传执行输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaUploadExecutionInput {
    /// 上传请求规划。
    pub plan: RemoteFeishuMediaUploadPlan,
    /// 本地文件路径。
    pub file_path: PathBuf,
}

/// Feishu 媒体上传执行报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaUploadExecutionReport {
    /// 上传请求规划。
    pub plan: RemoteFeishuMediaUploadPlan,
    /// 返回的 image_key 或 file_key。
    pub media_key: String,
    /// 上传字节数。
    pub bytes_uploaded: usize,
}

/// Feishu 出站本地媒体上传并生成消息输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuOutboundMediaMessageInput {
    /// 媒体类型。
    pub kind: RemoteFeishuMediaKind,
    /// 本地文件路径。
    pub file_path: PathBuf,
    /// tenant_access_token。
    pub tenant_access_token: String,
    /// OpenAPI 基址。
    pub api_base_url: Option<String>,
}

/// Feishu 出站本地媒体消息报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuOutboundMediaMessageReport {
    /// 上传报告。
    pub upload: RemoteFeishuMediaUploadExecutionReport,
    /// 上传 key 生成的 Feishu 消息计划。
    pub message: RemoteFeishuOutboundMessagePlan,
}

/// Feishu 媒体端到端流程 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuMediaFlowRuntime;

impl RemoteFeishuMediaFlowRuntime {
    /// 下载入站消息里的所有媒体，并把本地路径注入 agent 可见文本。
    pub fn download_inbound_media_with_http<F>(
        input: RemoteFeishuInboundMediaDownloadInput,
        mut http: F,
    ) -> AxonResult<RemoteFeishuInboundMediaDownloadReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuBinaryHttpResponse>,
    {
        if input.message.media_refs.is_empty() {
            return Ok(RemoteFeishuInboundMediaDownloadReport {
                message: input.message,
                downloads: Vec::new(),
            });
        }
        let mut downloads = Vec::with_capacity(input.message.media_refs.len());
        for media in &input.message.media_refs {
            let request = RemoteFeishuMediaTransferRuntime::plan_message_resource_download(
                RemoteFeishuMediaMessageResourceDownloadInput {
                    message_id: input.message.message_id.clone(),
                    media: media.clone(),
                    tenant_access_token: input.tenant_access_token.clone(),
                    api_base_url: input.api_base_url.clone(),
                },
            )?;
            let report = RemoteFeishuMediaTransferRuntime::execute_download_with_http(
                RemoteFeishuMediaDownloadExecutionInput {
                    request,
                    media: media.clone(),
                    cache_root: input.cache_root.clone(),
                },
                |request| {
                    let response = http(request)?;
                    Ok(response)
                },
            )?;
            downloads.push(report);
        }
        let mut message = input.message;
        append_downloads_to_message_text(&mut message, &downloads);
        Ok(RemoteFeishuInboundMediaDownloadReport { message, downloads })
    }

    /// 使用默认 HTTP 执行器下载媒体（受 `RemoteFeishuOapiExecutorRuntime` 的全局 30s 上界约束）。
    ///
    /// 入站主链等延迟敏感调用方必须改用 [`Self::download_inbound_media_with_http`] 注入
    /// 更短的 per-attachment 与 per-message 预算：附件是消息增强，不允许用全局超时
    /// 占用调用方的调度线程。本方法保留为库的默认入口（测试与离线调用）。
    pub fn download_inbound_media(
        input: RemoteFeishuInboundMediaDownloadInput,
    ) -> AxonResult<RemoteFeishuInboundMediaDownloadReport> {
        Self::download_inbound_media_with_http(
            input,
            RemoteFeishuOapiExecutorRuntime::execute_binary,
        )
    }

    /// 执行媒体上传并解析返回 key。
    pub fn execute_upload_with_http<F>(
        input: RemoteFeishuMediaUploadExecutionInput,
        mut http: F,
    ) -> AxonResult<RemoteFeishuMediaUploadExecutionReport>
    where
        F: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let request = multipart_request(&input)?;
        let bytes_uploaded = request.body.len();
        let response = http(&request)?;
        let media_key = RemoteFeishuMediaTransferRuntime::parse_upload_key(
            &response,
            input.plan.response_key_field,
        )?;
        Ok(RemoteFeishuMediaUploadExecutionReport {
            plan: input.plan,
            media_key,
            bytes_uploaded,
        })
    }

    /// 使用真实 HTTP 执行器上传媒体。
    pub fn execute_upload(
        input: RemoteFeishuMediaUploadExecutionInput,
    ) -> AxonResult<RemoteFeishuMediaUploadExecutionReport> {
        Self::execute_upload_with_http(input, RemoteFeishuOapiExecutorRuntime::execute_multipart)
    }

    /// 上传本地媒体并生成可直接发送的 Feishu 消息。
    pub fn upload_outbound_media_with_http<F>(
        input: RemoteFeishuOutboundMediaMessageInput,
        http: F,
    ) -> AxonResult<RemoteFeishuOutboundMediaMessageReport>
    where
        F: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        validate_outbound_media_input(&input)?;
        let file_name = input
            .file_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(AxonError::runtime_failure("Feishu 出站媒体文件名无效"))?
            .to_owned();
        let plan = RemoteFeishuMediaTransferRuntime::plan_upload(RemoteFeishuMediaUploadInput {
            kind: input.kind,
            file_name,
            tenant_access_token: input.tenant_access_token,
            api_base_url: input.api_base_url,
        })?;
        let upload = Self::execute_upload_with_http(
            RemoteFeishuMediaUploadExecutionInput {
                plan,
                file_path: input.file_path,
            },
            http,
        )?;
        let message = outbound_message_from_upload(&upload);
        Ok(RemoteFeishuOutboundMediaMessageReport { upload, message })
    }

    /// 使用真实 HTTP 执行器上传本地媒体并生成 Feishu 消息。
    pub fn upload_outbound_media(
        input: RemoteFeishuOutboundMediaMessageInput,
    ) -> AxonResult<RemoteFeishuOutboundMediaMessageReport> {
        Self::upload_outbound_media_with_http(
            input,
            RemoteFeishuOapiExecutorRuntime::execute_multipart,
        )
    }
}

/// Feishu multipart HTTP 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMultipartHttpRequest {
    /// 请求 URL。
    pub url: String,
    /// 请求头。
    pub headers: Vec<(String, String)>,
    /// 请求体。
    pub body: Vec<u8>,
}

fn append_downloads_to_message_text(
    message: &mut RemoteFeishuIncomingMessage,
    downloads: &[RemoteFeishuMediaDownloadExecutionReport],
) {
    let mut lines = Vec::new();
    for (index, report) in downloads.iter().enumerate() {
        let media = message.media_refs.get(index);
        let kind = media.map_or("media", |media| media.kind.as_str());
        let file_name = media
            .and_then(|media| media.file_name.as_deref())
            .unwrap_or(kind);
        lines.push(format!(
            "[Attachment: {file_name}]\n{}",
            report.cached_path.display()
        ));
    }
    if !lines.is_empty() {
        message.text = format!("{}\n\n{}", message.text.trim(), lines.join("\n"));
    }
}

fn multipart_request(
    input: &RemoteFeishuMediaUploadExecutionInput,
) -> AxonResult<RemoteFeishuMultipartHttpRequest> {
    validate_upload_input(input)?;
    let file_name = input
        .file_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(AxonError::runtime_failure("Feishu 媒体上传文件名无效"))?;
    let mut file = open_upload_file(&input.file_path, input.plan.max_bytes)?;
    let boundary = format!(
        "{MULTIPART_BOUNDARY_PREFIX}-{}",
        sha2_short_hex(input.file_path.to_string_lossy().as_ref())
    );
    let mut body = Vec::new();
    let value = serde_json::from_str::<serde_json::Value>(&input.plan.request.body_json)
        .map_err(|_| AxonError::runtime_failure("Feishu 媒体上传规划 JSON 无效"))?;
    if let Some(image_type) = value
        .get(FORM_FIELD_IMAGE_TYPE)
        .and_then(|value| value.as_str())
    {
        push_text_part(&mut body, &boundary, FORM_FIELD_IMAGE_TYPE, image_type);
        push_file_part(
            &mut body,
            &boundary,
            FORM_FIELD_IMAGE,
            file_name,
            &mut file,
            input.plan.max_bytes,
        )?;
    } else {
        let file_type = value
            .get(FORM_FIELD_FILE_TYPE)
            .and_then(|value| value.as_str())
            .ok_or(AxonError::runtime_failure("Feishu 媒体上传缺少 file_type"))?;
        push_text_part(&mut body, &boundary, FORM_FIELD_FILE_TYPE, file_type);
        push_text_part(&mut body, &boundary, FORM_FIELD_FILE_NAME, file_name);
        push_file_part(
            &mut body,
            &boundary,
            FORM_FIELD_FILE,
            file_name,
            &mut file,
            input.plan.max_bytes,
        )?;
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let headers = input
        .plan
        .request
        .headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case(CONTENT_TYPE_HEADER))
        .cloned()
        .chain(std::iter::once((
            CONTENT_TYPE_HEADER.to_owned(),
            format!("multipart/form-data; boundary={boundary}"),
        )))
        .collect();
    Ok(RemoteFeishuMultipartHttpRequest {
        url: input.plan.request.url.clone(),
        headers,
        body,
    })
}

fn validate_upload_input(input: &RemoteFeishuMediaUploadExecutionInput) -> AxonResult<()> {
    if input.plan.request.url.trim().is_empty() || !input.file_path.is_file() {
        return Err(AxonError::runtime_failure("Feishu 媒体上传输入无效"));
    }
    Ok(())
}

fn open_upload_file(path: &std::path::Path, max_bytes: usize) -> AxonResult<fs::File> {
    let file = fs::File::open(path)
        .map_err(|_| AxonError::runtime_failure("Feishu 媒体上传文件不可读"))?;
    if file
        .metadata()
        .map_err(|_| AxonError::runtime_failure("Feishu 媒体上传文件不可读"))?
        .len()
        > u64::try_from(max_bytes).expect("媒体上传上限必须能转换为 u64")
    {
        return Err(AxonError::runtime_failure(
            "Feishu 媒体上传文件超过大小上限",
        ));
    }
    Ok(file)
}

fn push_text_part(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

fn push_file_part(
    body: &mut Vec<u8>,
    boundary: &str,
    name: &str,
    file_name: &str,
    file: &mut fs::File,
    max_bytes: usize,
) -> AxonResult<()> {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: {OCTET_STREAM}\r\n\r\n"
        )
        .as_bytes(),
    );
    append_limited_file_bytes(body, file, max_bytes)?;
    body.extend_from_slice(b"\r\n");
    Ok(())
}

fn append_limited_file_bytes(
    body: &mut Vec<u8>,
    file: &mut fs::File,
    max_bytes: usize,
) -> AxonResult<()> {
    append_limited_bytes(body, file, max_bytes)
}

fn append_limited_bytes(body: &mut Vec<u8>, reader: impl Read, max_bytes: usize) -> AxonResult<()> {
    let start = body.len();
    let max_bytes_u64 = u64::try_from(max_bytes).expect("媒体上传上限必须能转换为 u64");
    reader
        .take(max_bytes_u64.saturating_add(1))
        .read_to_end(body)
        .map_err(|_| AxonError::runtime_failure("Feishu 媒体上传文件不可读"))?;
    if body.len().saturating_sub(start) > max_bytes {
        return Err(AxonError::runtime_failure(
            "Feishu 媒体上传文件超过大小上限",
        ));
    }
    Ok(())
}

fn validate_outbound_media_input(input: &RemoteFeishuOutboundMediaMessageInput) -> AxonResult<()> {
    if input.tenant_access_token.trim().is_empty() || !input.file_path.is_file() {
        return Err(AxonError::runtime_failure("Feishu 出站媒体上传输入无效"));
    }
    Ok(())
}

fn outbound_message_from_upload(
    upload: &RemoteFeishuMediaUploadExecutionReport,
) -> RemoteFeishuOutboundMessagePlan {
    let key_field = upload.plan.response_key_field;
    RemoteFeishuOutboundMessagePlan {
        msg_type: upload.plan.outbound_msg_type.to_owned(),
        content_json: serde_json::json!({ key_field: upload.media_key }).to_string(),
    }
}

fn sha2_short_hex(value: &str) -> String {
    axon_crypto::sha256_hex_prefix(value.as_bytes(), 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RemoteFeishuMediaKind, RemoteFeishuMediaRef, RemoteFeishuMediaUploadInput};
    use axon_domain::SessionId;

    #[test]
    fn media_flow_downloads_and_injects_local_paths() {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-feishu-flow-{}", unique_suffix()));
        let message = RemoteFeishuIncomingMessage {
            session_id: SessionId(1),
            client_id: "client".to_owned(),
            request_id: "req".to_owned(),
            message_id: "om_req".to_owned(),
            chat_id: "oc_chat".to_owned(),
            chat_type: crate::RemoteFeishuChatType::Group,
            sender_open_id: "ou_sender".to_owned(),
            mentions: Vec::new(),
            text: "请看附件".to_owned(),
            media_refs: vec![RemoteFeishuMediaRef {
                kind: RemoteFeishuMediaKind::File,
                key: "file_1".to_owned(),
                file_name: Some("报告.pdf".to_owned()),
            }],
            message_kind: crate::RemoteFeishuIncomingMessageKind::Text,
            reply_context: None,
            session_busy: false,
        };
        let report = RemoteFeishuMediaFlowRuntime::download_inbound_media_with_http(
            RemoteFeishuInboundMediaDownloadInput {
                message,
                tenant_access_token: "tenant".to_owned(),
                api_base_url: None,
                cache_root: root,
            },
            |_| {
                Ok(RemoteFeishuBinaryHttpResponse {
                    status: 200,
                    body: b"pdf".to_vec(),
                    content_type: Some("application/pdf".to_owned()),
                })
            },
        )
        .expect("download");

        assert_eq!(report.downloads.len(), 1);
        assert!(report.message.text.contains("[Attachment: 报告.pdf]\n"));
        assert!(!report.message.text.contains("local_path="));
        assert!(!report.message.text.contains("media_type="));
    }

    #[test]
    fn media_flow_downloads_multiple_attachments_into_one_message() {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-feishu-flow-batch-{}", unique_suffix()));
        let mut message = message_with_media("请看多个附件");
        message.media_refs = vec![
            RemoteFeishuMediaRef {
                kind: RemoteFeishuMediaKind::Image,
                key: "img_batch".to_owned(),
                file_name: Some("图一.png".to_owned()),
            },
            RemoteFeishuMediaRef {
                kind: RemoteFeishuMediaKind::File,
                key: "file_batch".to_owned(),
                file_name: Some("报告.pdf".to_owned()),
            },
        ];
        let mut calls = 0usize;
        let report = RemoteFeishuMediaFlowRuntime::download_inbound_media_with_http(
            RemoteFeishuInboundMediaDownloadInput {
                message,
                tenant_access_token: "tenant".to_owned(),
                api_base_url: None,
                cache_root: root,
            },
            |_| {
                calls += 1;
                Ok(RemoteFeishuBinaryHttpResponse {
                    status: 200,
                    body: format!("bytes-{calls}").into_bytes(),
                    content_type: Some("application/octet-stream".to_owned()),
                })
            },
        )
        .expect("download batch");

        assert_eq!(calls, 2);
        assert_eq!(report.downloads.len(), 2);
        assert!(!report.message.text.contains("local_path="));
        assert!(report.message.text.contains("[Attachment: 图一.png]\n"));
        assert!(report.message.text.contains("[Attachment: 报告.pdf]\n"));
    }

    #[test]
    fn multipart_file_reader_stops_after_the_upload_budget() {
        let mut body = Vec::new();

        assert_eq!(
            append_limited_bytes(&mut body, std::io::Cursor::new(b"12345"), 4)
                .expect_err("读取必须受上传预算约束"),
            AxonError::runtime_failure("Feishu 媒体上传文件超过大小上限")
        );
        assert_eq!(body, b"12345");
    }

    #[test]
    fn media_flow_uploads_multipart_and_parses_key() {
        let file_path = axon_test_support::test_temp_root()
            .join(format!("axon-feishu-upload-{}.txt", unique_suffix()));
        fs::write(&file_path, b"hello").expect("fixture");
        let plan = RemoteFeishuMediaTransferRuntime::plan_upload(RemoteFeishuMediaUploadInput {
            kind: RemoteFeishuMediaKind::File,
            file_name: "upload.txt".to_owned(),
            tenant_access_token: "tenant".to_owned(),
            api_base_url: None,
        })
        .expect("plan");
        let report = RemoteFeishuMediaFlowRuntime::execute_upload_with_http(
            RemoteFeishuMediaUploadExecutionInput { plan, file_path },
            |request| {
                assert!(
                    request
                        .body
                        .windows(b"hello".len())
                        .any(|chunk| chunk == b"hello")
                );
                assert!(request.headers.iter().any(|(name, value)| {
                    name == CONTENT_TYPE_HEADER && value.contains("multipart/form-data; boundary=")
                }));
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"file_key":"file_uploaded"}}"#.to_owned(),
                })
            },
        )
        .expect("upload");

        assert_eq!(report.media_key, "file_uploaded");
        assert!(report.bytes_uploaded > 5);
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    }

    fn message_with_media(text: &str) -> RemoteFeishuIncomingMessage {
        RemoteFeishuIncomingMessage {
            session_id: SessionId(1),
            client_id: "client".to_owned(),
            request_id: "req-batch".to_owned(),
            message_id: "om_req_batch".to_owned(),
            chat_id: "oc_chat".to_owned(),
            chat_type: crate::RemoteFeishuChatType::Group,
            sender_open_id: "ou_sender".to_owned(),
            mentions: Vec::new(),
            text: text.to_owned(),
            media_refs: Vec::new(),
            message_kind: crate::RemoteFeishuIncomingMessageKind::Text,
            reply_context: None,
            session_busy: false,
        }
    }
}
