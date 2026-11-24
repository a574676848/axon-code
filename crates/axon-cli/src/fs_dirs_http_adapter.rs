//! 服务端目录浏览路由：为「选择一个目录」这类交互列出子目录。
//!
//! 与文件列表的区别有两点：入参是**绝对路径**，而且不要求该目录已注册为工作区
//! （选目录时目标通常还没注册）。因此这条路由的边界不靠注册表，只靠归一化后的
//! 形态判定：非法形态与越界一律 403，判不出来的目标 404。
//!
//! 附件目录自成一类根（合同 §6.3.1）：命中 `<运行时根>/data/attachments` 及其
//! 子目录一律 403，否则附件会被当成源码目录浏览。
//!
//! 性能契约（P5）：只列子目录、只列一层、单层最多 500 条，不递归预取。

use std::path::PathBuf;

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde_json::{Value, json};

use crate::server_path::{
    PathDialect, Rejection, is_attachments_path, query_parameter, resolve_absolute,
    strip_verbatim_prefix,
};

/// 目录浏览路由。
const DIRS_PATH: &str = "/api/v1/fs/dirs";
/// 单层最多返回的子目录数。
const MAX_DIRECTORY_ENTRIES: usize = 500;
/// 方法不匹配的错误码。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 缺少必填查询参数的错误码。
const MISSING_PARAMETER: &str = "missing_parameter";
/// 查询参数形态非法的错误码。
const INVALID_PARAMETER: &str = "invalid_parameter";
/// 目标不存在或不是目录的错误码。
const NOT_FOUND: &str = "not_found";
/// 目标不允许访问的错误码。
const FORBIDDEN: &str = "forbidden";
/// 读取失败的兜底错误码。
const READ_FAILED: &str = "read_failed";

/// 目录浏览适配器。
pub(crate) struct FsDirsHttpAdapter {
    /// Axon 运行时根目录；附件目录据此判定。
    axon_root: PathBuf,
}

impl FsDirsHttpAdapter {
    /// 以运行时根目录构造。
    pub(crate) fn new(axon_root: impl Into<PathBuf>) -> Self {
        Self {
            axon_root: axon_root.into(),
        }
    }

    /// 列出某个绝对目录的直接子目录。
    fn browse(&self, query: Option<&str>) -> GatewayHttpResponse {
        let target = match query_parameter(query, "path") {
            Some(value) if !value.trim().is_empty() => value,
            _ => {
                return GatewayHttpResponse::json(
                    400,
                    &json!({
                        "error": MISSING_PARAMETER,
                        "message": "缺少必填查询参数 path（绝对路径）",
                    }),
                );
            }
        };
        let dialect = PathDialect::host();
        let directory = match resolve_absolute(&target, dialect) {
            Ok(directory) => directory,
            Err(rejection) => return rejection_response(rejection),
        };
        if is_attachments_path(&directory, &self.axon_root, dialect) {
            return GatewayHttpResponse::json(
                403,
                &json!({
                    "error": FORBIDDEN,
                    "message": "附件目录自成一类根，不得被当作可浏览目录",
                }),
            );
        }
        if !directory.is_dir() {
            return GatewayHttpResponse::json(
                404,
                &json!({ "error": NOT_FOUND, "message": "目标不是目录" }),
            );
        }

        let reader = match std::fs::read_dir(&directory) {
            Ok(reader) => reader,
            Err(error) => return read_failure("读取目录", error),
        };
        let mut dirs: Vec<(String, Value)> = Vec::new();
        for item in reader {
            if dirs.len() > MAX_DIRECTORY_ENTRIES {
                break;
            }
            let Ok(item) = item else {
                continue;
            };
            // 只要目录：文件由文件列表路由负责，这里混进来只会让选择框出现不可选项。
            if !item.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = item.file_name().to_string_lossy().to_string();
            let entry_path = strip_verbatim_prefix(&item.path().to_string_lossy()).to_owned();
            dirs.push((name.clone(), json!({ "name": name, "path": entry_path })));
        }
        let truncated = dirs.len() > MAX_DIRECTORY_ENTRIES;
        dirs.sort_by(|left, right| left.0.cmp(&right.0));
        dirs.truncate(MAX_DIRECTORY_ENTRIES);

        // 父目录到盘根为止；盘根没有父目录，回 null 而不是回自己。
        let parent = directory
            .parent()
            .filter(|parent| *parent != directory.as_path())
            .map(|parent| strip_verbatim_prefix(&parent.to_string_lossy()).to_owned());
        // 截断必须可见：否则客户端会把「只看到 500 个」当成「只有 500 个」。
        GatewayHttpResponse::json(
            200,
            &json!({
                "path": strip_verbatim_prefix(&directory.to_string_lossy()),
                "parent": parent,
                "dirs": dirs.into_iter().map(|(_, entry)| entry).collect::<Vec<Value>>(),
                "truncated": truncated,
            }),
        )
    }
}

impl GatewayHttpRouteHandler for FsDirsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        _body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        if path != DIRS_PATH {
            return None;
        }
        if method != "GET" {
            return Some(GatewayHttpResponse::json(
                405,
                &json!({ "error": METHOD_NOT_ALLOWED }),
            ));
        }
        Some(self.browse(query))
    }
}

/// 归一化拒绝结论 -> 响应：越界与非法形态按拒绝码给出，理由一律走中文。
fn rejection_response(rejection: Rejection) -> GatewayHttpResponse {
    let status = rejection.denied_status();
    let code = match status {
        403 => FORBIDDEN,
        404 => NOT_FOUND,
        _ => INVALID_PARAMETER,
    };
    GatewayHttpResponse::json(
        status,
        &json!({ "error": code, "message": rejection.message() }),
    )
}

/// 文件系统错误 -> 响应：不存在是 404，无权限是 403，其余如实报 500。
fn read_failure(action: &str, error: std::io::Error) -> GatewayHttpResponse {
    let message = format!("{action}失败：{error}");
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            GatewayHttpResponse::json(404, &json!({ "error": NOT_FOUND, "message": message }))
        }
        std::io::ErrorKind::PermissionDenied => {
            GatewayHttpResponse::json(403, &json!({ "error": FORBIDDEN, "message": message }))
        }
        _ => GatewayHttpResponse::json(500, &json!({ "error": READ_FAILED, "message": message })),
    }
}

#[cfg(test)]
#[path = "fs_dirs_http_adapter_tests.rs"]
mod fs_dirs_http_adapter_tests;
