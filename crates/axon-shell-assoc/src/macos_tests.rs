//! macOS 平台解析的单元验证：LaunchServices JSON 的形状校验、同应用多份拷贝的折叠、版本比较。
//!
//! 这些都只吃字符串，因此在任何平台（含开发机上的 Windows 与 CI）都能**真验证**，
//! 不需要一台 macOS 机器。`macos.rs` 里真正去调 `osascript` / `open` 的部分
//! 只能靠真机观察，这里不为它编造断言。

use super::macos_applications::{self, MacApplication};

/// 造一条 macOS 查询结果里的应用条目。
fn mac_app(
    id: &str,
    name: &str,
    default: bool,
    bundle: Option<&str>,
    version: Option<&str>,
) -> MacApplication {
    MacApplication {
        id: id.to_owned(),
        name: name.to_owned(),
        default,
        icon: None,
        bundle: bundle.map(str::to_owned),
        version: version.map(str::to_owned),
    }
}

#[test]
fn mac_parse_accepts_a_well_formed_payload() {
    let parsed = macos_applications::parse_applications(
        r#"[
            {"id":"/Applications/Preview.app","name":"Preview","default":true,
             "icon":"data:image/png;base64,AAAA","bundle":"com.apple.Preview","version":"11.0"},
            {"id":"/Applications/Safari.app","name":"Safari","default":false,"icon":null,
             "bundle":null,"version":null}
        ]"#,
    )
    .expect("形状合法的结果必须解析成功");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, "/Applications/Preview.app");
    assert!(parsed[0].default, "默认标记必须原样读出来");
    assert_eq!(parsed[0].bundle.as_deref(), Some("com.apple.Preview"));
    assert_eq!(parsed[1].icon, None, "null 图标必须读成「没有图标」");
    assert_eq!(parsed[1].bundle, None, "null 标识必须读成「没有标识」");
}

#[test]
fn mac_parse_rejects_the_whole_payload_when_one_entry_is_malformed() {
    // 一条不合法即整批判失败：宁可不显示，也不摆一份半懂的结果给用户。
    for payload in [
        r#"[{"id":"","name":"空标识","default":false,"icon":null}]"#,
        r#"[{"name":"缺标识","default":false,"icon":null}]"#,
        r#"[{"id":"/A.app","name":"类型不对","default":"yes","icon":null}]"#,
        r#"[{"id":"/A.app","name":"图标不是 data URL","default":false,"icon":"http://x/y.png"}]"#,
        r#"[{"id":"/A.app","name":"图标前缀不对","default":false,"icon":"data:image/gif;base64,AAAA"}]"#,
        r#"{"id":"/A.app"}"#,
        "不是 JSON",
    ] {
        assert!(
            macos_applications::parse_applications(payload).is_none(),
            "必须整批拒绝：{payload}"
        );
    }
}

#[test]
fn mac_dedupe_prefers_the_default_then_the_higher_version_in_place() {
    let folded = macos_applications::dedupe_applications(vec![
        mac_app(
            "/Applications/X.app",
            "X",
            false,
            Some("com.x"),
            Some("1.2"),
        ),
        mac_app(
            "/Applications/Other.app",
            "Other",
            false,
            Some("com.o"),
            None,
        ),
        // 同一应用的暂存副本：版本更高，但不是系统默认。
        mac_app("/tmp/updates/X.app", "X", false, Some("com.x"), Some("2.0")),
        mac_app("/Applications/X.app", "X", true, Some("com.x"), Some("1.0")),
    ]);
    assert_eq!(
        folded.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
        ["/Applications/X.app", "/Applications/Other.app"],
        "折叠后必须留在该组**首次出现**的位置，且只留一条"
    );
    assert!(folded[0].default, "系统默认压过版本号：默认的那份必须赢");
}

#[test]
fn mac_dedupe_uses_the_version_when_no_copy_is_default() {
    let folded = macos_applications::dedupe_applications(vec![
        mac_app("/A/X.app", "X", false, Some("com.x"), Some("1.9")),
        mac_app("/B/X.app", "X", false, Some("com.x"), Some("1.10")),
        mac_app("/C/X.app", "X", false, Some("com.x"), None),
    ]);
    assert_eq!(folded.len(), 1);
    assert_eq!(
        folded[0].id, "/B/X.app",
        "没有默认项时留版本最高的那份，且版本按数值比（1.10 > 1.9）"
    );
}

#[test]
fn mac_dedupe_keeps_differently_named_installs_and_ungrouped_entries() {
    let folded = macos_applications::dedupe_applications(vec![
        mac_app(
            "/Applications/Xcode.app",
            "Xcode",
            false,
            Some("com.apple.dt.Xcode"),
            Some("16.0"),
        ),
        mac_app(
            "/Applications/Xcode-26.app",
            "Xcode-26",
            false,
            Some("com.apple.dt.Xcode"),
            Some("26.0"),
        ),
        // 没有 bundle 标识：证明不了自己与别人是同一个应用，因此各留各的。
        mac_app("/opt/a/tool", "tool", false, None, None),
        mac_app("/opt/b/tool", "tool", false, None, None),
    ]);
    assert_eq!(
        folded
            .iter()
            .map(|app| app.name.as_str())
            .collect::<Vec<_>>(),
        ["Xcode", "Xcode-26", "tool", "tool"],
        "显示名不同就不折叠；没有 bundle 标识的条目不参与分组"
    );
}

#[test]
fn mac_version_comparison_treats_missing_as_lowest_and_non_numeric_as_zero() {
    use std::cmp::Ordering;
    let compare = macos_applications::compare_versions;
    assert_eq!(compare(Some("1.10"), Some("1.9")), Ordering::Greater);
    assert_eq!(compare(Some("1.0"), Some("1.0.0")), Ordering::Equal);
    assert_eq!(compare(Some("1.0"), None), Ordering::Greater);
    assert_eq!(compare(None, Some("0.1")), Ordering::Less);
    assert_eq!(compare(None, None), Ordering::Equal);
    assert_eq!(
        compare(Some("26.0-beta"), Some("26.0")),
        Ordering::Equal,
        "非数字段按 0 计，因此与缺段等价"
    );
}

// ---------------------------------------------------------------------------
