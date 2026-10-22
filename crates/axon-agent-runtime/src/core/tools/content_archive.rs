//! 内容归档与有界预览。
//!
//! Build/Test 输出可能很长，模型只应收到有界 preview；完整输出写入本机归档路径，
//! 后续由 CLI/session 回放按引用查看。

use axon_crypto::{Sha256Hex, sha256_hex};
use axon_domain::{OutputArchiveReference, OutputRef, OutputRefId};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// 默认预览最大字符数。
pub const CONTENT_ARCHIVE_DEFAULT_PREVIEW_CHARS: usize = 16_384;
/// 默认内容归档目录名。
pub const CONTENT_ARCHIVE_DIR_NAME: &str = "archive";
/// 结构化工具结果流名。
pub const CONTENT_ARCHIVE_RESULT_STREAM: &str = "result";
/// 工具输出来源追踪前缀。
pub const TOOL_OUTPUT_SOURCE_TRACE_PREFIX: &str = "tool://";
/// 归档文件默认后缀；原始进程输出使用 `.log`。
pub const CONTENT_ARCHIVE_DEFAULT_FILE_SUFFIX: &str = ".log";
const CONTENT_ARCHIVE_TEMP_SUFFIX: &str = ".tmp";
const CONTENT_ARCHIVE_COPY_BUFFER_BYTES: usize = 64 * 1024;
const OUTPUT_REF_ID_HEX_LEN: usize = 16;

/// 归档后的工具输出引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputReference {
    /// stdout/stderr 或其他稳定流名。
    pub stream: String,
    /// 有界预览。
    pub preview: String,
    /// 是否被截断。
    pub truncated: bool,
    /// 原始字符数。
    pub original_chars: usize,
    /// 预览字符数。
    pub retained_chars: usize,
    /// 原始内容 SHA-256。
    pub sha256: String,
    /// 可由 archive runtime 读取的完整输出引用。
    pub output_ref: Option<OutputRef>,
}

/// 内容归档运行时。
#[derive(Debug, Clone, Copy)]
pub struct ContentArchiveRuntime {
    preview_chars: usize,
    file_suffix: &'static str,
}

impl ContentArchiveRuntime {
    /// 创建归档运行时；归档文件使用默认 `.log` 后缀。
    #[must_use]
    pub const fn new(preview_chars: usize) -> Self {
        Self::with_suffix(preview_chars, CONTENT_ARCHIVE_DEFAULT_FILE_SUFFIX)
    }

    /// 创建归档运行时并指定归档文件后缀，供结构化内容使用 `.md` 等语义后缀。
    #[must_use]
    pub const fn with_suffix(preview_chars: usize, file_suffix: &'static str) -> Self {
        Self {
            preview_chars,
            file_suffix,
        }
    }

    /// 归档文本输出，返回模型可消费的有界引用。
    pub fn archive_text(
        &self,
        archive_root: &Path,
        stream: &str,
        text: &str,
        stable_id: &str,
    ) -> io::Result<ToolOutputReference> {
        let original_chars = text.chars().count();
        let preview = text.chars().take(self.preview_chars).collect::<String>();
        let retained_chars = preview.chars().count();
        let truncated = original_chars > retained_chars;
        self.archive_text_with_preview(archive_root, stream, text, stable_id, &preview, truncated)
    }

    /// 使用调用方已计算的预览和截断状态归档文本。
    pub fn archive_text_with_preview(
        &self,
        archive_root: &Path,
        stream: &str,
        text: &str,
        stable_id: &str,
        preview: &str,
        truncated: bool,
    ) -> io::Result<ToolOutputReference> {
        let original_chars = text.chars().count();
        let retained_chars = preview.chars().count();
        let output_ref = truncated
            .then(|| {
                self.archive_bytes(
                    archive_root,
                    text.as_bytes(),
                    normalized_tool_source_trace(stable_id),
                    preview.to_owned(),
                )
            })
            .transpose()?;
        Ok(ToolOutputReference {
            stream: stream.to_owned(),
            preview: preview.to_owned(),
            truncated,
            original_chars,
            retained_chars,
            sha256: sha256_hex(text.as_bytes()),
            output_ref,
        })
    }

    /// 将已有的完整工具产物复制到受控归档根，并返回统一 OutputRef。
    pub fn archive_file(
        &self,
        archive_root: &Path,
        source_path: &Path,
        source_trace: &str,
        preview: &str,
    ) -> io::Result<OutputRef> {
        let (sha256, byte_len, archive_ref) =
            persist_file(archive_root, source_path, self.file_suffix)?;
        Ok(OutputRef {
            id: output_ref_id(&sha256),
            byte_len,
            sha256,
            source_trace: source_trace.to_owned(),
            preview: preview.to_owned(),
            archive_ref: Some(archive_ref),
        })
    }

    /// 按相对引用读取完整正文，并校验长度与 hash。
    pub fn read_output(&self, archive_root: &Path, output_ref: &OutputRef) -> io::Result<Vec<u8>> {
        let archive_ref = output_ref
            .archive_ref
            .as_ref()
            .ok_or_else(|| invalid_archive("OutputRef 缺少 archive ref"))?;
        let path = resolve_archive_path(archive_root, archive_ref)?;
        let bytes = fs::read(path)?;
        validate_archive_bytes(&bytes, output_ref.byte_len, &output_ref.sha256)?;
        Ok(bytes)
    }

    fn archive_bytes(
        &self,
        archive_root: &Path,
        bytes: &[u8],
        source_trace: String,
        preview: String,
    ) -> io::Result<OutputRef> {
        let sha256 = sha256_hex(bytes);
        let archive_ref = persist_bytes(archive_root, &sha256, bytes, self.file_suffix)?;
        Ok(OutputRef {
            id: output_ref_id(&sha256),
            byte_len: bytes.len(),
            sha256,
            source_trace,
            preview,
            archive_ref: Some(archive_ref),
        })
    }
}

impl Default for ContentArchiveRuntime {
    fn default() -> Self {
        Self::new(CONTENT_ARCHIVE_DEFAULT_PREVIEW_CHARS)
    }
}

fn persist_bytes(
    archive_root: &Path,
    sha256: &str,
    bytes: &[u8],
    file_suffix: &str,
) -> io::Result<OutputArchiveReference> {
    let directory = archive_root.join(CONTENT_ARCHIVE_DIR_NAME);
    fs::create_dir_all(&directory)?;
    let relative_path =
        PathBuf::from(CONTENT_ARCHIVE_DIR_NAME).join(format!("{sha256}{file_suffix}"));
    let destination = archive_root.join(&relative_path);
    if destination.exists() {
        validate_archive_file(&destination, bytes.len(), sha256)?;
        return Ok(OutputArchiveReference {
            relative_path: relative_path.to_string_lossy().into_owned(),
        });
    }

    let temporary = directory.join(format!(
        ".{sha256}-{}-{CONTENT_ARCHIVE_TEMP_SUFFIX}",
        unique_suffix()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        if !destination.exists() {
            return Err(error);
        }
        validate_archive_file(&destination, bytes.len(), sha256)?;
    }
    Ok(OutputArchiveReference {
        relative_path: relative_path.to_string_lossy().into_owned(),
    })
}

fn persist_file(
    archive_root: &Path,
    source_path: &Path,
    file_suffix: &str,
) -> io::Result<(String, usize, OutputArchiveReference)> {
    let directory = archive_root.join(CONTENT_ARCHIVE_DIR_NAME);
    fs::create_dir_all(&directory)?;
    // 先做**只读**的哈希与长度计算，据此判重后再决定是否拷贝。
    //
    // 顺序很重要：原实现在落位时才判重（`persist_temporary`），而落位之前已经付过
    // 一次整份拷贝与 `sync_all()`：内容寻址去重因此对**性能**完全无效。同一会话被
    // `write_stdin` 轮询 N 次时，每次都白付一次临时文件写入与落盘。
    //
    // 契约前提：源文件**只追加**（tee 归档由 shell reader 线程顺序写入）。在此前提下
    // 「长度未变 ⇒ 内容未变」，拷贝循环可以完全不算哈希，未增长时直接复用 probe 哈希
    // （省去一遍全量 SHA-256，R2 审计 N-F-08 评估采纳）；仅在拷贝窗口内源继续追加的
    // 罕见场景才对临时文件补一次哈希。若将来出现非只追加的调用方，必须放弃该优化。
    let (probe_sha256, probe_len) = hash_file_contents(source_path)?;
    let probe_relative =
        PathBuf::from(CONTENT_ARCHIVE_DIR_NAME).join(format!("{probe_sha256}{file_suffix}"));
    if archive_root.join(&probe_relative).exists() {
        validate_archive_file(
            &archive_root.join(&probe_relative),
            probe_len,
            &probe_sha256,
        )?;
        return Ok((
            probe_sha256,
            probe_len,
            OutputArchiveReference {
                relative_path: probe_relative.to_string_lossy().into_owned(),
            },
        ));
    }

    // 未归档：拷贝到临时文件，只统计字节，不做哈希。
    //
    // 临时文件名由 `create_new` 独占创建成功后**才归本次调用所有**：此前的失败
    //（源打开失败、名字冲突）不得触碰同名文件，否则会在碰撞时误删他人的临时文件
    // （R2 审计 N-A-04）。
    let temporary = directory.join(format!(
        ".stream-{}-{CONTENT_ARCHIVE_TEMP_SUFFIX}",
        unique_suffix()
    ));
    let mut destination = (|| -> io::Result<File> {
        let _source = File::open(source_path)?;
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
    })()?;
    let copied_len = (|| -> io::Result<usize> {
        let mut source = File::open(source_path)?;
        let mut byte_len = 0_usize;
        let mut buffer = [0_u8; CONTENT_ARCHIVE_COPY_BUFFER_BYTES];
        loop {
            let read_len = source.read(&mut buffer)?;
            if read_len == 0 {
                break;
            }
            destination.write_all(&buffer[..read_len])?;
            byte_len = byte_len
                .checked_add(read_len)
                .ok_or_else(|| invalid_archive("工具输出长度超出 usize 范围"))?;
        }
        destination.sync_all()?;
        drop(destination);
        Ok(byte_len)
    })();
    let byte_len = match copied_len {
        Ok(byte_len) => byte_len,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    let (sha256, byte_len) = if byte_len == probe_len {
        // 长度未变：只追加前提下内容必然与 probe 时一致，直接复用 probe 哈希。
        (probe_sha256, byte_len)
    } else {
        // 拷贝窗口内源继续追加：以实际拷贝到的内容为准重新哈希临时文件。
        let (actual_sha256, actual_len) = hash_file_contents(&temporary)?;
        if actual_len != byte_len {
            return Err(invalid_archive("临时归档长度与拷贝字节不一致"));
        }
        (actual_sha256, actual_len)
    };
    let archive_ref = persist_temporary(archive_root, &sha256, byte_len, &temporary, file_suffix)?;
    Ok((sha256, byte_len, archive_ref))
}

/// 只读地计算文件内容的长度与 SHA-256。
fn hash_file_contents(path: &Path) -> io::Result<(String, usize)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256Hex::new();
    let mut byte_len = 0_usize;
    let mut buffer = [0_u8; CONTENT_ARCHIVE_COPY_BUFFER_BYTES];
    loop {
        let read_len = file.read(&mut buffer)?;
        if read_len == 0 {
            break;
        }
        hasher.update(&buffer[..read_len]);
        byte_len = byte_len
            .checked_add(read_len)
            .ok_or_else(|| invalid_archive("工具输出长度超出 usize 范围"))?;
    }
    Ok((hasher.finalize_hex(), byte_len))
}

fn persist_temporary(
    archive_root: &Path,
    sha256: &str,
    byte_len: usize,
    temporary: &Path,
    file_suffix: &str,
) -> io::Result<OutputArchiveReference> {
    let relative_path =
        PathBuf::from(CONTENT_ARCHIVE_DIR_NAME).join(format!("{sha256}{file_suffix}"));
    let destination = archive_root.join(&relative_path);
    if destination.exists() {
        let result = validate_archive_file(&destination, byte_len, sha256);
        let _ = fs::remove_file(temporary);
        result?;
        return Ok(OutputArchiveReference {
            relative_path: relative_path.to_string_lossy().into_owned(),
        });
    }
    if let Err(error) = fs::rename(temporary, &destination) {
        let _ = fs::remove_file(temporary);
        if !destination.exists() {
            return Err(error);
        }
        validate_archive_file(&destination, byte_len, sha256)?;
    }
    Ok(OutputArchiveReference {
        relative_path: relative_path.to_string_lossy().into_owned(),
    })
}

fn validate_archive_file(
    path: &Path,
    expected_len: usize,
    expected_sha256: &str,
) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256Hex::new();
    let mut byte_len = 0_usize;
    let mut buffer = [0_u8; CONTENT_ARCHIVE_COPY_BUFFER_BYTES];
    loop {
        let read_len = file.read(&mut buffer)?;
        if read_len == 0 {
            break;
        }
        hasher.update(&buffer[..read_len]);
        byte_len = byte_len
            .checked_add(read_len)
            .ok_or_else(|| invalid_archive("归档输出长度超出 usize 范围"))?;
    }
    if byte_len != expected_len || hasher.finalize_hex() != expected_sha256 {
        return Err(invalid_archive("归档内容长度或 SHA-256 校验失败"));
    }
    Ok(())
}

fn resolve_archive_path(
    archive_root: &Path,
    archive_ref: &OutputArchiveReference,
) -> io::Result<PathBuf> {
    let relative = Path::new(&archive_ref.relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid_archive("archive ref 必须是安全的相对路径"));
    }
    let root = fs::canonicalize(archive_root)?;
    let path = fs::canonicalize(root.join(relative))?;
    if !path.starts_with(&root) {
        return Err(invalid_archive("archive ref 超出归档根路径"));
    }
    Ok(path)
}

fn validate_archive_bytes(
    bytes: &[u8],
    expected_len: usize,
    expected_sha256: &str,
) -> io::Result<()> {
    if bytes.len() != expected_len || sha256_hex(bytes) != expected_sha256 {
        return Err(invalid_archive("归档内容长度或 SHA-256 校验失败"));
    }
    Ok(())
}

fn output_ref_id(sha256: &str) -> OutputRefId {
    let value = &sha256[..OUTPUT_REF_ID_HEX_LEN.min(sha256.len())];
    OutputRefId(u64::from_str_radix(value, 16).unwrap_or_default())
}

/// 临时文件唯一后缀：纳秒时间戳 ⊕ 进程 ID ⊕ 进程内单调序号。
///
/// 仅用「纳秒 + pid」在 Windows 粗时钟下同一进程内可碰撞；序号保证进程内严格唯一，
/// 消除 `create_new` 冲突路径（R2 审计 N-A-04）。
fn unique_suffix() -> u128 {
    static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = u128::from(TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
        .saturating_add(u128::from(std::process::id()) << 64)
        .saturating_add(sequence)
}

fn invalid_archive(message: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

/// 按工具 namespace/name 生成统一 source trace。
#[must_use]
pub fn tool_output_source_trace(namespace: &str, name: &str) -> String {
    format!("{TOOL_OUTPUT_SOURCE_TRACE_PREFIX}{namespace}/{name}")
}

/// 将 provider dispatch name（namespace.name）转换为统一 source trace。
#[must_use]
pub fn tool_output_source_trace_from_dispatch_name(dispatch_name: &str) -> String {
    dispatch_name.rsplit_once('.').map_or_else(
        || format!("{TOOL_OUTPUT_SOURCE_TRACE_PREFIX}{dispatch_name}"),
        |(namespace, name)| tool_output_source_trace(namespace, name),
    )
}

fn normalized_tool_source_trace(stable_id: &str) -> String {
    stable_id
        .strip_prefix(TOOL_OUTPUT_SOURCE_TRACE_PREFIX)
        .map_or_else(
            || format!("{TOOL_OUTPUT_SOURCE_TRACE_PREFIX}{stable_id}"),
            |value| format!("{TOOL_OUTPUT_SOURCE_TRACE_PREFIX}{value}"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(tag: &str) -> PathBuf {
        axon_test_support::test_temp_root()
            .join(format!("axon-content-archive-{tag}-{}", std::process::id()))
    }

    #[test]
    fn archives_full_output_and_reads_it_by_ref() {
        let root = test_root("roundtrip");
        let runtime = ContentArchiveRuntime::new(4);

        let reference = runtime
            .archive_text(&root, "stdout", "abcdef", "verify-1")
            .expect("输出归档应成功");

        assert!(reference.truncated);
        assert_eq!(reference.preview, "abcd");
        let output_ref = reference.output_ref.expect("截断输出应有归档引用");
        assert_eq!(runtime.read_output(&root, &output_ref).unwrap(), b"abcdef");
        let _ = fs::remove_dir_all(root);
    }

    /// P3 回归：对**相同内容**重复归档必须复用同一对象，且不残留临时文件。
    ///
    /// 判重顺序调整后，「内容已归档」会在创建临时文件**之前**直接返回：
    /// 本用例守住该新分支不泄漏临时文件，并确认同一内容始终映射到同一归档对象。
    #[test]
    fn rearchiving_identical_content_reuses_the_object_without_temp_residue() {
        let root = test_root("dedup-reuse");
        fs::create_dir_all(&root).unwrap();
        let source_path = root.join("session.shell.log");
        fs::write(&source_path, b"repeated archive payload").unwrap();
        let runtime = ContentArchiveRuntime::default();

        let first = runtime
            .archive_file(
                &root,
                &source_path,
                "tool://builtin/shell_command",
                "preview",
            )
            .expect("首次归档应成功");
        let second = runtime
            .archive_file(
                &root,
                &source_path,
                "tool://builtin/shell_command",
                "preview",
            )
            .expect("重复归档应成功");

        assert_eq!(first.sha256, second.sha256, "相同内容必然产生相同内容哈希");
        assert_eq!(
            first.archive_ref, second.archive_ref,
            "相同内容必须复用同一归档对象"
        );
        let directory = root.join(CONTENT_ARCHIVE_DIR_NAME);
        let entries = fs::read_dir(&directory).unwrap().count();
        assert_eq!(
            entries, 1,
            "判重命中路径不得残留临时文件，归档目录实际有 {entries} 个条目"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// R2 审计 N-F-08 回归：源在两次归档之间**追加**后，新归档必须产生与实际内容
    /// 一致的新对象：即「长度未变复用 probe 哈希、长度增长重哈希临时文件」的
    /// 条件哈希分支在增长场景下不得产出「文件名与内容不符」的归档。
    #[test]
    fn appending_to_source_produces_a_new_archive_matching_actual_content() {
        let root = test_root("append-growth");
        fs::create_dir_all(&root).unwrap();
        let source_path = root.join("session.shell.log");
        fs::write(&source_path, b"first chunk").unwrap();
        let runtime = ContentArchiveRuntime::default();

        let first = runtime
            .archive_file(
                &root,
                &source_path,
                "tool://builtin/shell_command",
                "preview",
            )
            .expect("首次归档应成功");

        // 模拟 shell reader 线程继续向 tee 归档追加。
        let mut appended = fs::read(&source_path).unwrap();
        appended.extend_from_slice(b" + appended tail");
        fs::write(&source_path, &appended).unwrap();

        let second = runtime
            .archive_file(
                &root,
                &source_path,
                "tool://builtin/shell_command",
                "preview",
            )
            .expect("追加后归档应成功");

        assert_ne!(first.sha256, second.sha256, "内容增长必须产生新内容哈希");
        assert_ne!(
            first.archive_ref, second.archive_ref,
            "内容增长必须落位为新的归档对象"
        );
        assert_eq!(
            ContentArchiveRuntime::default()
                .read_output(&root, &second)
                .unwrap(),
            appended,
            "新归档对象必须与实际追加后的内容逐字节一致"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// R2 审计 N-A-04 回归：临时文件序号在进程内严格单调，连续取用不得碰撞。
    #[test]
    fn unique_suffix_is_strictly_monotonic_within_the_process() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(unique_suffix()), "临时文件后缀发生进程内碰撞");
        }
    }

    #[test]
    fn archive_file_streams_output_larger_than_copy_buffer() {
        let root = test_root("streaming-file");
        fs::create_dir_all(&root).unwrap();
        let source_path = root.join("shell-full-output.log");
        let expected = vec![b'x'; CONTENT_ARCHIVE_COPY_BUFFER_BYTES + 1];
        fs::write(&source_path, &expected).unwrap();

        let output_ref = ContentArchiveRuntime::default()
            .archive_file(
                &root,
                &source_path,
                "tool://builtin/shell_command",
                "preview",
            )
            .expect("流式归档应成功");

        assert_eq!(output_ref.byte_len, expected.len());
        assert_eq!(
            ContentArchiveRuntime::default()
                .read_output(&root, &output_ref)
                .unwrap(),
            expected
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_tampered_archive_and_path_escape() {
        let root = test_root("tamper");
        let runtime = ContentArchiveRuntime::new(4);
        let reference = runtime
            .archive_text(&root, "stdout", "abcdef", "verify-2")
            .unwrap()
            .output_ref
            .unwrap();
        let path = root.join(&reference.archive_ref.as_ref().unwrap().relative_path);
        fs::write(path, "tampered").unwrap();
        assert!(runtime.read_output(&root, &reference).is_err());

        let escaped = OutputRef {
            archive_ref: Some(OutputArchiveReference {
                relative_path: "../outside.log".to_owned(),
            }),
            ..reference
        };
        assert!(runtime.read_output(&root, &escaped).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
