//! `axon persona` 命令：只读校验与状态查询。

use std::path::Path;

use axon_persona::{
    SoulDocument,
    infrastructure::{PERSONA_SCHEMA_IDENTITY, PersonaStore, persona_store_path},
};
use clap::{Args, Subcommand};

use crate::{
    CliRunResult,
    management_commands::{cli_error, resolved_config_path},
};

/// `axon persona` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct PersonaCommand {
    /// persona 子命令。
    #[command(subcommand)]
    pub command: Option<PersonaSubcommand>,
}

/// `axon persona` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum PersonaSubcommand {
    /// 校验 SOUL.md 是否符合 axon-soul/v1。
    Validate(ValidateArgs),
    /// 展示人格库中的持久化状态。
    Status(StatusArgs),
    /// 把人格状态重置为基线；不改动人设文件与习得风格。
    Reset(ResetArgs),
    /// 管理已发布与待发布的习得风格。
    Style(StyleArgs),
}

/// `axon persona validate` 参数。
#[derive(Debug, Clone, Args, Default, PartialEq, Eq)]
pub struct ValidateArgs {
    /// 校验指定文件而不是当前用户的 SOUL.md。
    #[arg(long)]
    pub file: Option<std::path::PathBuf>,
}

/// `axon persona status` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct StatusArgs {
    /// 以 JSON 输出。
    #[arg(long)]
    pub json: bool,
}

/// `axon persona style` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct StyleArgs {
    /// style 子命令。
    #[command(subcommand)]
    pub command: StyleSubcommand,
}

/// `axon persona style` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum StyleSubcommand {
    /// 列出习得风格。
    List(ListArgs),
    /// 退役一条习得风格，立即生效。
    Retire(RetireArgs),
}

/// `axon persona style list` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ListArgs {
    /// 连同待发布与已退役条目一起列出。
    #[arg(long)]
    pub all: bool,
}

/// `axon persona style retire` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct RetireArgs {
    /// 条目标识。
    pub style_id: String,
}

/// `axon persona reset` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ResetArgs {
    /// 确认执行；缺少该标志时只打印提示，不写库。
    #[arg(long)]
    pub yes: bool,
}

const SOUL_FILE_NAME: &str = "SOUL.md";

fn reset(root: &Path, confirmed: bool) -> CliRunResult {
    if !confirmed {
        return CliRunResult {
            exit_code: 0,
            stdout: "将把人格状态重置为基线（不改动人设文件与习得风格）。确认执行请加 --yes。\n"
                .to_owned(),
            stderr: String::new(),
        };
    }
    let path = persona_store_path(root);
    // 先确认 owner，再打开库：无 owner 时不得凭空建出一个空人格库。
    let owner = crate::runtime_principal_context::load_run_principal(root, "cli")
        .ok()
        .flatten()
        .map(|principal| principal.owner_id.as_str().to_owned());
    let Some(owner) = owner else {
        return cli_error("本机尚无 owner 身份，无可重置的人格状态".to_owned());
    };
    // 重置要写状态，必须用可写句柄。
    let mut store = match PersonaStore::open(&path) {
        Ok(store) => store,
        Err(error) => return cli_error(format!("人格库不可用：{}", storage_message(&error))),
    };
    let now_ms = axon_time::unix_now_ms();
    let event = axon_persona::PersonaEvent::Reset {
        reason: "用户手动重置".to_owned(),
        now_ms,
    };
    let key = format!("reset:{now_ms}");
    match store.settle(
        &owner,
        &event,
        axon_persona::domain::scene_frame::Audience::Private,
        &key,
    ) {
        Ok(outcome) => {
            let detail = match outcome {
                axon_persona::infrastructure::SettlementOutcome::Applied { state, cause } => {
                    format!("状态版本 {}，原因：{cause}", state.revision)
                }
                axon_persona::infrastructure::SettlementOutcome::AlreadyApplied => {
                    "该次重置此前已生效".to_owned()
                }
                axon_persona::infrastructure::SettlementOutcome::Unchanged => {
                    "状态本就在基线，无需变更".to_owned()
                }
            };
            CliRunResult {
                exit_code: 0,
                stdout: format!("人格状态已重置：{detail}\n"),
                stderr: String::new(),
            }
        }
        Err(error) => cli_error(format!("重置失败：{error}")),
    }
}

pub(crate) fn run_persona(command: &PersonaCommand, config_path: Option<&Path>) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let root = match axon_config::runtime_axon_root_path(&path) {
        Ok(root) => root,
        Err(error) => return cli_error(format!("无法解析 Axon 运行根目录：{error:?}")),
    };
    match command
        .command
        .clone()
        .unwrap_or(PersonaSubcommand::Status(StatusArgs { json: false }))
    {
        PersonaSubcommand::Validate(args) => validate(&root, args.file.as_deref()),
        PersonaSubcommand::Status(args) => status(&root, args.json),
        PersonaSubcommand::Reset(args) => reset(&root, args.yes),
        PersonaSubcommand::Style(args) => style(&root, &args.command),
    }
}

fn style(root: &Path, command: &StyleSubcommand) -> CliRunResult {
    let path = persona_store_path(root);
    // 库不存在时不建库：list 给友好提示，retire 明确报错。
    if !path.exists() {
        return match command {
            StyleSubcommand::List(_) => CliRunResult {
                exit_code: 0,
                stdout: "暂无习得风格：人格库尚未创建\n".to_owned(),
                stderr: String::new(),
            },
            StyleSubcommand::Retire(_) => cli_error("人格库尚未创建，无可退役的条目".to_owned()),
        };
    }
    // 先确认 owner：无 owner 时不得因为一次退役而凭空建库。
    let Some(owner) = owner_of(root) else {
        // 只读列表与 status 同口径：没有 owner 就如实说没有，不用退出码惩罚。
        return match command {
            StyleSubcommand::List(_) => CliRunResult {
                exit_code: 0,
                stdout: "暂无习得风格：本机尚无 owner 身份\n".to_owned(),
                stderr: String::new(),
            },
            StyleSubcommand::Retire(_) => cli_error("本机尚无 owner 身份".to_owned()),
        };
    };
    // 列表只读，退役才需要写句柄：只读命令不得建库或跑 DDL。
    let opened = match command {
        StyleSubcommand::List(_) => PersonaStore::open_read_only(&path),
        StyleSubcommand::Retire(_) => PersonaStore::open(&path),
    };
    let mut store = match opened {
        Ok(store) => store,
        Err(_) if !path.exists() => {
            return CliRunResult {
                exit_code: 0,
                stdout: "暂无习得风格：人格库尚未创建\n".to_owned(),
                stderr: String::new(),
            };
        }
        Err(error) => return cli_error(format!("人格库不可用：{}", storage_message(&error))),
    };
    match command {
        StyleSubcommand::List(args) => match store.list_styles(&owner, args.all) {
            Ok(listings) if listings.is_empty() => CliRunResult {
                exit_code: 0,
                stdout: "暂无习得风格\n".to_owned(),
                stderr: String::new(),
            },
            Ok(listings) => {
                let mut stdout = String::new();
                for listing in listings {
                    stdout.push_str(&format!(
                        "{}\t{}\t{}
",
                        listing.style_id, listing.status, listing.text
                    ));
                }
                CliRunResult {
                    exit_code: 0,
                    stdout,
                    stderr: String::new(),
                }
            }
            Err(error) => cli_error(format!("读取习得风格失败：{}", storage_message(&error))),
        },
        StyleSubcommand::Retire(args) => match store.retire_style(&owner, &args.style_id) {
            Ok(true) => CliRunResult {
                exit_code: 0,
                stdout: format!("已退役：{}\n", args.style_id),
                stderr: String::new(),
            },
            Ok(false) => cli_error(format!("未找到可退役的条目：{}", args.style_id)),
            Err(error) => cli_error(format!("退役失败：{error}")),
        },
    }
}

fn owner_of(root: &Path) -> Option<String> {
    crate::runtime_principal_context::load_run_principal(root, "cli")
        .ok()
        .flatten()
        .map(|principal| principal.owner_id.as_str().to_owned())
}

fn validate(root: &Path, file: Option<&Path>) -> CliRunResult {
    let path = file.map_or_else(|| root.join(SOUL_FILE_NAME), Path::to_path_buf);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => {
            return cli_error(format!("读取人设文件失败（{}）：{error}", path.display()));
        }
    };
    match SoulDocument::parse(&raw) {
        Ok(document) => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "人设校验通过：{} · {} 项不可违背项 · {} 项特质 · {} 条场景规则\n",
                document.schema,
                document.core.invariants.len(),
                document.core.traits.len(),
                document.core.rules.len()
            ),
            stderr: String::new(),
        },
        Err(error) => cli_error(format!("人设校验失败：{error}")),
    }
}

/// 存储层错误文案：`Storage` 已自带前缀，避免调用方再加一层。
fn storage_message(error: &axon_persona::infrastructure::PersonaStoreError) -> String {
    match error {
        axon_persona::infrastructure::PersonaStoreError::Storage(message) => {
            message.trim_start_matches("人格库不可用：").to_owned()
        }
        other => other.to_string(),
    }
}

/// 人设内核的读取结论。
enum SoulState {
    /// 文件不存在。
    Missing,
    /// 文件存在但读不出来（权限、编码等）。
    Unreadable(String),
    /// 读到内容：是否通过格式校验、字节数、内容哈希。
    Parsed {
        valid: bool,
        bytes: u64,
        hash: String,
    },
}

fn soul_status(root: &Path) -> SoulState {
    use sha2::{Digest, Sha256};

    let path = root.join(SOUL_FILE_NAME);
    if !path.exists() {
        return SoulState::Missing;
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => return SoulState::Unreadable(error.to_string()),
    };
    let bytes = u64::try_from(raw.len()).unwrap_or(u64::MAX);
    let hash = hex::encode(Sha256::digest(raw.as_bytes()));
    SoulState::Parsed {
        valid: SoulDocument::parse(&raw).is_ok(),
        bytes,
        hash,
    }
}

/// 内核的机读形态：三态都保有稳定字段。
fn soul_json(soul: &SoulState) -> serde_json::Value {
    match soul {
        SoulState::Missing => serde_json::json!({"state": "missing"}),
        SoulState::Unreadable(reason) => {
            serde_json::json!({"state": "unreadable", "error": reason})
        }
        SoulState::Parsed { valid, bytes, hash } => serde_json::json!({
            "state": if *valid { "valid" } else { "invalid" },
            "valid": valid,
            "bytes": bytes,
            "hash": hash,
        }),
    }
}

/// `--json` 下查询失败的统一出口：结构不变，只有对应段为空并附错误。
fn status_json_error(soul: &SoulState, section: &str, error: &str) -> CliRunResult {
    let value = serde_json::json!({
        "schema": PERSONA_SCHEMA_IDENTITY,
        "soul": soul_json(soul),
        "state": serde_json::Value::Null,
        "style": {"revision": 0, "published": 0},
        "transitions": [],
        "database": {"ready": true, "error": error},
        "failedSection": section,
    });
    CliRunResult {
        exit_code: 2,
        stdout: format!("{value}\n"),
        stderr: String::new(),
    }
}

/// 人读的内核一行：三种状态各给一句明确结论。
fn soul_line(soul: &SoulState) -> String {
    match soul {
        SoulState::Missing => "未找到 SOUL.md".to_owned(),
        SoulState::Unreadable(reason) => format!("读取失败（{reason}）"),
        SoulState::Parsed {
            valid: true,
            bytes,
            hash,
        } => {
            format!("有效（{bytes} B，hash {}）", &hash[..12.min(hash.len())])
        }
        SoulState::Parsed {
            valid: false,
            bytes,
            hash,
        } => {
            format!("无效（{bytes} B，hash {}）", &hash[..12.min(hash.len())])
        }
    }
}

fn status(root: &Path, json: bool) -> CliRunResult {
    let soul = soul_status(root);
    let soul_valid = matches!(soul, SoulState::Parsed { valid: true, .. });
    let soul_bytes = match &soul {
        SoulState::Parsed { bytes, .. } => Some(*bytes),
        _ => None,
    };
    let soul_hash = match &soul {
        SoulState::Parsed { hash, .. } => Some(hash.clone()),
        _ => None,
    };
    let path = persona_store_path(root);
    if !path.exists() {
        // 机器可读契约不因库缺失而失效：--json 始终输出 JSON。
        let stdout = if json {
            format!(
                "{}\n",
                serde_json::json!({
                    "schema": PERSONA_SCHEMA_IDENTITY,
                    "state": serde_json::Value::Null,
                    "style": {"revision": 0, "published": 0},
                    "transitions": [],
                    "database": {"ready": false},
                    "soul": soul_json(&soul),
                })
            )
        } else {
            format!(
                "内核：{}\n人格状态：尚未创建，完成一轮私聊对话后由轮后结算建立\n",
                soul_line(&soul)
            )
        };
        return CliRunResult {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        };
    }
    let store = match PersonaStore::open_read_only(&path) {
        Ok(store) => store,
        Err(error) if json => {
            return CliRunResult {
                exit_code: 2,
                stdout: format!(
                    "{}\n",
                    serde_json::json!({
                        "schema": PERSONA_SCHEMA_IDENTITY,
                        "soul": soul_json(&soul),
                        "state": serde_json::Value::Null,
                        "style": {"revision": 0, "published": 0},
                        "transitions": [],
                        "database": {"ready": false, "error": error.to_string()},
                    })
                ),
                stderr: String::new(),
            };
        }
        Err(error) => return cli_error(format!("人格库不可用：{}", storage_message(&error))),
    };
    let owner = crate::runtime_principal_context::load_run_principal(root, "cli")
        .ok()
        .flatten()
        .map(|principal| principal.owner_id.as_str().to_owned());
    let Some(owner) = owner else {
        // 机器契约不因缺 owner 而失效：--json 始终输出结构化读数。
        let stdout = if json {
            format!(
                "{}\n",
                serde_json::json!({
                    "schema": PERSONA_SCHEMA_IDENTITY,
                    "state": serde_json::Value::Null,
                    "style": {"revision": 0, "published": 0},
                    "transitions": [],
                    "database": {"ready": true},
                    "soul": soul_json(&soul),
                })
            )
        } else {
            format!(
                "内核：{}\n人格状态：库已就绪（{PERSONA_SCHEMA_IDENTITY}），但本机尚无 owner 身份\n",
                soul_line(&soul)
            )
        };
        return CliRunResult {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        };
    };
    let mut stdout = format!("人格库：{PERSONA_SCHEMA_IDENTITY}\n");
    match (&soul_hash, soul_valid) {
        (Some(hash), valid) => stdout.push_str(&format!(
            "内核：{}（{} B，hash {}）\n",
            if valid { "有效" } else { "无效" },
            soul_bytes.unwrap_or(0),
            &hash[..12.min(hash.len())]
        )),
        (None, _) => stdout.push_str("内核：未找到 SOUL.md\n"),
    }
    let state = match store.load_state(&owner) {
        Ok(state) => state,
        Err(error) if json => {
            return status_json_error(&soul, "state", &storage_message(&error));
        }
        Err(error) => return cli_error(format!("读取人格状态失败：{}", storage_message(&error))),
    };
    let published = match store.published_style(&owner) {
        Ok(published) => published,
        Err(error) if json => {
            return status_json_error(&soul, "style", &storage_message(&error));
        }
        Err(error) => return cli_error(format!("读取习得风格失败：{}", storage_message(&error))),
    };
    let transitions = store.recent_transitions(&owner, 10).unwrap_or_default();
    if json {
        let payload = serde_json::json!({
            "schema": PERSONA_SCHEMA_IDENTITY,
            "soul": soul_json(&soul),
            "state": state.as_ref().map(|state| serde_json::json!({
                "revision": state.revision,
                "warmthOffset": state.warmth_offset,
                "familiarity": state.familiarity,
                "userAffect": state.user_affect.as_token(),
                "fatigueLevel": state.fatigue_level.as_token(),
                "failureStreak": state.failure_streak,
                "lastCause": state.last_cause,
                "driftGuard": state.drift_guard,
                "updatedAtMs": state.updated_at_ms,
            })),
            "style": {
                "revision": published.revision,
                "published": published.entries.len(),
            },
            "database": {"ready": true},
            "transitions": transitions
                .iter()
                .map(|record| serde_json::json!({
                    "revision": record.revision,
                    "cause": record.cause,
                    "occurredAtMs": record.occurred_at_ms,
                }))
                .collect::<Vec<_>>(),
        });
        return CliRunResult {
            exit_code: 0,
            stdout: format!("{payload}\n"),
            stderr: String::new(),
        };
    }
    match &state {
        Some(state) => stdout.push_str(&format!(
            "状态版本：{}\n熟悉度：{}\n温度偏移：{}\n用户状态：{}\n疲劳度：{}\n连续失败：{}\n最近变化：{}\n",
            state.revision,
            state.familiarity,
            state.warmth_offset,
            state.user_affect.as_token(),
            state.fatigue_level.as_token(),
            state.failure_streak,
            state.last_cause
        )),
        None => stdout.push_str("状态：尚未产生\n"),
    }
    stdout.push_str(&format!(
        "已发布习得风格：{} 条（发布版本 {}）\n",
        published.entries.len(),
        published.revision
    ));
    if !transitions.is_empty() {
        stdout.push_str("最近转移：\n");
        for record in transitions.iter().take(10) {
            stdout.push_str(&format!("  v{} {}\n", record.revision, record.cause));
        }
    }
    CliRunResult {
        exit_code: 0,
        stdout,
        stderr: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> axon_test_support::TestTempDir {
        axon_test_support::test_temp_dir()
    }

    #[test]
    fn status_json_is_parseable_when_library_is_absent() {
        let root = temp_root();

        let result = status(root.path(), true);

        assert_eq!(result.exit_code, 0);
        let value: serde_json::Value =
            serde_json::from_str(result.stdout.trim()).expect("必须是合法 JSON");
        assert_eq!(value["database"]["ready"], serde_json::json!(false));
        assert!(value.get("soul").is_some());
    }

    #[test]
    fn status_json_is_parseable_when_library_is_corrupt() {
        let root = temp_root();
        let path = persona_store_path(root.path());
        std::fs::create_dir_all(path.parent().expect("父目录")).expect("创建目录");
        std::fs::write(&path, b"not a persona database").expect("写入损坏库");

        let result = status(root.path(), true);

        assert_eq!(result.exit_code, 2, "库损坏必须给非零退出码");
        let value: serde_json::Value =
            serde_json::from_str(result.stdout.trim()).expect("损坏库也必须输出合法 JSON");
        assert_eq!(value["database"]["ready"], serde_json::json!(false));
    }

    #[test]
    fn status_text_reports_soul_state() {
        let root = temp_root();
        std::fs::write(
            root.path().join(SOUL_FILE_NAME),
            "---\nschema: axon-soul/v1\nidentity:\n  name: Axon\n  address: 长官\n  role: 副官\nbackstory: 来处。\ninvariants:\n  - 依据事实作答\ntraits: [沉稳]\nvoice:\n  warmth: 40\n  formality: 60\n  humor: subtle\n  verbosity: concise\n---\n",
        )
        .expect("写入人设");

        let result = status(root.path(), false);

        assert!(result.stdout.contains("内核：有效"), "{}", result.stdout);
    }
}
