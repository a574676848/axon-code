//! [`crate::conversation::attachment`] 的用例。
//!
//! 单独成篇只为守住单文件 700 行硬限。断言的都是纯函数与本机分支：分流判定的单测在
//! `workspace_files_tests`，这里不依赖开发机上此刻生效的是哪个档案。

use super::*;

#[test]
fn file_names_are_reduced_to_a_single_safe_segment() {
    assert_eq!(safe_name("../../etc/passwd"), "passwd");
    assert_eq!(safe_name("C:\\tmp\\shot.png"), "shot.png");
    assert_eq!(safe_name("   "), "attachment");
    assert!(!safe_name("a:b*c?.png").contains([':', '*', '?']));
}

#[test]
fn display_names_keep_the_user_wording_and_drop_directories() {
    // 落盘名会把全角括号等替换成下划线，但卡片上必须还是用户那个名字。
    assert_eq!(
        display_name("实时语音识别 V2（WebSocket）.md"),
        "实时语音识别 V2（WebSocket）.md"
    );
    assert_eq!(display_name("C:\\tmp\\截图（1）.png"), "截图（1）.png");
    assert_eq!(display_name("   "), "attachment");
}

#[test]
fn staging_rejects_invalid_payloads_before_touching_disk() {
    // 两条分支共用同一个解码入口：非法负载在落盘与上传之前就被拒。
    assert!(stage_locally("a.png", "").is_err());
    assert!(stage_locally("a.png", "不是 base64").is_err());
    assert!(decode_payload("").is_err());
    assert!(decode_payload("不是 base64").is_err());
}

/// 造一个只带有效 PNG 头的字节串：宽高按 IHDR 写入。
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes
}

#[test]
fn image_dimensions_are_read_from_the_header() {
    assert_eq!(
        image_dimensions(&png_bytes(1_200, 800), "image/png"),
        Some((1_200, 800))
    );
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&40_u16.to_le_bytes());
    gif.extend_from_slice(&30_u16.to_le_bytes());
    assert_eq!(image_dimensions(&gif, "image/gif"), Some((40, 30)));
    // JPEG：SOI + SOF0 段，高在前宽在后。
    let jpeg = [
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x02, 0x58,
        0x03, 0x20,
    ];
    assert_eq!(image_dimensions(&jpeg, "image/jpeg"), Some((800, 600)));
}

#[test]
fn tiny_images_are_rejected_before_reaching_the_provider() {
    // 真机回执：8×8 的图被 Provider 拒（Image too small (area=64px)），这里提前拦住。
    let error = ensure_usable_image("tiny.png", &png_bytes(8, 8), "image/png")
        .expect_err("过小的图片必须被拒");
    assert!(error.contains("图片太小"), "{error}");
    assert!(error.contains("8×8"), "{error}");
    assert!(ensure_usable_image("ok.png", &png_bytes(16, 16), "image/png").is_ok());
    // 尺寸读不出来时不拦：宁可交给 Provider 判，也不误杀正常图片。
    assert!(
        ensure_usable_image(
            "vector.svg",
            b"<svg viewBox=\"0 0 4 4\"></svg>",
            "image/svg+xml"
        )
        .is_ok()
    );
}

#[test]
fn preview_serves_any_image_path_and_still_rejects_non_images() {
    assert!(resolve_preview("notes.txt").is_err());
    assert!(resolve_preview("missing.png").is_err());
}

#[test]
fn preview_reads_an_image_outside_the_runtime_root() {
    // 判据只有「图片后缀 + 体积」两条：工作区里的图片与控制台临时目录里的图片
    // 走同一条路径，不再按目录设卡。
    let root = axon_test_support::test_temp_root().join("preview-scope");
    std::fs::create_dir_all(&root).expect("建临时目录");
    let path = root.join("shot.png");
    let bytes = png_bytes(8, 8);
    std::fs::write(&path, &bytes).expect("写临时图片");
    let (resolved, mime) =
        resolve_preview(&path.to_string_lossy()).expect("运行态目录之外的图片必须可预览");
    assert_eq!(mime, "image/png");
    assert_eq!(std::fs::read(&resolved).expect("读回"), bytes);
}

#[test]
fn percent_decoding_round_trips_windows_paths() {
    let encoded = "C%3A%5CUsers%5Cexample%5C.axon%5Cdata%5Ct.png";
    assert_eq!(
        percent_decode(encoded).expect("解码"),
        r"C:\Users\example\.axon\data\t.png"
    );
    assert!(percent_decode("%zz").is_err());
    assert!(percent_decode("%4").is_err());
    assert_eq!(
        path_from_scheme_uri("axon-attachment://local/C%3A%5Ca%5Cb.png").expect("解析"),
        r"C:\a\b.png"
    );
}
