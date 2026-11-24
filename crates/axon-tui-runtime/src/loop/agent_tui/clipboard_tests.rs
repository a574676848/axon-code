//! 剪贴板隔离后端与原生访问互斥边界测试。

use super::*;

#[cfg(windows)]
const READER_COUNT: usize = 8;
#[cfg(windows)]
const READS_PER_READER: usize = 100;

#[test]
fn copy_rejects_empty_text_before_touching_platform_clipboard() {
    let error = copy_to_clipboard("  \n ").expect_err("空内容不应触发平台剪贴板");
    assert!(error.contains("没有可复制的内容"));
}

#[test]
fn memory_clipboard_roundtrips_text_without_sharing_state() {
    let mut first = ClipboardBackend::Memory {
        text: None,
        image: None,
    };
    let second = first.clone();
    assert!(first.read_text().is_err());
    assert!(first.copy_text(" \n ").is_err());

    first.copy_text(" 第一行\n第二行 ").expect("文本应写入内存");
    assert_eq!(first.read_text().expect("文本应可读取"), "第一行\n第二行");
    assert!(second.read_text().is_err());
}

#[test]
fn test_state_isolates_clipboard_and_cleans_runtime_directory_after_last_clone() {
    use crate::r#loop::agent_tui::{state::AgentTuiState, state_tests::launch_request};

    let request = launch_request(None);
    let mut state = AgentTuiState::new_for_test(&request);
    let runtime_dir = state.bridge.runtime_tmp_dir.clone();
    assert_ne!(runtime_dir, request.runtime_tmp_dir);
    assert!(state.composer.history.is_empty());
    state.replace_draft("隔离历史".to_owned());
    assert!(state.take_submit_from_draft().is_some());
    assert!(state.composer.history_path.starts_with(&runtime_dir));
    assert!(
        axon_jsonl::ShardedJsonlStore::physical_shard_path(&state.composer.history_path, 0)
            .is_file()
    );

    let clone = state.clone();
    drop(state);
    assert!(runtime_dir.is_dir());
    drop(clone);
    assert!(!runtime_dir.exists());
}

#[test]
fn memory_clipboard_persists_image_only_in_temporary_directory() {
    const PIXEL: [u8; 4] = [255, 0, 0, 255];
    let clipboard = ClipboardBackend::Memory {
        text: None,
        image: Some(arboard::ImageData {
            width: 1,
            height: 1,
            bytes: std::borrow::Cow::Owned(PIXEL.to_vec()),
        }),
    };
    let image_dir = axon_test_support::test_temp_dir();
    let path = clipboard
        .read_image(image_dir.path())
        .expect("图片应保存成功");
    assert!(path.starts_with(image_dir.path()));
    let image = image::open(path).expect("PNG 应可读取").to_rgba8();
    assert_eq!(image.dimensions(), (1, 1));
    assert_eq!(image.as_raw(), &PIXEL);
}

#[cfg(windows)]
#[test]
fn clipboard_access_serializes_concurrent_operations_without_system_io() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let barrier = std::sync::Barrier::new(READER_COUNT);
    let active = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..READER_COUNT {
            let barrier = &barrier;
            let active = &active;
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..READS_PER_READER {
                    with_clipboard_access(|| {
                        assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                        std::thread::yield_now();
                        assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                    });
                }
            });
        }
    });
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[cfg(windows)]
#[test]
#[ignore = "访问真实系统剪贴板，仅用于显式执行的 Windows 原生烟测"]
fn concurrent_clipboard_text_reads_do_not_corrupt_heap() {
    let barrier = std::sync::Barrier::new(READER_COUNT);
    std::thread::scope(|scope| {
        for _ in 0..READER_COUNT {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for _ in 0..READS_PER_READER {
                    let _ = read_clipboard_text();
                }
            });
        }
    });
}
