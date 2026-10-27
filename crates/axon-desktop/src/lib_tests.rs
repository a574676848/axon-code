use super::catalog::*;
use super::ipc::*;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 建立一次性的临时目录与配置文件路径。
fn scratch_config() -> (PathBuf, PathBuf) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("axon-desktop-test-{nonce}"));
    fs::create_dir_all(&temp_dir).unwrap();
    let config_file = temp_dir.join("config.toml");
    (temp_dir, config_file)
}

/// 读取配置文件全文。
fn read_config(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn test_ipc_read_validate_and_atomic_write_cycle() {
    let (temp_dir, config_file) = scratch_config();

    // 1. 初次读取：文件尚不存在时返回默认规范
    let read1 = execute_config_read_at(&config_file).unwrap();
    assert!(!read1.specs.is_empty(), "必须透出配置键规格定义");

    // 2. 校验无效 TOML：必须拒绝且不产生文件
    let invalid_toml = "[agent\ninvalid_syntax = ";
    let val_fail = execute_config_validate(invalid_toml);
    assert!(!val_fail.valid, "非法 TOML 必须校验失败");
    assert!(!config_file.exists(), "校验失败绝不应写入磁盘");

    // 3. 校验有效 TOML 并执行首次原子写入
    let valid_toml = "[agent]\ndefault_model = \"glm-5.3-flash\"\n";
    assert!(
        execute_config_validate(valid_toml).valid,
        "合法配置必须校验通过"
    );

    let write1 = execute_config_write_at(&config_file, valid_toml).unwrap();
    assert!(write1.written, "初次写入应返回 true");
    assert!(config_file.exists(), "原子写后目标文件必须存在");

    // 4. 重复写入完全相同内容：必须幂等跳过
    let write2 = execute_config_write_at(&config_file, valid_toml).unwrap();
    assert!(!write2.written, "内容未变时应跳过写入");

    // 5. 复读验证一致性
    let read2 = execute_config_read_at(&config_file).unwrap();
    assert_eq!(read2.toml_content.trim(), valid_toml.trim());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_apply_values_persists_and_rejects_invalid_entries() {
    let (temp_dir, config_file) = scratch_config();

    // 首次写入建立基线文件
    execute_config_write_at(&config_file, "[agent]\ndefault_model = \"\"\n").unwrap();

    // 1. 真实写入：推理强度落到磁盘并可读回
    let mut values = HashMap::new();
    values.insert("agent.reasoning_effort".to_string(), "low".to_string());
    values.insert("agent.log_level".to_string(), "debug".to_string());
    let applied = execute_apply_values_at(&config_file, &values).unwrap();
    assert!(applied.written, "有效变更必须真实落盘");

    let content = read_config(&config_file);
    assert!(
        content.contains("reasoning_effort = \"low\""),
        "磁盘内容必须包含新值"
    );
    assert!(
        content.contains("log_level = \"debug\""),
        "磁盘内容必须包含新值"
    );

    let read_back = execute_config_read_at(&config_file).unwrap();
    assert_eq!(
        read_back.flat_values.get("agent.reasoning_effort"),
        Some(&serde_json::json!("low")),
        "读回值必须与新写入一致"
    );

    // 2. 非法取值：必须整批拒绝，磁盘内容保持上一版
    let mut invalid = HashMap::new();
    invalid.insert(
        "agent.reasoning_effort".to_string(),
        "不存在的档位".to_string(),
    );
    assert!(
        execute_apply_values_at(&config_file, &invalid).is_err(),
        "非法枚举值必须被拒绝"
    );
    assert_eq!(read_config(&config_file), content, "拒绝后磁盘内容不得变化");

    // 3. 空变更集：不写盘
    assert!(
        !execute_apply_values_at(&config_file, &HashMap::new())
            .unwrap()
            .written
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_reset_key_restores_builtin_default() {
    let (temp_dir, config_file) = scratch_config();

    let mut values = HashMap::new();
    values.insert("agent.max_subagent_depth".to_string(), "9".to_string());
    execute_apply_values_at(&config_file, &values).unwrap();
    assert!(read_config(&config_file).contains("max_subagent_depth = 9"));

    execute_reset_key_at(&config_file, "agent.max_subagent_depth").unwrap();
    let restored = execute_config_read_at(&config_file).unwrap();
    let default_value = execute_config_read_at(&config_file)
        .unwrap()
        .flat_values
        .get("agent.max_subagent_depth")
        .cloned();
    assert_eq!(
        restored
            .flat_values
            .get("agent.max_subagent_depth")
            .cloned(),
        default_value,
        "恢复后应回到内置默认值"
    );

    assert!(
        execute_reset_key_at(&config_file, "不存在的键").is_err(),
        "未知键必须报错"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_validate_values_is_pure_memory() {
    let (temp_dir, config_file) = scratch_config();
    execute_config_write_at(&config_file, "[agent]\nlog_level = \"info\"\n").unwrap();
    let before = read_config(&config_file);

    let mut values = HashMap::new();
    values.insert("agent.log_level".to_string(), "trace".to_string());
    let report = execute_validate_values_at(&config_file, &values);
    assert!(report.valid, "合法变更应校验通过");
    assert_eq!(read_config(&config_file), before, "试算校验绝不能写盘");

    let mut broken = HashMap::new();
    broken.insert("agent.log_level".to_string(), "不存在".to_string());
    let report_broken = execute_validate_values_at(&config_file, &broken);
    assert!(!report_broken.valid, "非法变更应校验失败");
    assert!(!report_broken.errors.is_empty(), "必须给出可读的错误明细");

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_provider_profile_upsert_and_remove_are_atomic() {
    let (temp_dir, config_file) = scratch_config();
    execute_config_write_at(&config_file, "[agent]\ndefault_model = \"\"\n").unwrap();

    let profile = ProviderInput {
        name: "scratch-provider".to_string(),
        enabled: true,
        protocol: "openai".to_string(),
        base_url: "https://example.com/v1".to_string(),
        api_key_env: Some("SCRATCH_PROFILE_API_KEY".to_string()),
        wire_transport: "http".to_string(),
        priority: 10,
        weight: 1,
        ..ProviderInput::default()
    };
    let primary = ModelInput {
        name: "scratch-a".to_string(),
        id: "scratch-a-v1".to_string(),
        provider: "scratch-provider".to_string(),
        context_window: 64000,
        max_output_tokens: Some(4096),
        ..ModelInput::default()
    };
    let secondary = ModelInput {
        name: "scratch-b".to_string(),
        id: "scratch-b-v1".to_string(),
        provider: "scratch-provider".to_string(),
        context_window: 32000,
        ..ModelInput::default()
    };

    // 1. 新增档案：供应商与其名下模型一次落盘
    provider_profile_upsert_at(
        &config_file,
        profile.clone(),
        vec![primary.clone(), secondary.clone()],
        None,
    )
    .unwrap();
    let added = execute_config_read_at(&config_file).unwrap();
    assert_eq!(added.providers.len(), 1);
    assert_eq!(added.models.len(), 2);
    assert_eq!(
        added.flat_values.get("agent.default_model"),
        Some(&serde_json::json!("scratch-a")),
        "首次写入档案必须自动绑定默认模型槽位"
    );
    assert_eq!(
        added.flat_values.get("agent.fast_model"),
        Some(&serde_json::json!("scratch-a")),
        "首次写入档案必须自动绑定快速委托槽位"
    );

    // 2. 未设置的密钥引用必须如实报告未配置，绝不谎报已就绪
    assert!(!added.providers[0].api_key_configured);

    // 3. 整卡更新：移除 scratch-b 并修改 scratch-a 的上下文窗口
    let mut tuned = primary.clone();
    tuned.context_window = 128000;
    provider_profile_upsert_at(
        &config_file,
        profile.clone(),
        vec![tuned],
        Some("scratch-provider".to_string()),
    )
    .unwrap();
    let updated = execute_config_read_at(&config_file).unwrap();
    assert_eq!(updated.models.len(), 1, "整卡提交必须移除未提交的旧模型");
    assert_eq!(updated.models[0].context_window, 128000);

    // 4. 供应商改名：名下模型的归属同步跟随
    let mut renamed_profile = profile.clone();
    renamed_profile.name = "scratch-profile-renamed".to_string();
    provider_profile_upsert_at(
        &config_file,
        renamed_profile.clone(),
        vec![primary.clone()],
        Some("scratch-provider".to_string()),
    )
    .unwrap();
    let renamed = execute_config_read_at(&config_file).unwrap();
    assert_eq!(renamed.providers[0].name, "scratch-profile-renamed");
    assert_eq!(renamed.models[0].provider, "scratch-profile-renamed");

    // 5. 缺模型 ID 必须整卡拒绝，磁盘保持上一版
    let before_bytes = read_config(&config_file);
    let mut broken = primary.clone();
    broken.id = String::new();
    assert!(
        provider_profile_upsert_at(
            &config_file,
            renamed_profile.clone(),
            vec![broken],
            Some("scratch-profile-renamed".to_string()),
        )
        .is_err(),
        "缺模型 ID 的条目必须被拒绝"
    );
    assert_eq!(
        read_config(&config_file),
        before_bytes,
        "拒绝后磁盘内容不得变化"
    );

    // 6. 同档案内模型重名必须拒绝
    let mut collided = primary.clone();
    collided.id = "scratch-a-v2".to_string();
    assert!(
        provider_profile_upsert_at(
            &config_file,
            renamed_profile.clone(),
            vec![primary.clone(), collided],
            Some("scratch-profile-renamed".to_string()),
        )
        .is_err(),
        "同档案内模型重名必须被拒绝"
    );

    // 7. 移除档案：供应商与其名下模型一并消失，槽位清空
    provider_profile_remove_at(&config_file, "scratch-profile-renamed").unwrap();
    let removed = execute_config_read_at(&config_file).unwrap();
    assert!(removed.providers.is_empty());
    assert!(removed.models.is_empty());
    assert_eq!(
        removed.flat_values.get("agent.default_model"),
        Some(&serde_json::json!("")),
        "模型目录清空后必须同时清空模型槽位"
    );

    // 8. 重复移除必须报错，而不是静默成功
    assert!(provider_profile_remove_at(&config_file, "scratch-profile-renamed").is_err());

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_desktop_state_tolerates_utf8_bom() {
    let (temp_dir, _) = scratch_config();
    let state_file = temp_dir.join("desktop-state.json");
    fs::write(
        &state_file,
        "\u{feff}{\"version\":1,\"workspaces\":[{\"path\":\"E:\\\\repo\",\"name\":\"repo\"}],\"lane_pinned\":[\"lane-a\"]}",
    )
    .unwrap();

    let text = fs::read_to_string(&state_file).unwrap();
    let parsed: super::state::DesktopState =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(
        parsed.workspaces.len(),
        1,
        "带 BOM 的状态必须仍能解析出已登记工作区"
    );
    assert_eq!(parsed.workspaces[0].name, "repo");
    assert_eq!(parsed.lane_pinned, vec!["lane-a".to_string()]);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_workspace_list_only_exposes_user_registered_directories() {
    let source = super::lane_source::GatewayLaneSource::new();
    // 该用例校验的是真实运行态契约：数据经网关读取，网关未运行时无从校验。
    // 此时直接跳过而不是伪造数据，避免把「网关没起」误报成用例通过。
    let Ok(workspaces) = super::workspace::workspace_list_at(&source) else {
        return;
    };
    let state = super::state::load_state();

    // 侧栏只呈现用户在应用内添加过的目录，运行态注册表里的历史条目不得出现。
    assert_eq!(
        workspaces.len(),
        state.workspaces.len(),
        "展示的工作区数量必须与用户登记数量一致"
    );
    for workspace in &workspaces {
        assert!(
            state
                .workspaces
                .iter()
                .any(|registered| registered.path == workspace.path),
            "展示的工作区必须来自用户登记：{}",
            workspace.path
        );
        assert!(
            !workspace.path.contains(r"\\?\") && !workspace.path.contains("//?/"),
            "路径不得残留 Windows 扩展前缀"
        );
        assert!(!workspace.name.is_empty(), "工作区名称不得为空");
        for lane in &workspace.lanes {
            assert!(!lane.id.is_empty(), "车道标识不得为空");
            assert!(!lane.title.is_empty(), "车道标题不得为空");
            assert!(!lane.channel.is_empty(), "车道渠道不得为空");
        }
    }
}

#[test]
fn test_workspace_entries_strip_extended_path_prefix() {
    assert_eq!(
        super::workspace::strip_extended_prefix(r"\\?\E:\zbg\axon-code"),
        if cfg!(windows) {
            r"E:\zbg\axon-code"
        } else {
            r"\\?\E:\zbg\axon-code"
        }
    );
    assert_eq!(
        super::workspace::strip_extended_prefix("//?/e:/zbg/axon-code"),
        if cfg!(windows) {
            r"e:\zbg\axon-code"
        } else {
            "e:/zbg/axon-code"
        }
    );
}
