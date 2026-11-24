//! 技能清单路由测试：清单形状、前置块两种写法、缺失 SKILL.md 的兜底、
//! 正文不外泄、移除的边界（越界标识、软链接、不存在）、方法边界与未命中回落。

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{SKILLS_PATH, SkillsHttpAdapter};

/// 构造一个指向临时运行时根的适配器。
fn fixture() -> (axon_test_support::TestTempDir, SkillsHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = SkillsHttpAdapter::new(directory.path().to_path_buf());
    (directory, adapter)
}

/// 在测试运行时根下铺一个技能目录与它的 `SKILL.md`。
fn write_skill(root: &std::path::Path, id: &str, body: &str) {
    let directory = root.join("skills").join(id);
    std::fs::create_dir_all(&directory).expect("技能目录必须可创建");
    std::fs::write(directory.join("SKILL.md"), body).expect("SKILL.md 必须可写");
}

/// 发一次请求并按 JSON 解析回执。
fn call(adapter: &SkillsHttpAdapter, method: &str, path: &str) -> (u16, Value) {
    let response = adapter
        .handle_request(method, path, None, &[])
        .expect("技能路由必须命中");
    let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, payload)
}

#[test]
fn an_absent_skills_directory_is_an_empty_list() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "GET", SKILLS_PATH);

    assert_eq!(status, 200, "目录不存在等于一个技能也没装");
    assert_eq!(payload, json!({ "skills": [] }));
}

#[test]
fn list_projects_front_matter_and_falls_back_to_the_directory_name() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    write_skill(
        root,
        "alpha-skill",
        "---\nname: 阿尔法技能\ndescription: 单行说明\n---\n技能正文不得出现在清单里\n",
    );
    write_skill(
        root,
        "beta-skill",
        "---\nname: beta-skill\ndescription: |\n  第一行说明\n  第二行说明\n---\n正文\n",
    );
    // 只有目录没有 SKILL.md：名称回退为目录名，说明为空串。
    std::fs::create_dir_all(root.join("skills").join("gamma-skill")).expect("目录必须可创建");
    // 以点开头的目录是运行态数据，不属于技能清单。
    std::fs::create_dir_all(root.join("skills").join(".revisions")).expect("目录必须可创建");

    let (status, payload) = call(&adapter, "GET", SKILLS_PATH);
    assert_eq!(status, 200);
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["skills"], "载荷只允许 skills 一项");

    let skills = payload["skills"].as_array().expect("技能清单是数组");
    assert_eq!(skills.len(), 3, "点目录不得计入：{payload}");

    let ids = skills
        .iter()
        .filter_map(|skill| skill["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["alpha-skill", "beta-skill", "gamma-skill"]);

    assert_eq!(skills[0]["name"], "阿尔法技能");
    assert_eq!(skills[0]["description"], "单行说明");
    assert_eq!(
        skills[0]["path"],
        root.join("skills")
            .join("alpha-skill")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        skills[0]["entry_path"],
        root.join("skills")
            .join("alpha-skill")
            .join("SKILL.md")
            .to_string_lossy()
            .as_ref()
    );

    assert_eq!(
        skills[1]["description"], "第一行说明\n第二行说明",
        "块标量说明必须按正文读取"
    );

    assert_eq!(skills[2]["name"], "gamma-skill", "缺前置块时回退目录名");
    assert_eq!(skills[2]["description"], "");
    assert_eq!(skills[2]["entry_path"], "", "SKILL.md 不存在时为空串");

    let mut skill_keys = skills[0]
        .as_object()
        .expect("技能条目是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    skill_keys.sort_unstable();
    assert_eq!(
        skill_keys,
        vec!["description", "entry_path", "id", "name", "path"],
        "字段必须与桌面端 SkillDto 逐字一致"
    );

    let serialized = payload.to_string();
    assert!(
        !serialized.contains("技能正文不得出现在清单里"),
        "清单只回元数据，不回技能正文"
    );
}

#[test]
fn removal_deletes_only_the_named_directory() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    write_skill(root, "alpha-skill", "---\nname: alpha\n---\n正文\n");
    write_skill(root, "beta-skill", "---\nname: beta\n---\n正文\n");

    let (status, payload) = call(&adapter, "DELETE", "/api/v1/skills/alpha-skill");
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload, json!({ "removed": "alpha-skill" }));
    assert!(
        !root.join("skills").join("alpha-skill").exists(),
        "被移除的技能目录必须真的消失"
    );
    assert!(
        root.join("skills").join("beta-skill").is_dir(),
        "其它技能不得被波及"
    );

    let (status, payload) = call(&adapter, "GET", SKILLS_PATH);
    assert_eq!(status, 200);
    assert_eq!(
        payload["skills"].as_array().map(Vec::len),
        Some(1),
        "清单必须立刻反映移除结果：{payload}"
    );
}

#[test]
fn removal_decodes_the_identifier_before_touching_disk() {
    // 目录名带空格与中文时，客户端按 URL 规则编码后送来，服务端要解回原名再比对。
    let (directory, adapter) = fixture();
    let root = directory.path();
    write_skill(root, "工作区 工具", "---\nname: 工作区工具\n---\n正文\n");

    let (status, payload) = call(
        &adapter,
        "DELETE",
        "/api/v1/skills/%E5%B7%A5%E4%BD%9C%E5%8C%BA%20%E5%B7%A5%E5%85%B7",
    );
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["removed"], "工作区 工具");
    assert!(!root.join("skills").join("工作区 工具").exists());
}

#[test]
fn removal_refuses_identifiers_that_escape_the_skill_root() {
    let (directory, adapter) = fixture();
    let root = directory.path();
    write_skill(root, "alpha-skill", "---\nname: alpha\n---\n正文\n");
    std::fs::create_dir_all(root.join("skills").join(".revisions")).expect("目录必须可创建");
    // 写在技能根之外的文件：任何一次越界删除都会把它带走。
    std::fs::write(root.join("keep.txt"), "不得删除").expect("文件必须可写");

    for path in [
        "/api/v1/skills/.",
        "/api/v1/skills/..",
        "/api/v1/skills/.revisions",
        "/api/v1/skills/%2e%2e",
        "/api/v1/skills/..%2Fkeep.txt",
        "/api/v1/skills/alpha-skill%20",
    ] {
        let (status, payload) = call(&adapter, "DELETE", path);
        assert_eq!(status, 400, "{path}：{payload}");
        assert_eq!(payload["error"], "invalid_skill_id", "{path}");
    }
    assert!(
        root.join("keep.txt").is_file(),
        "技能根之外的文件不得被删除"
    );
    assert!(
        root.join("skills").join(".revisions").is_dir(),
        "点开头的运行态目录不得被删除"
    );
    assert!(
        root.join("skills").join("alpha-skill").is_dir(),
        "合法技能不得被误删"
    );
}

#[test]
fn removal_reports_missing_skills() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "DELETE", "/api/v1/skills/ghost");
    assert_eq!(status, 404);
    assert_eq!(payload["error"], "skill_not_found");
}

#[cfg(unix)]
#[test]
fn removal_refuses_symlinked_skills() {
    // 软链接指向别处：删链接本身看似无害，但顺着删就会删到技能根之外。
    use std::os::unix::fs::symlink;

    let (directory, adapter) = fixture();
    let root = directory.path();
    write_skill(root, "real-skill", "---\nname: real\n---\n正文\n");
    symlink(
        root.join("skills").join("real-skill"),
        root.join("skills").join("link-skill"),
    )
    .expect("软链接必须可创建");

    let (status, payload) = call(&adapter, "DELETE", "/api/v1/skills/link-skill");
    assert_eq!(status, 400, "{payload}");
    assert_eq!(payload["error"], "invalid_skill_id");
    assert!(
        root.join("skills").join("real-skill").is_dir(),
        "软链接背后的真实技能不得被删除"
    );
}

#[test]
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/skill",
        "/api/v1/skills/",
        "/api/v1/skills/alpha-skill/extra",
        "/api/v1/config",
    ] {
        assert!(
            adapter.handle_request("DELETE", path, None, &[]).is_none(),
            "{path} 不属于技能族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405_method_not_allowed() {
    let (_directory, adapter) = fixture();
    for (method, path) in [
        ("POST", SKILLS_PATH),
        ("PUT", SKILLS_PATH),
        ("DELETE", SKILLS_PATH),
        ("GET", "/api/v1/skills/alpha-skill"),
        ("POST", "/api/v1/skills/alpha-skill"),
    ] {
        let (status, payload) = call(&adapter, method, path);
        assert_eq!(status, 405, "{method} {path}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}
