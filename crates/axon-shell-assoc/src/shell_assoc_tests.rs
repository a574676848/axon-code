//! `axon-shell-assoc` 的单元验证。
//!
//! 覆盖两类东西：
//!
//! 1. **纯逻辑**：扩展名提取、环境变量展开、PNG data URL 拼装、候选去重与顺序保持；
//! 2. **平台解析**：Linux 的 `.desktop` / `mimeapps.list` / glob 规则解析：这些只吃字符串，
//!    因此在本机（Windows）开发环境下也能**真验证**，不需要 Linux 机器；
//!    macOS 的 LaunchServices JSON 解析与拷贝折叠在 `macos_tests`。
//!
//! **不**为系统调用本身编造断言：`SHAssocEnumHandlers` 对具体文件返回什么，只能由
//! [`smoke_lists_real_candidates`] 这类 `#[ignore]` 的只读冒烟用例在本机原样打出来观察，
//! 而不是写成一组会在别的机器上翻车的断言。

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    FileApplication, ICON_SIZE, desktop_entry,
    mime_glob::{self, GlobRule},
    normalize_candidates, windows_helpers,
};

/// 测试源码的文件名后缀：它们自身会提到 `unsafe` 关键字，必须排除在扫描之外。
const TEST_SOURCE_SUFFIX: &str = "_tests.rs";

/// 仅有的两个允许出现 `unsafe` 的源文件（都是 Windows 平台适配器）。
const UNSAFE_SOURCE_FILES: &[&str] = &["windows.rs", "windows_icon.rs"];

/// 扫描源目录的重试次数与间隔：并行跑全量测试时可能短暂耗尽文件句柄。
const SOURCE_SCAN_ATTEMPTS: usize = 3;
const SOURCE_SCAN_RETRY_DELAY: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------
// 候选去重与顺序
// ---------------------------------------------------------------------------

/// 造一条只关心去重与顺序字段的候选。
fn candidate(id: &str, name: &str, default: bool) -> FileApplication {
    FileApplication {
        id: id.to_owned(),
        name: name.to_owned(),
        icon: None,
        default,
    }
}

/// 取候选的标识序列，便于整体比较。
fn ids(applications: &[FileApplication]) -> Vec<&str> {
    applications.iter().map(|item| item.id.as_str()).collect()
}

#[test]
fn normalize_drops_duplicate_ids_ignoring_case_and_keeps_the_first() {
    let normalized = normalize_candidates(vec![
        candidate(r"C:\A\Tool.exe", "Tool", false),
        candidate(r"c:\a\tool.EXE", "重复的另一个写法", false),
        candidate(r"C:\B\Other.exe", "Other", false),
    ]);
    assert_eq!(normalized.len(), 2, "只差大小写的同一个标识必须合并成一条");
    // 顺序沿用适配器给出的顺序：去重只做去重，不重排。
    assert_eq!(
        ids(&normalized),
        [r"C:\A\Tool.exe", r"C:\B\Other.exe"],
        "去重必须保留先出现的位置，不得因为排序而改变系统给出的优先级"
    );
    let tool = normalized
        .iter()
        .find(|item| item.id == r"C:\A\Tool.exe")
        .expect("去重必须留下先出现的那条标识");
    assert_eq!(
        tool.name, "Tool",
        "去重必须保留先出现的那条：适配器按自身优先级返回，先出现的更可信"
    );
}

#[test]
fn normalize_keeps_the_adapter_order_instead_of_sorting_by_name() {
    let normalized = normalize_candidates(vec![
        candidate("c", "Zebra", false),
        candidate("a", "alpha", false),
        candidate("b", "Beta", true),
    ]);
    assert_eq!(
        ids(&normalized),
        ["c", "a", "b"],
        "顺序必须原样沿用适配器给出的系统偏好顺序：默认项只加标记，不置顶，也不按名称重排"
    );
    assert!(normalized[2].default, "默认标记必须留在原来的位置上");
}

#[test]
fn normalize_keeps_the_enumeration_order_and_drops_empty_ids() {
    let normalized = normalize_candidates(vec![
        candidate("second", "同名", false),
        candidate("", "没有标识就无法回传选择", false),
        candidate("first", "同名", false),
    ]);
    assert_eq!(
        ids(&normalized),
        ["second", "first"],
        "同名候选必须保持枚举顺序；空标识的候选必须丢弃"
    );
}

// ---------------------------------------------------------------------------
// Shell 扩展名提取
// ---------------------------------------------------------------------------

#[test]
fn shell_extension_keeps_the_dot_and_the_original_case() {
    assert_eq!(
        windows_helpers::shell_extension(Path::new("report.json")).as_deref(),
        Some(".json")
    );
    assert_eq!(
        windows_helpers::shell_extension(Path::new("archive.tar.gz")).as_deref(),
        Some(".gz"),
        "只认最后一个后缀，与 Shell 的扩展名规则一致"
    );
    assert_eq!(
        windows_helpers::shell_extension(Path::new("dir.d/file.TXT")).as_deref(),
        Some(".TXT"),
        "大小写原样保留：Shell 自己做不区分大小写的匹配"
    );
}

#[test]
fn shell_extension_reports_none_for_names_without_a_usable_extension() {
    for name in ["README", ".gitignore", "report.", "Cargo"] {
        assert_eq!(
            windows_helpers::shell_extension(Path::new(name)),
            None,
            "{name} 不应被当成「有扩展名」"
        );
    }
}

// ---------------------------------------------------------------------------
// 图标位置的环境变量展开
// ---------------------------------------------------------------------------

#[test]
fn expand_environment_variables_keeps_unresolved_references_verbatim() {
    // 变量名刻意取成不可能存在的值，因此结果不依赖运行环境。
    let unresolved = r"%AXON_SHELL_ASSOC_NOT_DEFINED%\shell32.dll,-152";
    assert_eq!(
        windows_helpers::expand_environment_variables(unresolved),
        unresolved,
        "未定义的变量必须原样保留，而不是替换成空串把路径截断"
    );
    assert_eq!(windows_helpers::expand_environment_variables("%%"), "%%");
    assert_eq!(windows_helpers::expand_environment_variables("50%"), "50%");
    assert_eq!(
        windows_helpers::expand_environment_variables(r"C:\Windows\system32\shell32.dll"),
        r"C:\Windows\system32\shell32.dll",
        "不含 % 的输入必须原样返回"
    );
}

#[test]
fn expand_environment_variables_replaces_a_reference_that_resolves() {
    // 目标变量与期望值都从运行环境里取，断言的是「引用被替换成了变量值」而不是具体路径。
    // 两个候选变量都不存在的裸环境里直接跳过，避免测试依赖运行环境。
    let resolved = [
        ("HOME", std::env::var("HOME")),
        ("USERPROFILE", std::env::var("USERPROFILE")),
    ]
    .into_iter()
    .find_map(|(name, value)| value.ok().map(|value| (name, value)));
    let Some((name, value)) = resolved else {
        return;
    };
    assert_eq!(
        windows_helpers::expand_environment_variables(&format!("%{name}%/nested")),
        format!("{value}/nested")
    );
}

// ---------------------------------------------------------------------------
// PNG data URL 拼装
// ---------------------------------------------------------------------------

#[test]
fn png_data_url_round_trips_the_pixels_it_was_given() {
    // 2×2 的四色图（含一个全透明像素）：解码回来必须逐字节相同，否则「图标是 RGBA」的契约
    // 只是名义上的。
    let pixels: [u8; 16] = [
        255, 0, 0, 255, // 红
        0, 255, 0, 255, // 绿
        0, 0, 255, 255, // 蓝
        255, 255, 255, 0, // 全透明
    ];
    let url = windows_helpers::png_data_url(&pixels, 2, 2).expect("2×2 RGBA 必须能编码");
    let encoded = url
        .strip_prefix("data:image/png;base64,")
        .expect("data URL 前缀是公开契约的一部分");
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("载荷必须是合法 base64");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "载荷必须真的是 PNG");

    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
        .read_info()
        .expect("刚编码出来的 PNG 必须能被解回来");
    let mut buffer = vec![0_u8; reader.output_buffer_size().expect("输出缓冲大小可计算")];
    let info = reader.next_frame(&mut buffer).expect("必须能解出一帧");
    assert_eq!((info.width, info.height), (2, 2));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(&buffer[..pixels.len()], &pixels[..]);
}

#[test]
fn png_data_url_rejects_pixel_buffers_that_do_not_match_the_dimensions() {
    assert_eq!(
        windows_helpers::png_data_url(&[0; 15], 2, 2),
        None,
        "长度不符时必须退化而不是编码出半张图"
    );
    assert_eq!(windows_helpers::png_data_url(&[], 0, 0), None);
}

#[test]
fn icon_size_is_the_documented_thirty_two_pixels() {
    assert_eq!(
        ICON_SIZE, 32,
        "公开契约是 32×32；改它必须同步改 Shell 请求尺寸、缩放目标与本用例"
    );
}

#[test]
fn to_wide_produces_a_nul_terminated_utf16_buffer() {
    let wide = windows_helpers::to_wide(".json");
    assert_eq!(wide.last(), Some(&0));
    assert_eq!(
        String::from_utf16(&wide[..wide.len() - 1]).unwrap(),
        ".json"
    );

    // 非 BMP 字符必须编成代理对，否则 Shell 收到的是半个字符。
    let wide = windows_helpers::to_wide("😀.txt");
    assert_eq!(wide.last(), Some(&0));
    assert_eq!(wide.len(), 2 + 1 + 3 + 1);
}

// ---------------------------------------------------------------------------
// 桌面条目解析（Linux 数据源，纯文本因此可在本机验证）
// ---------------------------------------------------------------------------

/// 一份覆盖常见键的桌面条目 fixture：含注释、无关组、本地化变体与字段码。
const DESKTOP_FIXTURE: &str = "\
# 注释行必须被忽略
[Desktop Entry]
Type=Application
Name=Feather Pad
Name[zh_CN]=羽毛记事本
Exec=featherpad --standalone %U
Icon=featherpad
MimeType=text/plain;application/json;
Terminal=false

[Desktop Action new-window]
Exec=featherpad --new-window
Name=New Window
";

#[test]
fn desktop_entry_reads_only_the_main_group() {
    let entry = desktop_entry::parse_desktop_entry(DESKTOP_FIXTURE);
    assert_eq!(entry.kind.as_deref(), Some("Application"));
    assert_eq!(
        entry.name.as_deref(),
        Some("Feather Pad"),
        "本地化变体不覆盖无后缀的 Name 键"
    );
    assert_eq!(entry.mime_types, ["text/plain", "application/json"]);
    assert_eq!(
        entry.exec,
        ["featherpad", "--standalone"],
        "`%U` 必须整段丢弃；[Desktop Action] 组的 Exec 不能串进主组"
    );
    assert!(!entry.hidden);
}

#[test]
fn desktop_entry_keeps_the_first_duplicate_key_and_defaults_to_not_hidden() {
    let entry = desktop_entry::parse_desktop_entry("[Desktop Entry]\nName=First\nName=Second\n");
    assert_eq!(
        entry.name.as_deref(),
        Some("First"),
        "重复键以第一次为准：后面的脏行不能覆盖已解析出来的值"
    );
    assert!(!entry.hidden, "缺少 Hidden 键时必须是「未隐藏」");
    assert!(desktop_entry::parse_desktop_entry("[Desktop Entry]\nHidden=TRUE\n").hidden);
}

#[test]
fn display_name_falls_back_from_name_to_exec_then_to_the_id() {
    let entry = desktop_entry::parse_desktop_entry(DESKTOP_FIXTURE);
    assert_eq!(
        desktop_entry::display_name_from_exec(&entry.exec, "featherpad.desktop"),
        "featherpad"
    );
    assert_eq!(
        desktop_entry::display_name_from_exec(&["/opt/My App/run".to_owned()], "x.desktop"),
        "run"
    );
    assert_eq!(
        desktop_entry::display_name_from_exec(&[], "x.desktop"),
        "x.desktop",
        "没有 Exec 时只能退回桌面条目 id",
    );
}

// ---------------------------------------------------------------------------
// Exec 字段码与引号
// ---------------------------------------------------------------------------

#[test]
fn exec_arguments_strip_every_field_code() {
    assert_eq!(
        desktop_entry::exec_arguments("/usr/bin/app %f"),
        ["/usr/bin/app"]
    );
    assert_eq!(
        desktop_entry::exec_arguments("/usr/bin/app %F %u %U %i %c %k"),
        ["/usr/bin/app"],
        "占位符由启动器替换，本 crate 不启动进程，因此整段丢弃"
    );
    assert_eq!(
        desktop_entry::exec_arguments("/usr/bin/app %d %D %n %N %v %m"),
        ["/usr/bin/app"],
        "已废弃的字段码同样丢弃"
    );
    assert_eq!(
        desktop_entry::exec_arguments("/usr/bin/app 100%% %f"),
        ["/usr/bin/app", "100%"],
        "`%%` 是字面量百分号"
    );
}

#[test]
fn exec_arguments_handle_quotes_and_drop_pure_placeholder_arguments() {
    assert_eq!(
        desktop_entry::exec_arguments(r#"/usr/bin/app --title "Two Words" %U"#),
        ["/usr/bin/app", "--title", "Two Words"],
        "引号内的空格属于同一个参数，引号本身不进入结果"
    );
    assert_eq!(
        desktop_entry::exec_arguments(r#""/opt/My App/run" "%f""#),
        ["/opt/My App/run"],
        "只剩占位符的参数整条消失"
    );
    assert!(
        desktop_entry::exec_arguments("%f").is_empty(),
        "全占位符的 Exec 没有可执行命令，不构成可用候选"
    );
    assert!(desktop_entry::exec_arguments("").is_empty());
}

// ---------------------------------------------------------------------------
// mimeapps.list 解析
// ---------------------------------------------------------------------------

/// 一份 `mimeapps.list` fixture：默认组之外还有必须被忽略的组。
const MIMEAPPS_FIXTURE: &str = "\
[Default Applications]
application/json=org.gnome.TextEditor.desktop;
text/plain=org.gnome.TextEditor.desktop;vim.desktop;

[Added Associations]
application/json=vim.desktop;
";

#[test]
fn mimeapps_defaults_are_flattened_in_priority_order() {
    assert_eq!(
        desktop_entry::parse_mimeapps_defaults(MIMEAPPS_FIXTURE),
        [
            (
                "application/json".to_owned(),
                "org.gnome.TextEditor.desktop".to_owned()
            ),
            (
                "text/plain".to_owned(),
                "org.gnome.TextEditor.desktop".to_owned()
            ),
            ("text/plain".to_owned(), "vim.desktop".to_owned()),
        ],
        "同一 MIME 的候选必须保持登记顺序（第一个是默认项），其他组必须被忽略"
    );
}

#[test]
fn mimeapps_defaults_fold_the_mime_type_to_lowercase() {
    assert_eq!(
        desktop_entry::parse_mimeapps_defaults(
            "[Default Applications]\nApplication/JSON=x.desktop;\n"
        ),
        [("application/json".to_owned(), "x.desktop".to_owned())],
        "MIME 类型不区分大小写，折叠后才能与 glob 判定结果比较"
    );
    assert!(desktop_entry::parse_mimeapps_defaults("[Added Associations]\na=b;\n").is_empty());
}

// ---------------------------------------------------------------------------
// glob 匹配与 MIME 裁决（Linux 数据源，纯文本因此可在本机验证）
// ---------------------------------------------------------------------------

#[test]
fn glob_matches_supports_star_question_and_character_classes() {
    assert!(mime_glob::glob_matches("*.json", "data.json"));
    assert!(
        !mime_glob::glob_matches("*.json", "data.JSON"),
        "不做大小写折叠：数据文件把大小写变体登记成分开的规则"
    );
    assert!(mime_glob::glob_matches("Makefile", "Makefile"));
    assert!(mime_glob::glob_matches("*.tar.?z", "a.tar.gz"));
    assert!(!mime_glob::glob_matches("*.tar.?z", "a.tar.zst"));
    assert!(mime_glob::glob_matches("*.[ch]", "main.c"));
    assert!(mime_glob::glob_matches("*.[ch]", "main.h"));
    assert!(!mime_glob::glob_matches("*.[ch]", "main.o"));
    assert!(mime_glob::glob_matches("*.[a-z]", "main.c"));
    assert!(mime_glob::glob_matches("*.[!a-z]", "main.1"));
    assert!(!mime_glob::glob_matches("*.[!a-z]", "main.c"));
    assert!(
        mime_glob::glob_matches("*.[^a-z]", "main.1"),
        "`[^...]` 与 `[!...]` 等价"
    );
    assert!(!mime_glob::glob_matches("literal", "liter"));
    assert!(!mime_glob::glob_matches("*.json", "data.json.bak"));
    assert!(mime_glob::glob_matches("*", "任意名字"));
}

#[test]
fn globs2_parses_weights_and_skips_dirty_lines() {
    let rules = mime_glob::parse_globs2(
        "\
# 注释
50:application/json:cs:*.json
80:application/json:cs:*.jsonc
不是权重:text/plain::*.txt
字段不够
60:text/plain::*.txt
",
    );
    assert_eq!(
        rules,
        [
            GlobRule {
                mime_type: "application/json".to_owned(),
                pattern: "*.json".to_owned(),
                weight: 50,
            },
            GlobRule {
                mime_type: "application/json".to_owned(),
                pattern: "*.jsonc".to_owned(),
                weight: 80,
            },
            GlobRule {
                mime_type: "text/plain".to_owned(),
                pattern: "*.txt".to_owned(),
                weight: 60,
            },
        ],
        "单个脏行必须被跳过，而不是让整张表作废"
    );
}

#[test]
fn globs_legacy_format_uses_the_default_weight() {
    let rules = mime_glob::parse_globs("text/x-rust:*.rs\ntext/plain:*.txt:cs\n");
    assert_eq!(rules.len(), 2);
    assert!(
        rules.iter().all(|rule| rule.weight == 50),
        "旧格式没有权重字段，必须给统一的默认权重"
    );
    assert_eq!(
        rules[1].pattern, "*.txt",
        "第三个字段是标志位，不能混进匹配模式"
    );
}

#[test]
fn resolve_mime_type_prefers_higher_weight_then_the_earlier_rule() {
    let rules = [
        GlobRule {
            mime_type: "application/json".to_owned(),
            pattern: "*.json".to_owned(),
            weight: 50,
        },
        GlobRule {
            mime_type: "text/plain".to_owned(),
            pattern: "*.json".to_owned(),
            weight: 20,
        },
        GlobRule {
            mime_type: "application/x-ndjson".to_owned(),
            pattern: "*.json".to_owned(),
            weight: 50,
        },
    ];
    assert_eq!(
        mime_glob::resolve_mime_type(&rules, "data.json"),
        Some("application/json"),
        "权重相同时先出现的规则赢（数据目录按 XDG 优先级依次读入）"
    );
    assert_eq!(mime_glob::resolve_mime_type(&rules, "data.yaml"), None);
}

// ---------------------------------------------------------------------------
// unsafe 的许可边界
// ---------------------------------------------------------------------------

/// 判断源码是否**实际声明**了不安全代码，而不是仅在文档或字符串里提到该关键字。
///
/// 只认四种形式，且关键字必须出现在代码行而非注释行，避免本模块与 `lib.rs` 里
/// 「unsafe 的边界」这类说明造成假阳性。
fn declares_unsafe_code(source: &str) -> bool {
    source.lines().any(|line| {
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with('*') {
            return false;
        }
        ["unsafe {", "unsafe impl ", "unsafe fn ", "unsafe extern "]
            .iter()
            .any(|marker| code.contains(marker))
    })
}

#[test]
fn unsafe_code_is_confined_to_the_windows_adapters() {
    let sources = production_source_files();
    assert!(
        !sources.is_empty(),
        "未扫描到任何生产源文件，路径断言会静默变绿"
    );
    let mut offenders = Vec::new();
    let mut fenced_seen = 0_usize;
    for path in sources {
        let body = std::fs::read_to_string(&path).expect("生产源文件必须可读");
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if UNSAFE_SOURCE_FILES.contains(&name) {
            fenced_seen += 1;
            // 反向对照：路径或扫描逻辑一旦失效，这里必须失败而不是静默通过。
            assert!(
                declares_unsafe_code(&body),
                "{name} 必须真的声明 unsafe；否则正向断言失去意义"
            );
            continue;
        }
        if declares_unsafe_code(&body) {
            offenders.push(path);
        }
    }
    assert_eq!(
        fenced_seen,
        UNSAFE_SOURCE_FILES.len(),
        "扫描结果缺失受围栏的文件，扫描范围可能已失效"
    );
    assert!(
        offenders.is_empty(),
        "unsafe 只允许存在于 {UNSAFE_SOURCE_FILES:?}，违规文件：{offenders:?}"
    );
}

/// 列出 `src` 下的生产源文件（排除测试源码）。
///
/// 目录读取失败必须重试、最终带上原始错误上报：全量测试并行时进程可能短暂耗尽
/// 文件句柄，静默返回空集会把「环境抖动」伪装成「扫描逻辑失效」
/// （实测 `未扫描到任何生产源文件` 就是这样产生的失败）。
fn production_source_files() -> Vec<PathBuf> {
    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut last_error = None;
    for _ in 0..SOURCE_SCAN_ATTEMPTS {
        let mut files = Vec::new();
        match collect_rust_sources(&source_root, &mut files) {
            Ok(()) => return files,
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(SOURCE_SCAN_RETRY_DELAY);
    }
    panic!("扫描生产源文件失败，无法判定 unsafe 边界：{last_error:?}");
}

/// 递归收集 `.rs` 文件；`*_tests.rs` 是测试源码，不参与 `unsafe` 边界断言。
fn collect_rust_sources(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_sources(&path, files)?;
            continue;
        }
        let is_test_source = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(TEST_SOURCE_SUFFIX));
        if path.extension().is_some_and(|value| value == "rs") && !is_test_source {
            files.push(path);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 只读冒烟验证（需要真实系统状态，因此默认忽略）
// ---------------------------------------------------------------------------

/// 只读冒烟验证：对真实文件调用 [`super::applications_for`]，打印候选数量与前 5 项概要。
///
/// 运行命令（在仓库根执行）：
///
/// ```text
/// cargo test -p axon-shell-assoc -- --ignored --nocapture
/// ```
///
/// 这个用例**只读**：不改注册表、不改文件关联、不启动任何被枚举出来的应用。它存在的意义是
/// 观察系统调用的真实行为：DSH 的调研报告给出 `.json` 约 10 项、`.txt` 约 9 项这一量级，
/// 本用例把本机实测值原样打出来供人工比对，而**不**写成会随机器变化的断言。
///
/// Windows 适配器**只按扩展名**查关联、从不打开文件，因此后两项用不存在的路径当探针即可
/// （否则为了冒烟验证就得往仓库里塞一个空 `.txt`）。它们验证的是「按扩展名短路」这条路径。
///
/// Windows 上打出来的是 Shell 关联处理程序；其他平台打出来的是对应的 XDG 桌面条目，
/// 「10 项 / 9 项」这个量级只对 Windows 有比对意义。
#[ignore = "需要真实系统状态与桌面会话；用 --ignored 显式运行"]
#[test]
fn smoke_lists_real_candidates() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let targets = [
        (
            "工作区根的 .json 示例（真实文件）",
            manifest_dir.join("../../mcp_stdio.example.json"),
        ),
        (
            "本 crate 的 Cargo.toml（真实文件）",
            manifest_dir.join("Cargo.toml"),
        ),
        (
            "`.txt` 扩展名探针（文件无需存在）",
            PathBuf::from("axon-shell-assoc-smoke.txt"),
        ),
        ("无扩展名的路径（必须短路成空表）", PathBuf::from("README")),
    ];
    for (label, path) in targets {
        println!("=== {label}：{} ===", path.display());
        match super::applications_for(&path) {
            Ok(applications) => {
                // 图标失败率与默认项是本次里程碑最需要观测的两个数字：前者决定菜单观感，
                // 后者决定「默认打开方式」标注是否正确，两者都只在真实系统状态里才有意义。
                let with_icon = applications
                    .iter()
                    .filter(|application| application.icon.is_some())
                    .count();
                let default = applications
                    .iter()
                    .find(|application| application.default)
                    .map_or("(无)", |application| application.name.as_str());
                println!(
                    "候选数量：{}　图标覆盖：{with_icon}/{}　默认项：{default}",
                    applications.len(),
                    applications.len()
                );
                for application in applications.iter().take(5) {
                    println!(
                        "  name={:?} default={} icon={} id={}",
                        application.name,
                        application.default,
                        if application.icon.is_some() {
                            "有"
                        } else {
                            "无"
                        },
                        application.id
                    );
                }
            }
            Err(error) => println!("枚举失败：{error}"),
        }
    }
}

/// 端口级保底：拿不到文件名的路径在任何平台都必须返回空表，且不产生错误。
#[test]
fn applications_for_a_path_without_a_file_name_is_empty_everywhere() {
    assert!(
        super::applications_for(Path::new(""))
            .expect("无文件名的路径不应产生错误，只应产生空表")
            .is_empty()
    );
}

/// Windows 的「无扩展名」分支必须在进入 Shell 枚举之前就短路：这条断言之所以成立，
/// 靠的正是 `shell_extension` 返回 `None` 时直接 `return Ok(Vec::new())`。
#[cfg(windows)]
#[test]
fn applications_for_a_name_without_extension_is_empty_on_windows() {
    assert!(
        super::applications_for(Path::new("README"))
            .expect("无扩展名不应产生错误")
            .is_empty()
    );
}
