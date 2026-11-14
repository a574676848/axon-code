//! 作业输出尾随读取的边界测试。
//!
//! 只测纯逻辑：归档头部定位、分片标记剥离、UTF-8 边界回退与分片大小收口。它们都不
//! 依赖运行态，因此三平台跑出来的结论一致。读取编排（归属校验、游标推进）由
//! `lane_job_output_target` 的解析测试与 `stop_lane_job` 共用的归属判据覆盖。

use super::*;

/// 多行 `command=` 会把头部撑成不止三行：判据必须是 `tty=` 那一行，而不是行数。
#[test]
fn header_bytes_stops_after_the_tty_line() {
    let path = axon_test_support::test_temp_root().join("job-header-multiline.log");
    // 中间那行 `missing=noop` 是故意的：按行数数会把它当成正文起点。
    let header = "command=cargo test \\\n  --workspace\nmissing=noop\ntty=false\n";
    std::fs::write(&path, format!("{header}----- stdout seq=0 -----\nbody\n"))
        .expect("归档应可写入");
    assert_eq!(
        job_artifact_header_bytes(&path),
        u64::try_from(header.len()).expect("头部长度应可转换")
    );
}

/// 认不出 `tty=` 时不猜：正文从文件起点算，宁可多一行也不吞掉命令输出。
#[test]
fn header_bytes_is_zero_without_a_tty_line() {
    let path = axon_test_support::test_temp_root().join("job-header-absent.log");
    std::fs::write(&path, "plain output\nsecond line\n").expect("归档应可写入");
    assert_eq!(job_artifact_header_bytes(&path), 0);
}

/// 分片标记不是命令输出；每片前后各留的空行也只保留一行。
#[test]
fn markers_are_stripped_and_blank_runs_collapsed() {
    let raw = "----- stdout seq=0 -----\nchecking\n\n\n----- stderr seq=1 -----\nwarning\n";
    assert_eq!(strip_archive_markers(raw), "checking\n\nwarning\n");
}

/// 帧边界切在半个字符上时，只保留它之前的完整部分。
#[test]
fn utf8_prefix_stops_before_a_split_character() {
    let mut bytes = "输出".as_bytes().to_vec();
    bytes.truncate(4);
    assert_eq!(utf8_prefix_len(&bytes), 3);
}

/// 整段可解码时一个字节都不丢。
#[test]
fn utf8_prefix_keeps_a_decodable_buffer() {
    let bytes = "输出".as_bytes();
    assert_eq!(utf8_prefix_len(bytes), bytes.len());
}

/// 分片大小有下界也有上界：0 与超大值都被夹进可服务区间。
#[test]
fn page_size_is_clamped() {
    assert_eq!(bounded_job_output_max(0), 1);
    assert_eq!(
        bounded_job_output_max(JOB_OUTPUT_DEFAULT_MAX_BYTES),
        JOB_OUTPUT_DEFAULT_MAX_BYTES
    );
    assert_eq!(bounded_job_output_max(usize::MAX), 256 * 1024);
}
