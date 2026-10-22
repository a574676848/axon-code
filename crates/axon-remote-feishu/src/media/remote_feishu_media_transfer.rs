//! Feishu 媒体下载与上传请求规划。
//!
//! 本模块只承接 Feishu OpenAPI 媒体请求与 adapter 本地缓存，不污染 Domain。

use std::path::{Path, PathBuf};

use axon_crypto::sha256_hex_prefix;
use axon_domain::{AxonError, AxonResult};

use crate::{
    REMOTE_FEISHU_API_BASE_URL, RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse,
    RemoteFeishuMediaKind, RemoteFeishuMediaRef,
};

/// Feishu 下载图片资源路径。
pub const REMOTE_FEISHU_IMAGE_DOWNLOAD_PATH: &str = "/open-apis/im/v1/images";
/// Feishu 下载文件资源路径。
pub const REMOTE_FEISHU_FILE_DOWNLOAD_PATH: &str = "/open-apis/im/v1/files";
/// Feishu 消息资源下载路径。
pub const REMOTE_FEISHU_MESSAGE_RESOURCE_DOWNLOAD_PATH: &str = "/open-apis/im/v1/messages";
/// Feishu 上传图片路径。
pub const REMOTE_FEISHU_IMAGE_UPLOAD_PATH: &str = "/open-apis/im/v1/images";
/// Feishu 上传文件路径。
pub const REMOTE_FEISHU_FILE_UPLOAD_PATH: &str = "/open-apis/im/v1/files";
/// Feishu 图片消息上传类型，对齐 oapi-sdk-go channel uploader。
pub const REMOTE_FEISHU_IMAGE_UPLOAD_TYPE_MESSAGE: &str = "message";
/// Feishu 文件流上传类型，对齐 oapi-sdk-go channel uploader。
pub const REMOTE_FEISHU_FILE_UPLOAD_TYPE_STREAM: &str = "stream";
/// Feishu 图片接口单个媒体的最大字节数。
pub const REMOTE_FEISHU_IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Feishu 文件接口单个媒体的最大字节数。
pub const REMOTE_FEISHU_FILE_MAX_BYTES: usize = 30 * 1024 * 1024;

const AUTHORIZATION_HEADER: &str = "Authorization";
const BEARER_PREFIX: &str = "Bearer ";
const CONTENT_TYPE_HEADER: &str = "Content-Type";
const APPLICATION_JSON: &str = "application/json";
const MULTIPART_FORM_DATA: &str = "multipart/form-data";
const DISPATCH_ID_MEDIA_TRANSFER: &str = "feishu-media-transfer";
const MEDIA_CACHE_DIR: &str = "remote_feishu_media";
const DEFAULT_IMAGE_EXTENSION: &str = "jpg";
const DEFAULT_FILE_EXTENSION: &str = "bin";
const DEFAULT_AUDIO_EXTENSION: &str = "ogg";
const DEFAULT_MEDIA_EXTENSION: &str = "mp4";
const DEFAULT_STICKER_EXTENSION: &str = "webp";
const MAX_FILE_NAME_CHARS: usize = 128;

/// Feishu 媒体下载请求规划输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaDownloadInput {
    /// Feishu 媒体引用。
    pub media: RemoteFeishuMediaRef,
    /// tenant_access_token。
    pub tenant_access_token: String,
    /// OpenAPI 基址。
    pub api_base_url: Option<String>,
}

/// Feishu 消息资源下载请求规划输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaMessageResourceDownloadInput {
    /// Feishu message_id。
    pub message_id: String,
    /// Feishu 媒体引用。
    pub media: RemoteFeishuMediaRef,
    /// tenant_access_token。
    pub tenant_access_token: String,
    /// OpenAPI 基址。
    pub api_base_url: Option<String>,
}

/// Feishu 媒体上传请求规划输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaUploadInput {
    /// 媒体类型。
    pub kind: RemoteFeishuMediaKind,
    /// 文件名。
    pub file_name: String,
    /// tenant_access_token。
    pub tenant_access_token: String,
    /// OpenAPI 基址。
    pub api_base_url: Option<String>,
}

/// Feishu 媒体上传请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaUploadPlan {
    /// 上传请求。
    pub request: RemoteFeishuHttpRequestPlan,
    /// 当前上传接口允许的最大字节数。
    pub max_bytes: usize,
    /// 成功响应里的 key 字段。
    pub response_key_field: &'static str,
    /// 上传后的消息类型。
    pub outbound_msg_type: &'static str,
}

/// Feishu 媒体下载执行输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaDownloadExecutionInput {
    /// 下载请求计划。
    pub request: RemoteFeishuHttpRequestPlan,
    /// Feishu 媒体引用。
    pub media: RemoteFeishuMediaRef,
    /// 缓存根目录。
    pub cache_root: PathBuf,
}

/// Feishu 媒体下载执行报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMediaDownloadExecutionReport {
    /// 下载请求。
    pub request: RemoteFeishuHttpRequestPlan,
    /// 缓存文件路径。
    pub cached_path: PathBuf,
    /// 写入字节数。
    pub bytes_written: usize,
    /// 响应 Content-Type。
    pub content_type: Option<String>,
}

/// Feishu 二进制 HTTP 响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuBinaryHttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应体。
    pub body: Vec<u8>,
    /// Content-Type。
    pub content_type: Option<String>,
}

/// Feishu 媒体 transfer runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuMediaTransferRuntime;

impl RemoteFeishuMediaTransferRuntime {
    /// 规划媒体下载请求。
    pub fn plan_download(
        input: RemoteFeishuMediaDownloadInput,
    ) -> AxonResult<RemoteFeishuHttpRequestPlan> {
        validate_token(&input.tenant_access_token)?;
        validate_media_ref(&input.media)?;
        let base = normalized_base_url(input.api_base_url.as_deref())?;
        let path = match input.media.kind {
            RemoteFeishuMediaKind::Image => REMOTE_FEISHU_IMAGE_DOWNLOAD_PATH,
            RemoteFeishuMediaKind::File
            | RemoteFeishuMediaKind::Audio
            | RemoteFeishuMediaKind::Media
            | RemoteFeishuMediaKind::Sticker => REMOTE_FEISHU_FILE_DOWNLOAD_PATH,
        };
        Ok(RemoteFeishuHttpRequestPlan {
            dispatch_id: DISPATCH_ID_MEDIA_TRANSFER.to_owned(),
            url: format!("{base}{path}/{}", input.media.key),
            method: "GET".to_owned(),
            headers: bearer_headers(&input.tenant_access_token, None),
            body_json: String::new(),
        })
    }

    /// 规划消息资源下载请求。
    pub fn plan_message_resource_download(
        input: RemoteFeishuMediaMessageResourceDownloadInput,
    ) -> AxonResult<RemoteFeishuHttpRequestPlan> {
        validate_token(&input.tenant_access_token)?;
        validate_message_id(&input.message_id)?;
        validate_media_ref(&input.media)?;
        let base = normalized_base_url(input.api_base_url.as_deref())?;
        Ok(RemoteFeishuHttpRequestPlan {
            dispatch_id: DISPATCH_ID_MEDIA_TRANSFER.to_owned(),
            url: format!(
                "{base}{}/{}/resources/{}?type={}",
                REMOTE_FEISHU_MESSAGE_RESOURCE_DOWNLOAD_PATH,
                input.message_id,
                input.media.key,
                input.media.kind.as_str()
            ),
            method: "GET".to_owned(),
            headers: bearer_headers(&input.tenant_access_token, None),
            body_json: String::new(),
        })
    }

    /// 规划媒体上传请求。
    pub fn plan_upload(
        input: RemoteFeishuMediaUploadInput,
    ) -> AxonResult<RemoteFeishuMediaUploadPlan> {
        validate_token(&input.tenant_access_token)?;
        if input.file_name.trim().is_empty() {
            return Err(AxonError::runtime_failure("Feishu 媒体上传文件名无效"));
        }
        let base = normalized_base_url(input.api_base_url.as_deref())?;
        let (path, file_type, max_bytes, response_key_field, outbound_msg_type) = match input.kind {
            RemoteFeishuMediaKind::Image => (
                REMOTE_FEISHU_IMAGE_UPLOAD_PATH,
                REMOTE_FEISHU_IMAGE_UPLOAD_TYPE_MESSAGE,
                REMOTE_FEISHU_IMAGE_MAX_BYTES,
                "image_key",
                "image",
            ),
            RemoteFeishuMediaKind::Audio => (
                REMOTE_FEISHU_FILE_UPLOAD_PATH,
                REMOTE_FEISHU_FILE_UPLOAD_TYPE_STREAM,
                REMOTE_FEISHU_FILE_MAX_BYTES,
                "file_key",
                "audio",
            ),
            RemoteFeishuMediaKind::Media => (
                REMOTE_FEISHU_FILE_UPLOAD_PATH,
                REMOTE_FEISHU_FILE_UPLOAD_TYPE_STREAM,
                REMOTE_FEISHU_FILE_MAX_BYTES,
                "file_key",
                "media",
            ),
            RemoteFeishuMediaKind::File => (
                REMOTE_FEISHU_FILE_UPLOAD_PATH,
                REMOTE_FEISHU_FILE_UPLOAD_TYPE_STREAM,
                REMOTE_FEISHU_FILE_MAX_BYTES,
                "file_key",
                "file",
            ),
            RemoteFeishuMediaKind::Sticker => {
                return Err(AxonError::runtime_failure(
                    "Feishu sticker 仅支持入站识别与下载，不支持出站上传",
                ));
            }
        };
        let body_json = serde_json::json!({
            "file_name": input.file_name,
            "file_type": file_type,
        })
        .to_string();
        Ok(RemoteFeishuMediaUploadPlan {
            request: RemoteFeishuHttpRequestPlan {
                dispatch_id: DISPATCH_ID_MEDIA_TRANSFER.to_owned(),
                url: format!("{base}{path}"),
                method: "POST".to_owned(),
                headers: bearer_headers(&input.tenant_access_token, Some(MULTIPART_FORM_DATA)),
                body_json,
            },
            max_bytes,
            response_key_field,
            outbound_msg_type,
        })
    }

    /// 执行下载并写入本地媒体缓存。
    pub fn execute_download_with_http<F>(
        input: RemoteFeishuMediaDownloadExecutionInput,
        mut http: F,
    ) -> AxonResult<RemoteFeishuMediaDownloadExecutionReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuBinaryHttpResponse>,
    {
        validate_media_ref(&input.media)?;
        validate_cache_root(&input.cache_root)?;
        validate_download_request(&input.request)?;
        let response = http(&input.request)?;
        validate_binary_response(&response, input.media.kind)?;
        let cached_path = cached_media_path(
            &input.cache_root,
            &input.media,
            response.content_type.as_deref(),
        );
        crate::remote_feishu_media_cache_budget::write_media_cache_file(
            &input.cache_root,
            &cached_path,
            &response.body,
        )?;
        Ok(RemoteFeishuMediaDownloadExecutionReport {
            request: input.request,
            cached_path,
            bytes_written: response.body.len(),
            content_type: response.content_type,
        })
    }

    /// 从普通 HTTP 响应里解析媒体上传返回 key。
    pub fn parse_upload_key(
        response: &RemoteFeishuHttpResponse,
        key_field: &str,
    ) -> AxonResult<String> {
        if response.status != 200 {
            return Err(AxonError::runtime_failure("Feishu 媒体上传响应状态码无效"));
        }
        let value = serde_json::from_str::<serde_json::Value>(&response.body)
            .map_err(|_| AxonError::runtime_failure("Feishu 媒体上传响应 JSON 无效"))?;
        if value["code"].as_i64().unwrap_or_default() != 0 {
            return Err(AxonError::runtime_failure("Feishu 媒体上传业务响应失败"));
        }
        value["data"][key_field]
            .as_str()
            .map(str::to_owned)
            .filter(|key| !key.trim().is_empty())
            .ok_or(AxonError::runtime_failure("Feishu 媒体上传响应缺少 key"))
    }
}

fn validate_token(token: &str) -> AxonResult<()> {
    if token.trim().is_empty() {
        return Err(AxonError::runtime_failure(
            "Feishu tenant_access_token 无效",
        ));
    }
    Ok(())
}

fn validate_media_ref(media: &RemoteFeishuMediaRef) -> AxonResult<()> {
    if media.key.trim().is_empty() {
        return Err(AxonError::runtime_failure("Feishu 媒体 key 无效"));
    }
    Ok(())
}

fn validate_message_id(message_id: &str) -> AxonResult<()> {
    if message_id.trim().is_empty() {
        return Err(AxonError::runtime_failure("Feishu message_id 无效"));
    }
    Ok(())
}

fn validate_cache_root(cache_root: &Path) -> AxonResult<()> {
    if cache_root.as_os_str().is_empty() {
        return Err(AxonError::runtime_failure("Feishu 媒体缓存根目录无效"));
    }
    Ok(())
}

fn validate_download_request(request: &RemoteFeishuHttpRequestPlan) -> AxonResult<()> {
    if request.method != "GET" || request.url.trim().is_empty() {
        return Err(AxonError::runtime_failure("Feishu 媒体下载请求无效"));
    }
    Ok(())
}

fn validate_binary_response(
    response: &RemoteFeishuBinaryHttpResponse,
    kind: RemoteFeishuMediaKind,
) -> AxonResult<()> {
    if response.status != 200
        || response.body.is_empty()
        || response.body.len() > media_max_bytes(kind)
    {
        return Err(AxonError::runtime_failure("Feishu 媒体下载响应无效"));
    }
    Ok(())
}

const fn media_max_bytes(kind: RemoteFeishuMediaKind) -> usize {
    match kind {
        RemoteFeishuMediaKind::Image => REMOTE_FEISHU_IMAGE_MAX_BYTES,
        RemoteFeishuMediaKind::File
        | RemoteFeishuMediaKind::Audio
        | RemoteFeishuMediaKind::Media
        | RemoteFeishuMediaKind::Sticker => REMOTE_FEISHU_FILE_MAX_BYTES,
    }
}

fn normalized_base_url(api_base_url: Option<&str>) -> AxonResult<String> {
    let base = api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL).trim();
    if !base.starts_with("http://") && !base.starts_with("https://") {
        return Err(AxonError::runtime_failure("Feishu OpenAPI 基址无效"));
    }
    Ok(base.trim_end_matches('/').to_owned())
}

fn bearer_headers(token: &str, content_type: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![(
        AUTHORIZATION_HEADER.to_owned(),
        format!("{BEARER_PREFIX}{}", token.trim()),
    )];
    headers.push((
        CONTENT_TYPE_HEADER.to_owned(),
        content_type.unwrap_or(APPLICATION_JSON).to_owned(),
    ));
    headers
}

fn cached_media_path(
    cache_root: &Path,
    media: &RemoteFeishuMediaRef,
    content_type: Option<&str>,
) -> PathBuf {
    let extension = resolved_extension(media, content_type);
    let file_name = safe_file_name(media, &extension);
    cache_root
        .join(MEDIA_CACHE_DIR)
        .join(media.kind.as_str())
        .join(file_name)
}

fn safe_file_name(media: &RemoteFeishuMediaRef, extension: &str) -> String {
    let digest = short_sha256_hex(&media.key);
    let stem = media
        .file_name
        .as_deref()
        .map(file_stem)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| media.kind.as_str().to_owned());
    let safe_stem = sanitize_file_stem(&stem);
    format!("{digest}_{safe_stem}.{extension}")
}

fn file_stem(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name)
        .to_owned()
}

fn sanitize_file_stem(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars().take(MAX_FILE_NAME_CHARS) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    if output.trim_matches('_').is_empty() {
        "media".to_owned()
    } else {
        output
    }
}

fn resolved_extension(media: &RemoteFeishuMediaRef, content_type: Option<&str>) -> String {
    extension_from_file_name(media.file_name.as_deref())
        .or_else(|| extension_from_content_type(content_type))
        .unwrap_or_else(|| default_extension(media.kind).to_owned())
}

fn extension_from_file_name(file_name: Option<&str>) -> Option<String> {
    Path::new(file_name?)
        .extension()
        .and_then(|value| value.to_str())
        .map(sanitize_extension)
        .filter(|value| !value.is_empty())
}

fn extension_from_content_type(content_type: Option<&str>) -> Option<String> {
    let normalized = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "image/png" => Some("png".to_owned()),
        "image/jpeg" | "image/jpg" => Some("jpg".to_owned()),
        "image/gif" => Some("gif".to_owned()),
        "image/webp" => Some("webp".to_owned()),
        "audio/ogg" => Some("ogg".to_owned()),
        "audio/mpeg" => Some("mp3".to_owned()),
        "video/mp4" => Some("mp4".to_owned()),
        "text/plain" => Some("txt".to_owned()),
        "application/pdf" => Some("pdf".to_owned()),
        _ => None,
    }
}

fn sanitize_extension(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(16)
        .collect()
}

const fn default_extension(kind: RemoteFeishuMediaKind) -> &'static str {
    match kind {
        RemoteFeishuMediaKind::Image => DEFAULT_IMAGE_EXTENSION,
        RemoteFeishuMediaKind::File => DEFAULT_FILE_EXTENSION,
        RemoteFeishuMediaKind::Audio => DEFAULT_AUDIO_EXTENSION,
        RemoteFeishuMediaKind::Media => DEFAULT_MEDIA_EXTENSION,
        RemoteFeishuMediaKind::Sticker => DEFAULT_STICKER_EXTENSION,
    }
}

fn short_sha256_hex(value: &str) -> String {
    sha256_hex_prefix(value.as_bytes(), 16)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn media_transfer_plans_image_download_like_oapi_channel() {
        let request =
            RemoteFeishuMediaTransferRuntime::plan_download(RemoteFeishuMediaDownloadInput {
                media: RemoteFeishuMediaRef {
                    kind: RemoteFeishuMediaKind::Image,
                    key: "img_123".to_owned(),
                    file_name: None,
                },
                tenant_access_token: "tenant-token".to_owned(),
                api_base_url: Some("https://open.feishu.cn/".to_owned()),
            })
            .expect("download");

        assert_eq!(request.method, "GET");
        assert!(request.url.contains(REMOTE_FEISHU_IMAGE_DOWNLOAD_PATH));
        assert!(request.url.ends_with("/open-apis/im/v1/images/img_123"));
        assert!(
            request
                .headers
                .iter()
                .any(|(_, value)| value == "Bearer tenant-token")
        );
    }

    #[test]
    fn media_transfer_plans_file_download_like_oapi_channel() {
        let request =
            RemoteFeishuMediaTransferRuntime::plan_download(RemoteFeishuMediaDownloadInput {
                media: RemoteFeishuMediaRef {
                    kind: RemoteFeishuMediaKind::Audio,
                    key: "file_123".to_owned(),
                    file_name: Some("voice.ogg".to_owned()),
                },
                tenant_access_token: "tenant-token".to_owned(),
                api_base_url: None,
            })
            .expect("download");

        assert!(request.url.contains(REMOTE_FEISHU_FILE_DOWNLOAD_PATH));
        assert!(request.url.ends_with("/open-apis/im/v1/files/file_123"));
    }

    #[test]
    fn media_transfer_plans_message_resource_download() {
        let request = RemoteFeishuMediaTransferRuntime::plan_message_resource_download(
            RemoteFeishuMediaMessageResourceDownloadInput {
                message_id: "om_123".to_owned(),
                media: RemoteFeishuMediaRef {
                    kind: RemoteFeishuMediaKind::Audio,
                    key: "file_123".to_owned(),
                    file_name: Some("voice.ogg".to_owned()),
                },
                tenant_access_token: "tenant-token".to_owned(),
                api_base_url: None,
            },
        )
        .expect("message resource download");

        assert!(
            request
                .url
                .ends_with("/open-apis/im/v1/messages/om_123/resources/file_123?type=audio")
        );
    }

    #[test]
    fn media_transfer_plans_sticker_message_resource_download() {
        let request = RemoteFeishuMediaTransferRuntime::plan_message_resource_download(
            RemoteFeishuMediaMessageResourceDownloadInput {
                message_id: "om_123".to_owned(),
                media: RemoteFeishuMediaRef {
                    kind: RemoteFeishuMediaKind::Sticker,
                    key: "sticker_123".to_owned(),
                    file_name: None,
                },
                tenant_access_token: "tenant-token".to_owned(),
                api_base_url: None,
            },
        )
        .expect("sticker message resource download");

        assert!(
            request
                .url
                .ends_with("/open-apis/im/v1/messages/om_123/resources/sticker_123?type=sticker")
        );
    }

    #[test]
    fn media_transfer_plans_upload_keys_and_outbound_types() {
        let image = RemoteFeishuMediaTransferRuntime::plan_upload(RemoteFeishuMediaUploadInput {
            kind: RemoteFeishuMediaKind::Image,
            file_name: "screen.png".to_owned(),
            tenant_access_token: "tenant-token".to_owned(),
            api_base_url: None,
        })
        .expect("image upload");
        let media = RemoteFeishuMediaTransferRuntime::plan_upload(RemoteFeishuMediaUploadInput {
            kind: RemoteFeishuMediaKind::Media,
            file_name: "demo.mp4".to_owned(),
            tenant_access_token: "tenant-token".to_owned(),
            api_base_url: None,
        })
        .expect("media upload");

        assert_eq!(image.max_bytes, REMOTE_FEISHU_IMAGE_MAX_BYTES);
        assert_eq!(image.response_key_field, "image_key");
        assert!(image.request.url.contains(REMOTE_FEISHU_IMAGE_UPLOAD_PATH));
        assert_eq!(media.max_bytes, REMOTE_FEISHU_FILE_MAX_BYTES);
        assert_eq!(media.response_key_field, "file_key");
        assert_eq!(media.outbound_msg_type, "media");
        assert!(media.request.url.contains(REMOTE_FEISHU_FILE_UPLOAD_PATH));
    }

    #[test]
    fn media_transfer_executes_download_into_safe_cache_path() {
        let request = RemoteFeishuMediaTransferRuntime::plan_message_resource_download(
            RemoteFeishuMediaMessageResourceDownloadInput {
                message_id: "om_123".to_owned(),
                media: RemoteFeishuMediaRef {
                    kind: RemoteFeishuMediaKind::File,
                    key: "file_key_123".to_owned(),
                    file_name: Some("报告 2026.pdf".to_owned()),
                },
                tenant_access_token: "tenant-token".to_owned(),
                api_base_url: None,
            },
        )
        .expect("request");
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-feishu-media-cache-{}", unique_suffix()));
        let report = RemoteFeishuMediaTransferRuntime::execute_download_with_http(
            RemoteFeishuMediaDownloadExecutionInput {
                request,
                media: RemoteFeishuMediaRef {
                    kind: RemoteFeishuMediaKind::File,
                    key: "file_key_123".to_owned(),
                    file_name: Some("报告 2026.pdf".to_owned()),
                },
                cache_root: root.clone(),
            },
            |_| {
                Ok(RemoteFeishuBinaryHttpResponse {
                    status: 200,
                    body: b"pdf-bytes".to_vec(),
                    content_type: Some("application/pdf".to_owned()),
                })
            },
        )
        .expect("download");

        assert_eq!(report.bytes_written, 9);
        assert!(report.cached_path.starts_with(root.join(MEDIA_CACHE_DIR)));
        assert!(report.cached_path.to_string_lossy().ends_with(".pdf"));
        assert_eq!(fs::read(&report.cached_path).expect("cached"), b"pdf-bytes");
    }

    #[test]
    fn media_transfer_rejects_a_response_above_the_media_type_budget() {
        let response = RemoteFeishuBinaryHttpResponse {
            status: 200,
            body: vec![0_u8; REMOTE_FEISHU_IMAGE_MAX_BYTES + 1],
            content_type: None,
        };

        assert_eq!(
            validate_binary_response(&response, RemoteFeishuMediaKind::Image)
                .expect_err("超限图片必须拒绝"),
            AxonError::runtime_failure("Feishu 媒体下载响应无效")
        );
        assert!(validate_binary_response(&response, RemoteFeishuMediaKind::File).is_ok());
    }

    #[test]
    fn media_transfer_parses_upload_key() {
        let key = RemoteFeishuMediaTransferRuntime::parse_upload_key(
            &RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"data":{"file_key":"file_uploaded"}}"#.to_owned(),
            },
            "file_key",
        )
        .expect("key");

        assert_eq!(key, "file_uploaded");
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    }
}
