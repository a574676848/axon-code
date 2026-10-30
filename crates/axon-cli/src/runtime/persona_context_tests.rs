//! 人格上下文装配的边界用例。

use super::*;
use axon_persona::domain::scene_frame::Channel;

/// 结构类用例的装配预算：永不降级，断言只观测结构，不受真实耗时与机器负载影响。
const UNBOUNDED_BUDGET_MS: u64 = u64::MAX;
/// 超预算用例的装配预算：任何耗时都算超限，把「超预算」变成确定输入而非判断依据。
const ZERO_BUDGET_MS: u64 = 0;

fn scene() -> PersonaSceneInput {
    PersonaSceneInput {
        channel: Channel::Cli,
        audience: Audience::Private,
        has_workspace: true,
    }
}

#[test]
fn missing_soul_yields_empty_context_without_warning() {
    let root = axon_test_support::test_temp_dir();
    let started = Instant::now();

    let context = load_persona_context_from_root(root.path(), scene(), started, UNBOUNDED_BUDGET_MS)
        .expect("缺失文件不应报错");

    assert!(context.stable.is_none());
    assert!(context.dynamic.is_none());
    assert!(context.warnings.is_empty());
    assert!(!context.degraded);
}

#[test]
fn invalid_soul_degrades_with_warning_but_keeps_turn_alive() {
    let root = axon_test_support::test_temp_dir();
    fs::write(
        root.path().join(USER_SOUL_FILE_NAME),
        "[SOUL_DEFINITION]\n- 旧格式\n",
    )
    .unwrap();

    let context = load_persona_context_from_root(root.path(), scene(), Instant::now(), UNBOUNDED_BUDGET_MS)
        .expect("无效人设不应报错");

    assert!(context.stable.is_none());
    assert!(context.dynamic.is_none());
    assert_eq!(context.warnings.len(), 1);
    assert!(context.warnings[0].contains("人设未装配"));
    assert!(!context.degraded);
}

#[test]
fn valid_soul_produces_stable_and_dynamic_messages() {
    let root = axon_test_support::test_temp_dir();
    fs::write(root.path().join(USER_SOUL_FILE_NAME), VALID_SOUL).unwrap();

    let context = load_persona_context_from_root(root.path(), scene(), Instant::now(), UNBOUNDED_BUDGET_MS)
        .expect("合法人设应装配");

    let stable = context.stable.expect("应有稳定段");
    assert!(stable.content.starts_with(SESSION_STABLE_SOUL_START_MARKER));
    assert!(stable.content.ends_with(SESSION_STABLE_SOUL_END_MARKER));
    assert!(!stable.cache_excluded);
    assert!(stable.content.contains("[INVARIANTS]"));

    let dynamic = context.dynamic.expect("应有动态段");
    assert!(dynamic.content.starts_with(PERSONA_STATE_START_MARKER));
    assert!(dynamic.content.ends_with(PERSONA_STATE_END_MARKER));
    assert!(dynamic.cache_excluded, "动态段不得进入缓存");
    assert!(dynamic.content.contains("mode:"));
}

#[test]
fn stable_bytes_are_unchanged_across_loads() {
    let root = axon_test_support::test_temp_dir();
    fs::write(root.path().join(USER_SOUL_FILE_NAME), VALID_SOUL).unwrap();

    let first = load_persona_context_from_root(root.path(), scene(), Instant::now(), UNBOUNDED_BUDGET_MS).unwrap();
    let second = load_persona_context_from_root(root.path(), scene(), Instant::now(), UNBOUNDED_BUDGET_MS).unwrap();

    assert_eq!(
        first.stable.unwrap().content,
        second.stable.unwrap().content
    );
}

const VALID_SOUL: &str = "---\nschema: axon-soul/v1\nidentity:\n  name: Axon\n  address: 长官\n  role: 副官\nbackstory: 来自工程一线。\ninvariants:\n  - 依据事实作答\ntraits: [沉稳]\nvoice:\n  warmth: 40\n  formality: 60\n  humor: subtle\n  verbosity: concise\n---\n输出习惯：结论先行。\n";

#[test]
fn assembly_over_budget_degrades_and_keeps_the_turn_alive() {
    let root = axon_test_support::test_temp_dir();
    fs::write(root.path().join(USER_SOUL_FILE_NAME), VALID_SOUL).unwrap();
    // 零预算等价于「装配一定超限」：把超预算做成确定输入，而不是靠回拨单调时钟
    // 去赌真实装配耗时能压住 50 ms 预算（INV-12 的负向验证）。
    let context =
        load_persona_context_from_root(root.path(), scene(), Instant::now(), ZERO_BUDGET_MS)
            .expect("降级不应报错");

    assert!(context.degraded, "零预算必须置降级");
    assert!(context.stable.is_some(), "稳定段照常装配");
    assert!(context.dynamic.is_none(), "降级时放弃动态段");
    assert!(
        context
            .warnings
            .iter()
            .any(|warning| warning.contains("降级")),
        "降级必须留下可见告警：{:?}",
        context.warnings
    );
}

#[test]
fn oversized_stable_segment_raises_a_visible_warning() {
    let root = axon_test_support::test_temp_dir();
    // 不可违背项按合同不得截断，因此只要它本身超限，稳定段必然超限。
    let oversized = VALID_SOUL.replace(
        "  - 依据事实作答",
        &format!("  - {}", "不可违背项".repeat(2_000)),
    );
    fs::write(root.path().join(USER_SOUL_FILE_NAME), oversized).unwrap();

    let context =
        load_persona_context_from_root(root.path(), scene(), Instant::now(), UNBOUNDED_BUDGET_MS).expect("应装配");

    assert!(
        context
            .warnings
            .iter()
            .any(|warning| warning.contains("稳定段")),
        "超限必须有告警：{:?}",
        context.warnings
    );
}

#[test]
fn unreadable_soul_degrades_to_a_warning_instead_of_failing_the_turn() {
    let root = axon_test_support::test_temp_dir();
    // 用目录顶替文件：读取必然失败，等价于权限或编码故障。
    fs::create_dir(root.path().join(USER_SOUL_FILE_NAME)).expect("创建同名目录");

    let context = load_persona_context_from_root(root.path(), scene(), Instant::now(), UNBOUNDED_BUDGET_MS)
        .expect("读取失败不得让回合失败");

    assert!(context.stable.is_none(), "读不到人设就不装配人格");
    assert!(
        context
            .warnings
            .iter()
            .any(|warning| warning.contains("读取")),
        "必须留下可见告警：{:?}",
        context.warnings
    );
}
