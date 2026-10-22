//! Skill Evolution 状态机的本地当前状态存储。

use std::{
    fs::{self, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::resolved_axon_root;

pub(super) const SKILL_EVOLUTION_STATE_DIR: &str = "skill-evolution";
pub(super) const SKILL_EVOLUTION_STATE_FILE: &str = "state.json";
pub(super) const SKILL_EVOLUTION_DRAFT_DIR: &str = "drafts";

/// Skill Evolution 的持久化阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SkillEvolutionStage {
    Candidate,
    Draft,
    Validated,
    Promoted,
    Rejected,
}

/// 单个 Skill 的当前治理状态。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillEvolutionRecord {
    pub(crate) session_id: String,
    pub(crate) kind: String,
    pub(crate) skill_name: String,
    pub(crate) risk: String,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) stage: SkillEvolutionStage,
    pub(crate) material_ref: Option<String>,
    pub(crate) revision: Option<String>,
    pub(crate) reason: String,
    pub(crate) recorded_at_ms: u64,
}

/// Skill Evolution 当前状态 store。
#[derive(Debug, Clone)]
pub(crate) struct SkillEvolutionStore {
    root: PathBuf,
    state_file: PathBuf,
}

impl SkillEvolutionStore {
    pub(crate) fn open(config_path: &Path) -> Result<Self, String> {
        let axon_root = resolved_axon_root(config_path).map_err(ToOwned::to_owned)?;
        Self::open_at(&axon_root)
    }

    pub(crate) fn open_at(axon_root: &Path) -> Result<Self, String> {
        let root = axon_root.join(SKILL_EVOLUTION_STATE_DIR);
        fs::create_dir_all(&root)
            .map_err(|error| format!("创建 Skill Evolution 目录失败：{error}"))?;
        Ok(Self {
            state_file: root.join(SKILL_EVOLUTION_STATE_FILE),
            root,
        })
    }

    pub(crate) fn record_current(&self, record: &SkillEvolutionRecord) -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.state_file)
            .map_err(|error| format!("打开 Skill Evolution state 失败：{error}"))?;
        file.lock()
            .map_err(|error| format!("锁定 Skill Evolution state 失败：{error}"))?;
        let result = update_current_state(&mut file, record);
        let _ = file.unlock();
        result
    }

    pub(crate) fn records_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<SkillEvolutionRecord>, String> {
        let records = read_current_state(&self.state_file)?;
        Ok(records
            .into_iter()
            .filter(|record| record.session_id == session_id)
            .collect())
    }

    pub(super) fn draft_file(&self, skill_name: &str) -> PathBuf {
        self.root
            .join(SKILL_EVOLUTION_DRAFT_DIR)
            .join(skill_name)
            .join("SKILL.md")
    }

    pub(crate) fn state_file(&self) -> &Path {
        &self.state_file
    }
}

fn update_current_state(
    file: &mut std::fs::File,
    record: &SkillEvolutionRecord,
) -> Result<(), String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("定位 Skill Evolution state 失败：{error}"))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("读取 Skill Evolution state 失败：{error}"))?;
    let mut records = parse_state(&text)?;
    if let Some(current) = records.iter_mut().find(|current| {
        current.session_id == record.session_id
            && current.kind == record.kind
            && current.skill_name == record.skill_name
    }) {
        current.clone_from(record);
    } else {
        records.push(record.clone());
    }
    records.sort_by(|left, right| {
        (&left.session_id, &left.kind, &left.skill_name).cmp(&(
            &right.session_id,
            &right.kind,
            &right.skill_name,
        ))
    });
    let bytes = serde_json::to_vec_pretty(&records)
        .map_err(|error| format!("序列化 Skill Evolution state 失败：{error}"))?;
    file.set_len(0)
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .and_then(|()| file.write_all(&bytes))
        .and_then(|()| file.sync_data())
        .map_err(|error| format!("写入 Skill Evolution state 失败：{error}"))
}

fn read_current_state(path: &Path) -> Result<Vec<SkillEvolutionRecord>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("读取 Skill Evolution state 失败：{error}")),
    };
    parse_state(&text)
}

fn parse_state(text: &str) -> Result<Vec<SkillEvolutionRecord>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|error| format!("Skill Evolution state JSON 无效：{error}"))
}

#[cfg(test)]
#[path = "run_skill_evolution_store_tests.rs"]
mod tests;
