//! 官方 Skill 目录加载与 frontmatter 解析。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use hex::encode as hex_encode;
use sha2::{Digest, Sha256};
use yaml_rust::YamlLoader;

use super::model::{Skill, SkillLoadError, SkillLoadOutcome, SkillMetadata, SkillRoot};

const SKILL_FILE_NAME: &str = "SKILL.md";
const SKILL_ROOT_READ_FAILED: &str = "Skill 根目录读取失败";
const SKILL_FILE_READ_FAILED: &str = "SKILL.md 读取失败";
const SKILL_PATH_ESCAPE: &str = "Skill 路径超出所属根目录";
const SKILL_FRONTMATTER_INVALID: &str =
    "Skill frontmatter 必须是包含 name 和 description 的 YAML 对象";

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    root: SkillRoot,
}

/// Loads skill documents from configured roots.
#[derive(Debug, Default, Clone, Copy)]
pub struct SkillLoader;

impl SkillLoader {
    /// 检查文档是否具有可加载的 Skill frontmatter。
    #[must_use]
    pub fn has_valid_frontmatter(raw: &str) -> bool {
        let Some((frontmatter, _)) = split_frontmatter(raw) else {
            return false;
        };
        let Ok((name, description)) = parse_manifest(frontmatter) else {
            return false;
        };
        validate_name(&name).is_ok() && !description.trim().is_empty()
    }

    /// Load and de-duplicate skills from roots.
    #[must_use]
    pub fn load_roots(roots: &[SkillRoot]) -> SkillLoadOutcome {
        let mut errors = Vec::new();
        let mut by_name: BTreeMap<String, (u8, Skill)> = BTreeMap::new();

        for root in roots {
            for candidate in candidates(root, &mut errors) {
                match load_candidate(&candidate) {
                    Ok(skill) => {
                        let name = skill.metadata.name.clone();
                        let replace = by_name
                            .get(&name)
                            .is_none_or(|(precedence, _)| candidate.root.precedence >= *precedence);
                        if replace {
                            by_name.insert(name, (candidate.root.precedence, skill));
                        }
                    }
                    Err(error) => errors.push(error),
                }
            }
        }

        SkillLoadOutcome {
            skills: by_name.into_values().map(|(_, skill)| skill).collect(),
            errors,
        }
    }
}

fn candidates(root: &SkillRoot, errors: &mut Vec<SkillLoadError>) -> Vec<Candidate> {
    if !root.path.is_dir() {
        return Vec::new();
    }
    let Ok(root_path) = root.path.canonicalize() else {
        errors.push(load_error(&root.path, SKILL_ROOT_READ_FAILED));
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&root_path) else {
        errors.push(load_error(&root.path, SKILL_ROOT_READ_FAILED));
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            errors.push(load_error(&root.path, SKILL_ROOT_READ_FAILED));
            continue;
        };
        let skill_path = entry.path().join(SKILL_FILE_NAME);
        if !skill_path.is_file() {
            continue;
        }
        let Ok(canonical_path) = skill_path.canonicalize() else {
            errors.push(load_error(&skill_path, SKILL_FILE_READ_FAILED));
            continue;
        };
        if !canonical_path.starts_with(&root_path) {
            errors.push(load_error(&skill_path, SKILL_PATH_ESCAPE));
            continue;
        }
        items.push(Candidate {
            path: canonical_path,
            root: root.clone(),
        });
    }
    items.sort_by(|left, right| left.path.cmp(&right.path));
    items
}

fn load_candidate(candidate: &Candidate) -> Result<Skill, SkillLoadError> {
    let raw = fs::read_to_string(&candidate.path)
        .map_err(|_| load_error(&candidate.path, SKILL_FILE_READ_FAILED))?;
    let (frontmatter, content) = split_frontmatter(&raw)
        .ok_or_else(|| load_error(&candidate.path, SKILL_FRONTMATTER_INVALID))?;
    let (name, description) =
        parse_manifest(frontmatter).map_err(|message| load_error(&candidate.path, &message))?;
    validate_name(&name).map_err(|message| load_error(&candidate.path, message))?;
    if description.trim().is_empty() {
        return Err(load_error(&candidate.path, "Skill description 不能为空"));
    }
    let skill_dir = candidate
        .path
        .parent()
        .ok_or_else(|| load_error(&candidate.path, SKILL_PATH_ESCAPE))?
        .to_path_buf();
    Ok(Skill {
        metadata: SkillMetadata {
            name,
            description,
            path: candidate.path.clone(),
            skill_dir,
            scope: candidate.root.scope,
            content_digest: hex_encode(Sha256::digest(content.as_bytes())),
        },
        content: content.to_owned(),
    })
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let normalized = raw.strip_prefix('﻿').unwrap_or(raw);
    let opening_end = normalized.find('\n')?;
    let opening = normalized[..opening_end]
        .strip_suffix('\r')
        .unwrap_or(&normalized[..opening_end]);
    if opening != "---" {
        return None;
    }

    let body = &normalized[opening_end + 1..];
    let mut offset = 0usize;
    for line in body.split_inclusive('\n') {
        let without_lf = line.strip_suffix('\n').unwrap_or(line);
        let delimiter = without_lf.strip_suffix('\r').unwrap_or(without_lf);
        if delimiter == "---" {
            return Some((&body[..offset], &body[offset + line.len()..]));
        }
        offset = offset.saturating_add(line.len());
    }
    None
}

fn parse_manifest(frontmatter: &str) -> Result<(String, String), String> {
    let documents =
        YamlLoader::load_from_str(frontmatter).map_err(|_| SKILL_FRONTMATTER_INVALID.to_owned())?;
    if documents.len() != 1 {
        return Err(SKILL_FRONTMATTER_INVALID.to_owned());
    }
    let fields = documents[0]
        .as_hash()
        .ok_or_else(|| SKILL_FRONTMATTER_INVALID.to_owned())?;
    let required_string = |field_name: &str| -> Result<String, String> {
        let value = fields
            .iter()
            .find_map(|(key, value)| (key.as_str() == Some(field_name)).then_some(value))
            .ok_or_else(|| SKILL_FRONTMATTER_INVALID.to_owned())?;
        let value = value
            .as_str()
            .ok_or_else(|| format!("Skill frontmatter 字段 {field_name} 必须是字符串"))?;
        Ok(value.to_owned())
    };
    Ok((required_string("name")?, required_string("description")?))
}

fn validate_name(name: &str) -> Result<(), &'static str> {
    let first = name.as_bytes().first().copied();
    let last = name.as_bytes().last().copied();
    let valid = !name.is_empty()
        && first.is_some_and(|byte| byte.is_ascii_lowercase())
        && last.is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err("Skill name 必须使用小写字母、数字和非连续连字符")
    }
}

fn load_error(path: &Path, message: &str) -> SkillLoadError {
    SkillLoadError {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}
