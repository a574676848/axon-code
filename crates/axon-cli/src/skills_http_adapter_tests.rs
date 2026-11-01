//! 技能清单路由测试：清单形状、前置块两种写法、缺失 SKILL.md 的兜底、
//! 正文不外泄、方法边界与未命中回落。

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
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/skill",
        "/api/v1/skills/alpha-skill",
        "/api/v1/config",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于技能族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405_method_not_allowed() {
    let (_directory, adapter) = fixture();
    for method in ["POST", "PUT", "DELETE"] {
        let (status, payload) = call(&adapter, method, SKILLS_PATH);
        assert_eq!(status, 405, "{method}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}
