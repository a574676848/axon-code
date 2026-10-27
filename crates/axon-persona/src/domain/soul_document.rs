//! `SOUL.md` 的解析与校验。
//!
//! 格式为必填 YAML frontmatter 加可选 Markdown 正文，`schema` 必须等于
//! [`SOUL_SCHEMA_V1`]。旧格式不做兼容解析：缺少 frontmatter 即判为无效。

use std::fmt;

use yaml_rust::{Yaml, YamlLoader};

use crate::{
    constants::{SOUL_DOCUMENT_MAX_BYTES, SOUL_SCHEMA_V1},
    domain::{
        persona_core::{Identity, PersonaCore, RuleCondition, SceneRule},
        scene_frame::{Audience, PersonaMode},
        voice::{HumorLevel, Verbosity, Voice, VoicePatch},
    },
};

/// `SOUL.md` 校验失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoulError {
    /// 文档超过字节上限。
    TooLarge {
        /// 实际字节数。
        bytes: usize,
    },
    /// 缺少 frontmatter。
    MissingFrontmatter,
    /// frontmatter 未闭合。
    UnterminatedFrontmatter,
    /// YAML 语法错误，行号为一基。
    Syntax {
        /// 一基行号。
        line: usize,
        /// 原始诊断。
        message: String,
    },
    /// 字段校验失败。
    Invalid {
        /// 字段路径。
        field: String,
        /// 中文原因。
        message: String,
    },
}

impl fmt::Display for SoulError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(
                formatter,
                "SOUL.md 超过上限：{bytes} 字节，上限 {SOUL_DOCUMENT_MAX_BYTES} 字节"
            ),
            Self::MissingFrontmatter => write!(
                formatter,
                "SOUL.md 缺少 frontmatter：首行必须是 ---，格式版本 {SOUL_SCHEMA_V1}"
            ),
            Self::UnterminatedFrontmatter => {
                write!(formatter, "SOUL.md 的 frontmatter 未闭合：缺少结束的 ---")
            }
            Self::Syntax { line, message } => {
                write!(formatter, "SOUL.md 第 {line} 行 YAML 语法错误：{message}")
            }
            Self::Invalid { field, message } => {
                write!(formatter, "SOUL.md 字段 {field} 无效：{message}")
            }
        }
    }
}

impl std::error::Error for SoulError {}

/// 解析成功后的 `SOUL.md`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoulDocument {
    /// 格式标识。
    pub schema: String,
    /// 人格内核。
    pub core: PersonaCore,
    /// 正文补充说明。
    pub body: Option<String>,
}

impl SoulDocument {
    /// 解析并校验 `SOUL.md` 全文。
    ///
    /// # Errors
    ///
    /// 超过字节上限、缺少或未闭合 frontmatter、YAML 语法错误，或任一字段不合法时返回
    /// [`SoulError`]。
    pub fn parse(raw: &str) -> Result<Self, SoulError> {
        if raw.len() > SOUL_DOCUMENT_MAX_BYTES {
            return Err(SoulError::TooLarge { bytes: raw.len() });
        }
        let normalized = raw
            .trim_start_matches('\u{feff}')
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let mut lines = normalized.split('\n');
        let first = lines.next().unwrap_or_default();
        if first.trim() != "---" {
            return Err(SoulError::MissingFrontmatter);
        }
        let rest = lines.collect::<Vec<_>>();
        // 结束行必须是顶格的 ---；带前导空白的一律视为正文内容，避免把块标量里的分隔线当成结束符。
        let Some(close) = rest.iter().position(|line| *line == "---") else {
            return Err(SoulError::UnterminatedFrontmatter);
        };
        let yaml_text = rest[..close].join("\n");
        let body_text = rest[close + 1..].join("\n");
        let documents =
            YamlLoader::load_from_str(&yaml_text).map_err(|error| SoulError::Syntax {
                line: error.marker().line() + 2,
                message: error.to_string(),
            })?;
        let root = documents.first().ok_or(SoulError::MissingFrontmatter)?;
        let parsed = parse_root(root)?;
        let body = body_text.trim();
        Ok(Self {
            schema: SOUL_SCHEMA_V1.to_owned(),
            core: PersonaCore {
                notes: (!body.is_empty()).then(|| body.to_owned()),
                ..parsed
            },
            body: (!body.is_empty()).then(|| body.to_owned()),
        })
    }
}

const ROOT_KEYS: [&str; 7] = [
    "schema",
    "identity",
    "backstory",
    "invariants",
    "traits",
    "voice",
    "rules",
];

fn parse_root(root: &Yaml) -> Result<PersonaCore, SoulError> {
    let hash = as_hash(root, "根节点")?;
    reject_unknown_keys(hash, &ROOT_KEYS, "根节点")?;
    let schema = required_str(hash, "schema", "schema")?;
    if schema != SOUL_SCHEMA_V1 {
        return Err(SoulError::Invalid {
            field: "schema".to_owned(),
            message: format!("格式版本不符（{schema}），需升级到 {SOUL_SCHEMA_V1}"),
        });
    }
    Ok(PersonaCore {
        identity: parse_identity(hash)?,
        backstory: required_str(hash, "backstory", "backstory")?,
        invariants: required_str_list(hash, "invariants")?,
        traits: required_str_list(hash, "traits")?,
        voice: parse_voice(hash)?,
        rules: parse_rules(hash)?,
        notes: None,
    })
}

fn parse_identity(hash: &Hash) -> Result<Identity, SoulError> {
    let identity = as_hash(
        get(hash, "identity").ok_or_else(|| invalid("identity", "缺少该字段"))?,
        "identity",
    )?;
    reject_unknown_keys(identity, &["name", "address", "role"], "identity")?;
    Ok(Identity {
        name: required_str(identity, "identity.name", "name")?,
        address: required_str(identity, "identity.address", "address")?,
        role: required_str(identity, "identity.role", "role")?,
    })
}

fn parse_voice(hash: &Hash) -> Result<Voice, SoulError> {
    let voice = as_hash(
        get(hash, "voice").ok_or_else(|| invalid("voice", "缺少该字段"))?,
        "voice",
    )?;
    reject_unknown_keys(
        voice,
        &["warmth", "formality", "humor", "verbosity"],
        "voice",
    )?;
    let humor_token = required_str(voice, "voice.humor", "humor")?;
    let humor =
        parse_humor(&humor_token).ok_or_else(|| invalid("voice.humor", "取值不在允许范围内"))?;
    let verbosity_token = required_str(voice, "voice.verbosity", "verbosity")?;
    let verbosity = parse_verbosity(&verbosity_token)
        .ok_or_else(|| invalid("voice.verbosity", "取值不在允许范围内"))?;
    Ok(Voice::new(
        required_percent(voice, "voice.warmth", "warmth")?,
        required_percent(voice, "voice.formality", "formality")?,
        humor,
        verbosity,
    ))
}

fn parse_rules(hash: &Hash) -> Result<Vec<SceneRule>, SoulError> {
    let Some(value) = get(hash, "rules") else {
        return Ok(Vec::new());
    };
    let items = value
        .as_vec()
        .ok_or_else(|| invalid("rules", "必须是列表"))?;
    items
        .iter()
        .enumerate()
        .map(|(index, item)| parse_rule(item, index))
        .collect()
}

fn parse_rule(item: &Yaml, index: usize) -> Result<SceneRule, SoulError> {
    let path = format!("rules[{index}]");
    let rule = as_hash(item, &path)?;
    reject_unknown_keys(rule, &["when", "voice"], &path)?;
    let when_path = format!("{path}.when");
    let when = as_hash(
        get(rule, "when").ok_or_else(|| invalid(&when_path, "缺少该字段"))?,
        &when_path,
    )?;
    reject_unknown_keys(when, &["mode", "audience", "user_affect"], &when_path)?;
    let condition = RuleCondition {
        mode: optional_parse(when, "mode", &when_path, parse_mode)?,
        audience: optional_parse(when, "audience", &when_path, parse_audience)?,
        user_affect: optional_parse(when, "user_affect", &when_path, parse_user_affect)?,
    };
    if condition.is_empty() {
        return Err(invalid(
            &when_path,
            "必须至少声明 mode、audience 或 user_affect",
        ));
    }
    let voice_path = format!("{path}.voice");
    let voice = as_hash(
        get(rule, "voice").ok_or_else(|| invalid(&voice_path, "缺少该字段"))?,
        &voice_path,
    )?;
    reject_unknown_keys(
        voice,
        &["warmth", "formality", "humor", "verbosity"],
        &voice_path,
    )?;
    let patch = VoicePatch {
        warmth: optional_percent(voice, "warmth", &format!("{voice_path}.warmth"))?,
        formality: optional_percent(voice, "formality", &format!("{voice_path}.formality"))?,
        humor: optional_parse(voice, "humor", &voice_path, parse_humor)?,
        verbosity: optional_parse(voice, "verbosity", &voice_path, parse_verbosity)?,
    };
    if patch.is_empty() {
        return Err(invalid(&voice_path, "必须至少覆盖一个表达字段"));
    }
    Ok(SceneRule {
        when: condition,
        voice: patch,
    })
}

type Hash = yaml_rust::yaml::Hash;

fn as_hash<'a>(value: &'a Yaml, path: &str) -> Result<&'a Hash, SoulError> {
    value
        .as_hash()
        .ok_or_else(|| invalid(path, "必须是映射（键值对）"))
}

fn get<'a>(hash: &'a Hash, key: &str) -> Option<&'a Yaml> {
    hash.get(&Yaml::String(key.to_owned()))
}

fn reject_unknown_keys(hash: &Hash, allowed: &[&str], path: &str) -> Result<(), SoulError> {
    for (key, _) in hash {
        let Some(name) = key.as_str() else {
            return Err(invalid(path, "键必须是字符串"));
        };
        if !allowed.contains(&name) {
            return Err(invalid(
                &format!("{path}.{name}"),
                "不支持的字段，禁止静默忽略拼写错误",
            ));
        }
    }
    Ok(())
}

fn required_str(hash: &Hash, path: &str, key: &str) -> Result<String, SoulError> {
    let value = get(hash, key).ok_or_else(|| invalid(path, "缺少该字段"))?;
    let text = value
        .as_str()
        .ok_or_else(|| invalid(path, "必须是字符串"))?
        .trim();
    if text.is_empty() {
        return Err(invalid(path, "不能为空"));
    }
    Ok(text.to_owned())
}

fn required_str_list(hash: &Hash, key: &str) -> Result<Vec<String>, SoulError> {
    let path = key;
    let items = get(hash, key)
        .ok_or_else(|| invalid(path, "缺少该字段"))?
        .as_vec()
        .ok_or_else(|| invalid(path, "必须是字符串列表"))?;
    if items.is_empty() {
        return Err(invalid(path, "至少需要一项"));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let text = item
                .as_str()
                .ok_or_else(|| invalid(&format!("{path}[{index}]"), "必须是字符串"))?
                .trim();
            if text.is_empty() {
                return Err(invalid(&format!("{path}[{index}]"), "不能为空"));
            }
            Ok(text.to_owned())
        })
        .collect()
}

fn required_percent(hash: &Hash, path: &str, key: &str) -> Result<u8, SoulError> {
    let value = get(hash, key).ok_or_else(|| invalid(path, "缺少该字段"))?;
    let number = value.as_i64().ok_or_else(|| invalid(path, "必须是整数"))?;
    u8::try_from(number)
        .ok()
        .filter(|value| *value <= 100)
        .ok_or_else(|| invalid(path, "必须在 0 至 100 之间"))
}

fn optional_percent(hash: &Hash, key: &str, path: &str) -> Result<Option<u8>, SoulError> {
    let Some(value) = get(hash, key) else {
        return Ok(None);
    };
    let number = value.as_i64().ok_or_else(|| invalid(path, "必须是整数"))?;
    u8::try_from(number)
        .ok()
        .filter(|value| *value <= 100)
        .map(Some)
        .ok_or_else(|| invalid(path, "必须在 0 至 100 之间"))
}

fn optional_parse<T>(
    hash: &Hash,
    key: &str,
    path: &str,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<T>, SoulError> {
    let Some(value) = get(hash, key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| invalid(&format!("{path}.{key}"), "必须是字符串"))?;
    parse(text)
        .map(Some)
        .ok_or_else(|| invalid(&format!("{path}.{key}"), "取值不在允许范围内"))
}

fn parse_user_affect(token: &str) -> Option<crate::domain::persona_state::UserAffect> {
    use crate::domain::persona_state::UserAffect;

    match token.trim() {
        "neutral" => Some(UserAffect::Neutral),
        "positive" => Some(UserAffect::Positive),
        "frustrated" => Some(UserAffect::Frustrated),
        "urgent" => Some(UserAffect::Urgent),
        _ => None,
    }
}

fn parse_mode(token: &str) -> Option<PersonaMode> {
    match token.trim() {
        "focus" => Some(PersonaMode::Focus),
        "casual" => Some(PersonaMode::Casual),
        "incident" => Some(PersonaMode::Incident),
        "group" => Some(PersonaMode::Group),
        _ => None,
    }
}

fn parse_audience(token: &str) -> Option<Audience> {
    match token.trim() {
        "private" => Some(Audience::Private),
        "group" => Some(Audience::Group),
        _ => None,
    }
}

fn parse_humor(token: &str) -> Option<HumorLevel> {
    HumorLevel::parse(token)
}

fn parse_verbosity(token: &str) -> Option<Verbosity> {
    Verbosity::parse(token)
}

fn invalid(field: &str, message: &str) -> SoulError {
    SoulError::Invalid {
        field: field.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[path = "soul_document_tests.rs"]
mod tests;
