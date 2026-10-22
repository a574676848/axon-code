//! Skill 文档的验证、revision 与原子落盘。

use std::{fs, io::Write as _, path::Path};

use axon_agent_runtime::{SkillLoader, SkillRoot, SkillScope, build_skill_injection};
use sha2::{Digest as _, Sha256};
use tempfile::NamedTempFile;

use super::run_skill_evolution_store::SkillEvolutionStore;

const SKILL_FILE_NAME: &str = "SKILL.md";
const SKILL_REVISION_DIR: &str = ".revisions";
const SKILL_DOCUMENT_MAX_BYTES: usize = 256 * 1024;
const DRAFT_DESCRIPTION: &str = "由 Axon 基于已验证执行证据生成的可复用流程草案";
const NEGATIVE_EVAL_PROMPT: &str = "执行一个与该 Skill 无关的普通任务";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillMaterialization {
    pub(crate) material_ref: String,
    pub(crate) revision: String,
}

pub(super) fn update_existing_skill(
    skill_file: &Path,
    skill_name: &str,
    summary: &str,
) -> Result<Option<SkillMaterialization>, String> {
    let existing =
        fs::read_to_string(skill_file).map_err(|error| format!("读取现有 Skill 失败：{error}"))?;
    validate_skill_document(skill_name, &existing)?;
    if existing.contains(summary) {
        return Ok(None);
    }
    let addition = format!("\n\n## 已验证经验\n\n- {}\n", summary.trim());
    let updated = format!("{}{}", existing.trim_end(), addition);
    validate_skill_document(skill_name, &updated)?;
    preserve_revision(skill_file, &existing)?;
    atomic_write(skill_file, updated.as_bytes())?;
    Ok(Some(materialization(skill_file, &updated)))
}

pub(super) fn write_validated_draft(
    store: &SkillEvolutionStore,
    skill_name: &str,
    summary: &str,
) -> Result<SkillMaterialization, String> {
    let document = format!(
        "---\nname: {skill_name}\ndescription: {DRAFT_DESCRIPTION}\n---\n\n# 可复用流程\n\n## 执行步骤\n\n{}\n\n## 验证\n\n- 必须使用独立运行时回执确认实际执行结果。\n",
        summary.trim()
    );
    validate_skill_document(skill_name, &document)?;
    let draft_file = store.draft_file(skill_name);
    atomic_write(&draft_file, document.as_bytes())?;
    Ok(materialization(&draft_file, &document))
}

pub(crate) fn promote_validated_draft(
    store: &SkillEvolutionStore,
    skill_name: &str,
    target_skill_root: &Path,
) -> Result<SkillMaterialization, String> {
    let draft_file = store.draft_file(skill_name);
    let document = fs::read_to_string(&draft_file)
        .map_err(|error| format!("读取 Skill draft 失败：{error}"))?;
    validate_skill_document(skill_name, &document)?;
    let target = target_skill_root.join(skill_name).join(SKILL_FILE_NAME);
    if target.exists() {
        return Err("目标 Skill 已存在，禁止覆盖晋升".to_owned());
    }
    atomic_write(&target, document.as_bytes())?;
    Ok(materialization(&target, &document))
}

fn validate_skill_document(skill_name: &str, document: &str) -> Result<(), String> {
    if document.len() > SKILL_DOCUMENT_MAX_BYTES {
        return Err("Skill 文档超过大小限制".to_owned());
    }
    if !SkillLoader::has_valid_frontmatter(document) {
        return Err("Skill frontmatter 校验失败".to_owned());
    }
    let staging =
        tempfile::tempdir().map_err(|error| format!("创建 Skill eval 目录失败：{error}"))?;
    let skill_dir = staging.path().join(skill_name);
    fs::create_dir_all(&skill_dir)
        .map_err(|error| format!("创建 Skill eval 子目录失败：{error}"))?;
    fs::write(skill_dir.join(SKILL_FILE_NAME), document)
        .map_err(|error| format!("写入 Skill eval 文件失败：{error}"))?;
    let outcome = SkillLoader::load_roots(&[SkillRoot::new(staging.path(), SkillScope::User, 1)]);
    if !outcome.errors.is_empty()
        || outcome.skills.len() != 1
        || outcome.skills[0].metadata.name != skill_name
    {
        return Err("Skill schema eval 失败".to_owned());
    }
    let positive = build_skill_injection(&outcome, &format!("@skill:{skill_name} 验证流程"));
    if positive.selected_skill_name() != Some(skill_name) {
        return Err("Skill 正向触发 eval 失败".to_owned());
    }
    if build_skill_injection(&outcome, NEGATIVE_EVAL_PROMPT)
        .selected
        .is_some()
    {
        return Err("Skill 负向不触发 eval 失败".to_owned());
    }
    Ok(())
}

fn preserve_revision(skill_file: &Path, existing: &str) -> Result<(), String> {
    let parent = skill_file
        .parent()
        .ok_or_else(|| "Skill 文件缺少父目录".to_owned())?;
    let revision_dir = parent.join(SKILL_REVISION_DIR);
    fs::create_dir_all(&revision_dir)
        .map_err(|error| format!("创建 Skill revision 目录失败：{error}"))?;
    let revision_file = revision_dir.join(format!("{}.md", digest(existing)));
    if !revision_file.exists() {
        atomic_write(&revision_file, existing.as_bytes())?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Skill 写入路径缺少父目录".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建 Skill 目录失败：{error}"))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| format!("创建 Skill 临时文件失败：{error}"))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("写入 Skill 临时文件失败：{error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("原子替换 Skill 文件失败：{}", error.error))?;
    Ok(())
}

fn materialization(path: &Path, document: &str) -> SkillMaterialization {
    SkillMaterialization {
        material_ref: path.to_string_lossy().replace('\\', "/"),
        revision: digest(document),
    }
}

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub(super) fn draft_skill_name(summary: &str) -> String {
    format!("learned-procedure-{}", &digest(summary)[..12])
}

#[cfg(test)]
#[path = "run_skill_evolution_materialize_tests.rs"]
mod tests;
