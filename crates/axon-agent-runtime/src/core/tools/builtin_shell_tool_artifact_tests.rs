//! Shell 归档容量上限与截断语义的回归测试。
//!
//! 归档上限的默认值为 64 MiB，直接跑端到端命令无法在合理时间内触发，
//! 因此这里以极小上限直接驱动 `SharedProcessOutput`，覆盖判定、一次性截断与原因上报。

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{ShellCommandOutputStream, output_buffer::SharedProcessOutput, session};

/// 测试用归档上限：远小于默认值，足以在数次追加内触发截断。
const TEST_ARTIFACT_LIMIT_BYTES: usize = 1024;
/// 每次追加的负载大小；与上限配合可稳定触发截断。
const TEST_CHUNK_BYTES: usize = 256;
/// 触发截断所需的追加次数（含判定失败的那一次）。
const TEST_OVERFLOW_APPENDS: usize = 8;
/// 测试负载填充字节。
const TEST_CHUNK_FILL: u8 = b'x';
/// 归档落盘失败原因。
const REASON_ARTIFACT_WRITE_FAILED: &str = "shell_command:artifact_write_failed";
/// 归档截断原因。
const REASON_ARTIFACT_TRUNCATED: &str = "shell_command:artifact_truncated";

#[test]
fn artifact_within_limit_stays_complete() {
    let (artifact, path) = temp_artifact("complete");
    let output = SharedProcessOutput::new(artifact, TEST_ARTIFACT_LIMIT_BYTES);

    output.append(ShellCommandOutputStream::Stdout, &chunk());

    assert!(!output.artifact_truncated());
    assert!(!output.artifact_failed());
    let written = fs::read(&path).unwrap();
    assert!(written.starts_with(b"\n----- stdout seq=0 -----"));
    assert!(written.ends_with(&chunk()));
    cleanup(&path);
}

#[test]
fn artifact_preexisting_header_counts_toward_limit() {
    let (mut artifact, path) = temp_artifact("header");
    let header = b"command=echo axon\ncwd=/\ntty=false\n";
    artifact.write_all(header).unwrap();
    artifact.flush().unwrap();
    let output = SharedProcessOutput::new(artifact, header.len() + TEST_CHUNK_BYTES);

    output.append(ShellCommandOutputStream::Stdout, &chunk());

    assert!(
        output.artifact_truncated(),
        "归档已有头部时剩余容量不足以容纳该帧，必须判定为截断"
    );
    assert_eq!(fs::read(&path).unwrap().len(), header.len());
    cleanup(&path);
}

#[test]
fn artifact_overflow_stops_writing_and_never_exceeds_limit() {
    let (artifact, path) = temp_artifact("overflow");
    let output = SharedProcessOutput::new(artifact, TEST_ARTIFACT_LIMIT_BYTES);

    for _ in 0..TEST_OVERFLOW_APPENDS {
        output.append(ShellCommandOutputStream::Stdout, &chunk());
    }

    assert!(output.artifact_truncated());
    let written = fs::read(&path).unwrap();
    assert!(
        written.len() <= TEST_ARTIFACT_LIMIT_BYTES,
        "归档大小 {} 越过上限 {TEST_ARTIFACT_LIMIT_BYTES}",
        written.len()
    );
    assert!(!written.is_empty(), "达到上限前必须已写入可诊断的归档内容");
    cleanup(&path);
}

#[test]
fn artifact_truncation_is_permanent() {
    let (artifact, path) = temp_artifact("permanent");
    let output = SharedProcessOutput::new(artifact, TEST_ARTIFACT_LIMIT_BYTES);
    for _ in 0..TEST_OVERFLOW_APPENDS {
        output.append(ShellCommandOutputStream::Stdout, &chunk());
    }
    let truncated_size = fs::read(&path).unwrap().len();

    output.append(ShellCommandOutputStream::Stdout, b"tail");

    assert_eq!(
        fs::read(&path).unwrap().len(),
        truncated_size,
        "截断后即使追加极小负载也不得恢复写入，否则归档内容会与 seq 标记错位"
    );
    cleanup(&path);
}

#[test]
fn truncated_artifact_still_serves_bounded_preview() {
    let (artifact, path) = temp_artifact("preview");
    let output = SharedProcessOutput::new(artifact, TEST_ARTIFACT_LIMIT_BYTES);
    for _ in 0..TEST_OVERFLOW_APPENDS {
        output.append(ShellCommandOutputStream::Stdout, &chunk());
    }

    let receipt = output.take_receipt(None);

    assert!(
        receipt.stdout.preview.contains("xxxx"),
        "归档截断只应影响落盘，模型可见预览必须仍然可用"
    );
    cleanup(&path);
}

#[test]
fn artifact_truncation_is_reported_in_receipt_reasons() {
    let reasons = session::shell_receipt_reasons(false, false, true);

    assert!(
        reasons
            .iter()
            .any(|reason| reason == REASON_ARTIFACT_TRUNCATED),
        "归档截断必须出现在回执原因中，实际为 {reasons:?}"
    );
}

#[test]
fn intact_artifact_omits_truncation_reason() {
    let reasons = session::shell_receipt_reasons(false, false, false);

    assert!(
        !reasons
            .iter()
            .any(|reason| reason == REASON_ARTIFACT_TRUNCATED),
        "归档完整时不得上报截断，实际为 {reasons:?}"
    );
}

#[test]
fn failed_and_truncated_artifacts_are_both_reported() {
    let reasons = session::shell_receipt_reasons(true, true, true);

    for expected in [
        "shell_command:persistent_session",
        "shell_command:host_process_used",
        "shell_command:pty_used",
        REASON_ARTIFACT_WRITE_FAILED,
        REASON_ARTIFACT_TRUNCATED,
    ] {
        assert!(
            reasons.iter().any(|reason| reason == expected),
            "缺少原因 {expected}，实际为 {reasons:?}"
        );
    }
}

/// R-26 回归（一）：并发追加不得丢失归档字节计数。
///
/// stdout / stderr 各有一个 reader 线程调用 `append`。旧实现**锁外**读计数快照、
/// 锁内用陈旧值回写，两个线程会互相覆盖增量（lost update）。
///
/// 判据取「计数 ≥ 归档实际大小」：帧开销常量 `ARTIFACT_FRAME_MAX_OVERHEAD_BYTES`
/// 是实际帧长的**保守上界**，因此正确实现下该不等式恒成立；一旦计数低于实际大小，
/// 即证明有增量被并发覆盖。
#[test]
fn concurrent_appends_never_lose_the_byte_counter() {
    /// 单流追加次数；两流合计 1000 次足以稳定暴露丢失。
    const APPENDS_PER_STREAM: usize = 500;
    /// 上限放宽到不会触发截断，以便覆盖尽可能多的并发写。
    const NO_TRUNCATION_LIMIT: usize = 64 * 1024 * 1024;

    let (artifact, path) = temp_artifact("concurrent-counter");
    let output = Arc::new(SharedProcessOutput::new(artifact, NO_TRUNCATION_LIMIT));
    let payload = chunk();

    let mut handles = Vec::new();
    for stream in [
        ShellCommandOutputStream::Stdout,
        ShellCommandOutputStream::Stderr,
    ] {
        let output = Arc::clone(&output);
        let payload = payload.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..APPENDS_PER_STREAM {
                output.append(stream, &payload);
            }
        }));
    }
    for handle in handles {
        handle.join().expect("并发追加线程不得 panic");
    }

    assert!(!output.artifact_failed(), "测试环境不应出现归档写入失败");
    let written = fs::read(&path).unwrap().len();
    let counted = output.archived_bytes();
    assert!(
        counted >= written,
        "计数必须不低于归档实际大小（帧开销为保守上界）：计数 {counted}，实际 {written}；\
         低于实际即说明并发下丢失了增量"
    );
    cleanup(&path);
}

/// R-26 回归（二）：并发追加下归档上限仍是**硬上限**。
#[test]
fn concurrent_appends_stay_within_the_artifact_limit() {
    const APPENDS_PER_STREAM: usize = 200;
    /// 并发场景下的极小上限：截断会频繁触发，压缩竞态窗口。
    const CONCURRENT_LIMIT: usize = 8 * 1024;

    let (artifact, path) = temp_artifact("concurrent-limit");
    let output = Arc::new(SharedProcessOutput::new(artifact, CONCURRENT_LIMIT));
    let payload = chunk();

    let mut handles = Vec::new();
    for stream in [
        ShellCommandOutputStream::Stdout,
        ShellCommandOutputStream::Stderr,
    ] {
        let output = Arc::clone(&output);
        let payload = payload.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..APPENDS_PER_STREAM {
                output.append(stream, &payload);
            }
        }));
    }
    for handle in handles {
        handle.join().expect("并发追加线程不得 panic");
    }

    let written = fs::read(&path).unwrap().len();
    assert!(
        written <= CONCURRENT_LIMIT,
        "上限是硬上限：实际 {written} 字节，不得超过 {CONCURRENT_LIMIT}"
    );
    cleanup(&path);
}

fn chunk() -> Vec<u8> {
    vec![TEST_CHUNK_FILL; TEST_CHUNK_BYTES]
}

/// 创建一次性测试归档文件；调用方负责以 [`cleanup`] 删除。
fn temp_artifact(tag: &str) -> (File, PathBuf) {
    let path = axon_test_support::test_temp_root().join(format!(
        "axon-shell-artifact-{tag}-{}-{}.shell.log",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let file = File::create(&path).unwrap();
    (file, path)
}

fn cleanup(path: &Path) {
    let _ = fs::remove_file(path);
}
