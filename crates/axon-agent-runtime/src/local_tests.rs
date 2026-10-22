use std::{fs, path::PathBuf};

use axon_domain::AttachmentSensitivity;

use super::{AttachmentDragDropEntryKind, AttachmentLocalUploadRequest, AttachmentPathKind};
use crate::{AttachmentBlobStore, AttachmentIngressRuntime};

#[test]
fn local_upload_sniffs_mime_and_respects_blob_policy() {
    let root = unique_test_blob_root("local-upload-sniffs-mime");
    let mut store = AttachmentBlobStore::new(&root);
    let mut runtime = AttachmentIngressRuntime::default();
    let record = runtime
        .ingest_local_upload_with_blob_store(
            AttachmentLocalUploadRequest {
                file_name: "note.bin".to_owned(),
                mime_hint: Some("application/octet-stream".to_owned()),
                bytes: "hello".as_bytes().to_vec(),
                source_trace: "local://pane/1/drop/note.bin".to_owned(),
                sensitivity: AttachmentSensitivity::Normal,
            },
            &mut store,
            2_000,
        )
        .expect("UTF-8 文本应 sniff 为 text/plain 并写入 BlobStore");

    assert_eq!(record.attachment.mime_type, "text/plain");
    assert_eq!(record.attachment.preview.as_deref(), Some("hello"));
    assert!(record.blob_path.exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn drag_drop_directory_only_creates_manifest() {
    let manifest = AttachmentIngressRuntime::build_drag_drop_manifest(vec![
        ("E:\\work\\readme.md".to_owned(), AttachmentPathKind::File),
        ("E:\\work\\src".to_owned(), AttachmentPathKind::Directory),
    ])
    .expect("非空 drag/drop 应生成 manifest");

    assert!(!manifest.recursive_read_allowed);
    assert_eq!(manifest.entries.len(), 2);
    assert_eq!(
        manifest.entries[1].kind,
        AttachmentDragDropEntryKind::DirectoryManifest
    );
}

#[test]
fn unknown_binary_upload_is_rejected_by_content_validation() {
    let root = unique_test_blob_root("unknown-binary-upload");
    let mut store = AttachmentBlobStore::new(&root);
    let mut runtime = AttachmentIngressRuntime::default();
    let result = runtime.ingest_local_upload_with_blob_store(
        AttachmentLocalUploadRequest {
            file_name: "payload.bin".to_owned(),
            mime_hint: None,
            bytes: vec![0, 159, 146, 150],
            source_trace: "local://pane/1/drop/payload.bin".to_owned(),
            sensitivity: AttachmentSensitivity::Normal,
        },
        &mut store,
        2_000,
    );

    assert!(result.is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn clipboard_png_preserves_format_dimensions_trace_and_blob_record() {
    let root = unique_test_blob_root("clipboard-png");
    let mut store = AttachmentBlobStore::new(&root);
    let mut runtime = AttachmentIngressRuntime::default();
    let record = runtime
        .ingest_clipboard_image_with_blob_store(
            super::AttachmentClipboardImageRequest {
                content_id: "clipboard-image-1.png".to_owned(),
                mime_hint: None,
                bytes: png_1x1_bytes(),
                source_trace: "clipboard://window/1/image/1".to_owned(),
                sensitivity: AttachmentSensitivity::Normal,
            },
            &mut store,
            3_000,
        )
        .expect("剪贴板 PNG 图片应进入附件入口");

    assert_eq!(record.image_format, "image/png");
    assert_eq!((record.width, record.height), (1, 1));
    assert_eq!(record.content_id, "clipboard-image-1.png");
    assert_eq!(
        record.blob.attachment.source,
        axon_domain::AttachmentSource::ClipboardImage
    );
    assert_eq!(
        record.blob.attachment.kind,
        axon_domain::AttachmentKind::Image
    );
    assert_eq!(
        record.blob.attachment.source_trace,
        "clipboard://window/1/image/1"
    );
    assert!(record.blob.blob_path.exists());
    assert!(record.blob.attachment.preview.is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn screenshot_binds_window_session_pane_trace_and_blob_record() {
    let root = unique_test_blob_root("screenshot-ingress");
    let mut store = AttachmentBlobStore::new(&root);
    let mut runtime = AttachmentIngressRuntime::default();
    let record = runtime
        .ingest_screenshot_with_blob_store(
            super::AttachmentScreenshotRequest {
                window_id: 7,
                session_id: "session-1".to_owned(),
                pane_id: axon_domain::PaneId(3),
                mime_hint: None,
                bytes: png_1x1_bytes(),
                sensitivity: AttachmentSensitivity::Normal,
            },
            &mut store,
            4_000,
        )
        .expect("截图应进入附件入口");

    assert_eq!(record.image_format, "image/png");
    assert_eq!((record.width, record.height), (1, 1));
    assert_eq!(record.window_id, 7);
    assert_eq!(record.session_id, "session-1");
    assert_eq!(record.pane_id, axon_domain::PaneId(3));
    assert_eq!(
        record.blob.attachment.source,
        axon_domain::AttachmentSource::Screenshot
    );
    assert_eq!(
        record.blob.attachment.source_trace,
        "screenshot://window/7/session/session-1/pane/3"
    );
    assert!(record.blob.blob_path.exists());
    assert!(record.blob.attachment.preview.is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_upload_enters_ready_and_remains_ref_only() {
    let root = unique_test_blob_root("remote-upload");
    let mut store = AttachmentBlobStore::new(&root);
    let mut runtime = AttachmentIngressRuntime::default();
    let record = runtime
        .ingest_remote_upload_with_blob_store(
            super::AttachmentRemoteUploadRequest {
                client_id: "feishu-agent".to_owned(),
                upload_id: "upload-1".to_owned(),
                file_name: "remote-note.txt".to_owned(),
                mime_hint: None,
                bytes: "远控上传文本".as_bytes().to_vec(),
                sensitivity: AttachmentSensitivity::Normal,
            },
            &mut store,
            5_000,
        )
        .expect("远控上传应进入附件入口");

    assert_eq!(
        record.attachment.source,
        axon_domain::AttachmentSource::RemoteUpload
    );
    assert_eq!(
        record.attachment.review_state,
        axon_domain::AttachmentReviewState::Ready
    );
    assert_eq!(
        record.attachment.prompt_policy,
        axon_domain::AttachmentPromptPolicy::RefOnly
    );
    assert_eq!(
        record.attachment.source_trace,
        "remote://client/feishu-agent/upload/upload-1"
    );
    assert_eq!(record.attachment.mime_type, "text/plain");
    assert!(record.blob_path.exists());

    let _ = fs::remove_dir_all(root);
}

fn unique_test_blob_root(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!("axon-{name}-{}", std::process::id()))
}

fn png_1x1_bytes() -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0,
        0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
    ]
}
