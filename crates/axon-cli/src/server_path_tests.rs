//! 服务端路径归一化的契约测试。
//!
//! 分两组：纯函数组只断言「输入 + 方言 -> 结论」，不碰文件系统，因此 POSIX 语义
//! 也能在 Windows 上逐条验证；文件系统组用真实临时目录验证目录联接、符号链接与
//! 目标不存在时的解析口径，越界必须被拒绝而不是放行。

#[cfg(windows)]
use std::path::Path;

use super::{
    PathDialect, Rejection, is_absolute_input, is_attachments_path, is_within, normalize_absolute,
    normalize_relative, resolve_absolute, resolve_within, strip_verbatim_prefix,
};

/// 断言拒绝结论里带中文原因，避免把英文报错或空串当成交付文案。
fn assert_chinese(message: &str) {
    assert!(
        message
            .chars()
            .any(|value| ('\u{4e00}'..='\u{9fff}').contains(&value)),
        "拒绝结论必须带中文原因：{message}"
    );
}

/// 断言某条输入被归一化拒绝，且拒绝结论带中文原因。
fn reject_absolute(input: &str, dialect: PathDialect, expected: Rejection) {
    let rejection = normalize_absolute(input, dialect).expect_err("该形态必须被拒绝");
    assert_eq!(rejection, expected, "{input}");
    assert_chinese(rejection.message());
}

/// 合同 §9 阶段 3 的 Windows 越界组，逐条断言被拒绝。
#[test]
fn windows_escape_shapes_are_rejected() {
    let cases = [
        (r"\\?\C:\Windows\win.ini", Rejection::VerbatimPrefix),
        (r"\\.\PhysicalDrive0", Rejection::DeviceNamespace),
        (r"C:..\..\Windows", Rejection::DriveRelative),
        (r"\\server\share", Rejection::Unc),
        (r"C:\work\repo\file.txt::$DATA", Rejection::DataStream),
        (r"C:\work\repo\PROGRA~1\..", Rejection::ShortNameFallback),
    ];
    for (input, expected) in cases {
        reject_absolute(input, PathDialect::Windows, expected);
    }
    // 形态合法但落在根之外：归一化本身不拒绝，由包含性判定拒绝。
    let outside = normalize_absolute(r"C:\Windows\win.ini", PathDialect::Windows)
        .expect("普通绝对路径必须可归一化");
    let root = normalize_absolute(r"C:\work\repo", PathDialect::Windows).expect("根必须可归一化");
    assert!(!is_within(&outside, &root, PathDialect::Windows));
}

/// 尾随点与空格会被 Windows 静默剥掉，因此必须拒绝而不是按剥掉后的名字放行。
#[test]
fn windows_trailing_dot_and_space_variants_are_rejected() {
    for input in [
        r"C:\work\repo\secret.",
        r"C:\work\repo\secret ",
        r"C:\work\repo\secret. ",
        r"C:\work\repo\. ",
        r"C:\work\repo\Windows..",
    ] {
        reject_absolute(input, PathDialect::Windows, Rejection::TrailingDotOrSpace);
    }
}

/// 相对路径里的越界与非法形态同样拒绝，理由与绝对路径一致。
#[test]
fn windows_relative_escape_shapes_are_rejected() {
    let base = normalize_absolute(r"D:\work\repo", PathDialect::Windows).expect("根必须可归一化");
    let cases = [
        (r"..\..\Windows", Rejection::ParentEscape),
        (r"sub\..\..\x", Rejection::ParentEscape),
        (r"..", Rejection::ParentEscape),
        (r"C:..\..\Windows", Rejection::DriveRelative),
        (r"\Windows", Rejection::DriveRelative),
        (r"\\server\share", Rejection::Unc),
        (r"\\?\C:\Windows", Rejection::VerbatimPrefix),
        (r"\\.\PhysicalDrive0", Rejection::DeviceNamespace),
        (r"file.txt::$DATA", Rejection::DataStream),
        (r"PROGRA~1\..", Rejection::ShortNameFallback),
        (r"evil.", Rejection::TrailingDotOrSpace),
        (r"evil ", Rejection::TrailingDotOrSpace),
    ];
    for (input, expected) in cases {
        let rejection = normalize_relative(&base, input, PathDialect::Windows)
            .expect_err("该相对形态必须被拒绝");
        assert_eq!(rejection, expected, "{input}");
        assert_chinese(rejection.message());
    }
}

/// 归一化正确性：注册一个根之后，大小写、分隔符与 `.` 变体必须命中同一个根。
#[test]
fn equivalent_spellings_hit_the_same_windows_root() {
    let base = normalize_absolute(r"D:\work\repo", PathDialect::Windows).expect("根必须可归一化");
    for spelling in [
        r"D:\work\repo",
        r"d:/WORK/repo/",
        r"D:\work\repo\.",
        r"D:\WORK\Repo\\",
        r"D:/work/./repo/",
    ] {
        let candidate =
            normalize_absolute(spelling, PathDialect::Windows).expect("等价写法必须可归一化");
        assert!(
            is_within(&candidate, &base, PathDialect::Windows),
            "{spelling} 必须命中同一个根"
        );
        assert!(
            is_within(&base, &candidate, PathDialect::Windows),
            "{spelling} 与根必须互为包含"
        );
    }
}

/// `..` 回到根内是合法的，只要不越过根。
#[test]
fn parent_traversal_stays_legal_inside_the_root() {
    let base = normalize_absolute(r"D:\work\repo", PathDialect::Windows).expect("根必须可归一化");
    let candidate = normalize_relative(&base, r"src\..\docs\a.md", PathDialect::Windows)
        .expect("根内回退必须合法");
    assert_eq!(
        candidate.display(PathDialect::Windows),
        r"D:\work\repo\docs\a.md"
    );
}

/// 盘根的展示形态：只有盘符一个组件时必须补尾分隔符，否则 `C:` 是驱动器相对路径。
#[test]
fn a_bare_drive_root_keeps_its_trailing_separator() {
    let root = normalize_absolute(r"C:\", PathDialect::Windows).expect("盘根必须可归一化");
    assert_eq!(root.components().len(), 1);
    assert_eq!(root.display(PathDialect::Windows), r"C:\");

    let child = normalize_relative(&root, r"work\repo", PathDialect::Windows)
        .expect("盘根下的相对路径必须合法");
    assert_eq!(child.display(PathDialect::Windows), r"C:\work\repo");
    assert!(is_within(&child, &root, PathDialect::Windows));
}

/// 前缀比较按组件进行：`/a/bc` 不得命中 `/a/b`。
#[test]
fn component_prefix_comparison_never_matches_a_sibling() {
    let root = normalize_absolute(r"C:\a\b", PathDialect::Windows).expect("根必须可归一化");
    for sibling in [r"C:\a\bc", r"C:\a\bc\d", r"C:\ab"] {
        let child =
            normalize_absolute(sibling, PathDialect::Windows).expect("兄弟目录必须可归一化");
        assert!(
            !is_within(&child, &root, PathDialect::Windows),
            "{sibling} 不属于 {root:?} 之内"
        );
    }
    let child = normalize_absolute(r"C:\a\b\c", PathDialect::Windows).expect("子目录必须可归一化");
    assert!(is_within(&child, &root, PathDialect::Windows));
    // 反向包含不成立：根不在子目录之内。
    assert!(!is_within(&root, &child, PathDialect::Windows));
}

/// 盘符是路径组件的一部分，不同盘的同名目录互不包含。
#[test]
fn different_drives_never_contain_each_other() {
    let root = normalize_absolute(r"C:\work\repo", PathDialect::Windows).expect("根必须可归一化");
    let other = normalize_absolute(r"D:\work\repo\a.txt", PathDialect::Windows)
        .expect("另一盘的同名路径必须可归一化");
    assert!(!is_within(&other, &root, PathDialect::Windows));
}

/// POSIX 组：`..` 跳出、大小写敏感、冒号是合法文件名字符。
#[test]
fn posix_semantics_are_asserted_independently_of_the_host() {
    let root = normalize_absolute("/srv/work/repo", PathDialect::Posix).expect("根必须可归一化");
    reject_absolute("/..", PathDialect::Posix, Rejection::ParentEscape);
    reject_absolute(
        "/srv/../../../etc/passwd",
        PathDialect::Posix,
        Rejection::ParentEscape,
    );
    reject_absolute("relative/path", PathDialect::Posix, Rejection::NotAbsolute);

    // 大小写敏感：`/SRV/work/repo` 不是同一个根。
    let upper = normalize_absolute("/SRV/work/repo", PathDialect::Posix).expect("必须可归一化");
    assert!(!is_within(&upper, &root, PathDialect::Posix));
    assert!(!is_within(&root, &upper, PathDialect::Posix));

    // 冒号与结尾空格在 POSIX 下是合法字符，不能被 Windows 的规则误伤。
    let colon = normalize_absolute("/srv/work/repo/file.txt::$DATA", PathDialect::Posix)
        .expect("POSIX 下冒号是合法文件名");
    assert_eq!(
        colon.display(PathDialect::Posix),
        "/srv/work/repo/file.txt::$DATA"
    );
    let spaced = normalize_absolute("/srv/work/repo/name ", PathDialect::Posix)
        .expect("POSIX 下结尾空格是合法文件名");
    assert_eq!(spaced.display(PathDialect::Posix), "/srv/work/repo/name ");

    // POSIX 下反斜杠不是分隔符。
    let backslash = normalize_absolute(r"/srv/work/repo/a\b", PathDialect::Posix)
        .expect("反斜杠在 POSIX 下是普通字符");
    assert_eq!(backslash.components().len(), 4);
}

/// 空字符与控制字符两平台都拒绝。
#[test]
fn nul_and_control_characters_are_rejected_on_both_dialects() {
    for dialect in [PathDialect::Windows, PathDialect::Posix] {
        let nul = normalize_absolute("/a\u{0}b", dialect).expect_err("空字符必须被拒绝");
        assert_eq!(nul, Rejection::NullByte);
        let control = normalize_absolute("/a\u{7}b", dialect).expect_err("控制字符必须被拒绝");
        assert_eq!(control, Rejection::ControlCharacter);
        assert_eq!(
            normalize_absolute("", dialect).expect_err("空路径必须被拒绝"),
            Rejection::Empty
        );
    }
}

/// 绝对路径判定按方言区分。
#[test]
fn absolute_detection_follows_the_dialect() {
    assert!(is_absolute_input(r"C:\work", PathDialect::Windows));
    assert!(!is_absolute_input(r"C:work", PathDialect::Windows));
    assert!(!is_absolute_input(r"\work", PathDialect::Windows));
    assert!(!is_absolute_input("/srv/work", PathDialect::Windows));
    assert!(is_absolute_input("/srv/work", PathDialect::Posix));
    assert!(!is_absolute_input(r"C:\work", PathDialect::Posix));
}

/// `\\?\` 前缀在对外输出里必须被剥掉。
#[test]
fn verbatim_prefix_is_stripped_from_outputs() {
    assert_eq!(strip_verbatim_prefix(r"\\?\C:\work\repo"), r"C:\work\repo");
    assert_eq!(strip_verbatim_prefix(r"C:\work\repo"), r"C:\work\repo");
    assert_eq!(
        strip_verbatim_prefix(r"\\?\UNC\server\share"),
        r"UNC\server\share"
    );
}

/// 拒绝原因与 HTTP 拒绝码的映射必须与合同一致。
#[test]
fn rejections_carry_the_contract_status_codes() {
    assert_eq!(Rejection::OutsideRoot.denied_status(), 403);
    assert_eq!(Rejection::ParentEscape.denied_status(), 403);
    assert_eq!(Rejection::DataStream.denied_status(), 403);
    assert_eq!(Rejection::DeviceNamespace.denied_status(), 403);
    assert_eq!(Rejection::Unc.denied_status(), 403);
    assert_eq!(Rejection::DriveRelative.denied_status(), 403);
    assert_eq!(Rejection::TrailingDotOrSpace.denied_status(), 403);
    assert_eq!(Rejection::ShortNameFallback.denied_status(), 403);
    assert_eq!(Rejection::VerbatimPrefix.denied_status(), 403);
    assert_eq!(Rejection::Empty.denied_status(), 400);
    assert_eq!(Rejection::NotAbsolute.denied_status(), 400);
    assert_eq!(Rejection::ResolveFailed.denied_status(), 404);
}

/// 工作区内的正常读取：相对路径、绝对路径、根自身三条入口都成立。
#[test]
fn resolved_paths_inside_the_root_are_absolute_and_prefix_free() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    std::fs::create_dir_all(root.join("src")).expect("测试目录必须可创建");
    std::fs::write(root.join("src").join("a.txt"), b"a").expect("测试文件必须可写");

    for relative in ["src/a.txt", r"src\a.txt", ""] {
        let resolved =
            resolve_within(root, relative, PathDialect::host()).expect("根内路径必须解析成功");
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "对外路径不得带长路径前缀：{}",
            resolved.display()
        );
    }
    let absolute = root.join("src").join("a.txt");
    let resolved = resolve_within(root, &absolute.to_string_lossy(), PathDialect::host())
        .expect("根内绝对路径必须解析成功");
    assert!(resolved.ends_with("a.txt"));
}

/// 绝对路径跳出根必须被拒绝，且拒绝结论是 403 口径。
#[test]
fn an_absolute_path_outside_the_root_is_rejected() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path().join("root");
    std::fs::create_dir_all(&root).expect("测试目录必须可创建");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&outside).expect("测试目录必须可创建");
    std::fs::write(outside.join("secret.txt"), b"secret").expect("测试文件必须可写");

    let rejection = resolve_within(
        &root,
        &outside.join("secret.txt").to_string_lossy(),
        PathDialect::host(),
    )
    .expect_err("根外绝对路径必须被拒绝");
    assert_eq!(rejection, Rejection::OutsideRoot);
    assert_eq!(rejection.denied_status(), 403);
}

/// 不存在的目标按「最近存在祖先 + 尾部组件」解析，不报错也不越界。
#[test]
fn absent_targets_resolve_through_the_nearest_existing_ancestor() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    let resolved = resolve_within(root, "not/created/yet.txt", PathDialect::host())
        .expect("未创建的目标必须能给出等价路径");
    assert!(resolved.ends_with("yet.txt"), "{}", resolved.display());
    // 上一级再上一级都不存在时，也必须收敛到根内。
    let deep =
        resolve_within(root, "a/b/c/d/e.txt", PathDialect::host()).expect("深层缺失必须可解析");
    assert!(deep.ends_with("e.txt"));
}

/// 不存在的根必须报「根目录不可用」，不得把历史路径当成合法根。
#[test]
fn an_absent_root_is_reported_as_unavailable() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path().join("absent");
    let rejection = resolve_within(&root, "", PathDialect::host()).expect_err("缺失的根必须被拒绝");
    assert_eq!(rejection, Rejection::RootUnavailable);
    assert_eq!(rejection.denied_status(), 404);
}

/// 附件目录自成一类根：目录自身与其中任何一项都不得被当作可浏览路径。
#[test]
fn the_attachments_directory_is_never_browsable() {
    let directory = axon_test_support::test_temp_dir();
    let axon_root = directory.path();
    let attachments = axon_root.join("data").join("attachments");
    std::fs::create_dir_all(attachments.join("uploaded")).expect("测试目录必须可创建");

    let dialect = PathDialect::host();
    for candidate in [
        resolve_within(axon_root, "data/attachments", dialect).expect("附件目录必须可解析"),
        resolve_within(axon_root, "data/attachments/uploaded", dialect)
            .expect("附件子目录必须可解析"),
    ] {
        assert!(
            is_attachments_path(&candidate, axon_root, dialect),
            "{} 必须被认成附件目录",
            candidate.display()
        );
    }
    for candidate in [
        resolve_within(axon_root, "data", dialect).expect("data 目录必须可解析"),
        resolve_within(axon_root, "", dialect).expect("根必须可解析"),
        axon_root.join("data").join("attachments-other"),
    ] {
        assert!(
            !is_attachments_path(&candidate, axon_root, dialect),
            "{} 不是附件目录",
            candidate.display()
        );
    }
}

/// 绝对路径解析：存在与不存在两种目标都能给出可用路径。
#[test]
fn absolute_resolution_handles_both_existing_and_absent_targets() {
    let directory = axon_test_support::test_temp_dir();
    let existing = directory.path().join("here.txt");
    std::fs::write(&existing, b"here").expect("测试文件必须可写");
    let dialect = PathDialect::host();

    let resolved =
        resolve_absolute(&existing.to_string_lossy(), dialect).expect("存在的目标必须可解析");
    assert!(resolved.ends_with("here.txt"));
    let absent = resolve_absolute(
        &directory.path().join("gone.txt").to_string_lossy(),
        dialect,
    )
    .expect("不存在的目标必须给出等价路径");
    assert!(absent.ends_with("gone.txt"));
}

/// 创建目录联接；失败即判定为环境不支持，直接让测试红掉而不是静默跳过。
#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("创建目录联接需要 cmd");
    assert!(status.success(), "测试环境必须能创建目录联接");
    assert!(link.exists(), "目录联接必须落盘");
}

/// 指向工作区之外的目录联接必须被拒绝（合同 §9 阶段 3 的 Windows 越界组）。
#[cfg(windows)]
#[test]
fn a_junction_pointing_outside_the_root_is_rejected() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path().join("root");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&root).expect("测试目录必须可创建");
    std::fs::create_dir_all(&outside).expect("测试目录必须可创建");
    std::fs::write(outside.join("secret.txt"), b"secret").expect("测试文件必须可写");
    create_junction(&root.join("link"), &outside);

    let dialect = PathDialect::Windows;
    for relative in ["link", "link/secret.txt", r"link\."] {
        let rejection =
            resolve_within(&root, relative, dialect).expect_err("指向根外的目录联接必须被拒绝");
        assert_eq!(rejection, Rejection::OutsideRoot, "{relative}");
        assert_chinese(rejection.message());
    }
}

/// 指向工作区内部的目录联接与真实目录等价：必须命中同一个工作区。
#[cfg(windows)]
#[test]
fn a_junction_inside_the_root_is_equivalent_to_its_target() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path().join("root");
    let inside = root.join("inside");
    std::fs::create_dir_all(&inside).expect("测试目录必须可创建");
    std::fs::write(inside.join("a.txt"), b"a").expect("测试文件必须可写");
    create_junction(&root.join("alias"), &inside);

    let dialect = PathDialect::Windows;
    let direct = resolve_within(&root, "inside/a.txt", dialect).expect("真实目录必须可解析");
    let via_link = resolve_within(&root, "alias/a.txt", dialect).expect("联接目录必须可解析");
    assert_eq!(direct, via_link, "联接与真实目录必须收敛到同一个工作区路径");
    assert!(!via_link.to_string_lossy().starts_with(r"\\?\"));
}

/// 指向根外的符号链接必须被拒绝（Linux 与 macOS 的越界组）。
#[cfg(unix)]
#[test]
fn a_symlink_pointing_outside_the_root_is_rejected() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path().join("root");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&root).expect("测试目录必须可创建");
    std::fs::create_dir_all(&outside).expect("测试目录必须可创建");
    std::fs::write(outside.join("secret.txt"), b"secret").expect("测试文件必须可写");
    std::os::unix::fs::symlink(&outside, root.join("link")).expect("测试环境必须能建符号链接");

    let rejection = resolve_within(&root, "link/secret.txt", PathDialect::Posix)
        .expect_err("根外符号链接必须被拒绝");
    assert_eq!(rejection, Rejection::OutsideRoot);
}
