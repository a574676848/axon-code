use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_agent_runtime::{
    PromptFragment, PromptFragmentMetadata, WORKSPACE_INSTRUCTIONS_END_MARKER,
    WORKSPACE_INSTRUCTIONS_START_MARKER,
};

const WORKSPACE_INSTRUCTIONS_FILE_NAME: &str = "AGENTS.md";
/// 指令来源类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstructionProvenance {
    /// 项目级指令：<project>/AGENTS.md
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceInstruction {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) provenance: InstructionProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceInstructionsDeveloperFragment {
    pub(crate) directory: Option<String>,
    pub(crate) text: String,
}

impl PromptFragment for WorkspaceInstructionsDeveloperFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            WORKSPACE_INSTRUCTIONS_START_MARKER,
            WORKSPACE_INSTRUCTIONS_END_MARKER,
        )
    }

    fn body(&self) -> String {
        let directory = self
            .directory
            .as_ref()
            .map(|directory| format!(" for {directory}"))
            .unwrap_or_default();
        format!("{directory}\n\n<INSTRUCTIONS>\n{}\n", self.text)
    }
}

pub(crate) fn workspace_instructions_developer_fragment(
    directory: Option<&str>,
    instructions: &[WorkspaceInstruction],
) -> Option<WorkspaceInstructionsDeveloperFragment> {
    render_workspace_instructions_text(instructions).map(|text| {
        WorkspaceInstructionsDeveloperFragment {
            directory: directory.map(ToOwned::to_owned),
            text,
        }
    })
}

fn render_workspace_instructions_text(instructions: &[WorkspaceInstruction]) -> Option<String> {
    let mut output = String::new();

    for instruction in instructions {
        let content = instruction.content.trim();
        if content.is_empty() {
            continue;
        }
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(content);
    }

    (!output.is_empty()).then_some(output)
}

/// 仅加载项目级 AGENTS.md 指令。
///
/// 加载顺序：从 Git 根目录到当前工作目录的所有 AGENTS.md。
pub(crate) fn load_workspace_instructions(cwd: &Path) -> Result<Vec<WorkspaceInstruction>, String> {
    load_project_instructions(cwd)
}

fn load_project_instructions(cwd: &Path) -> Result<Vec<WorkspaceInstruction>, String> {
    let mut project_paths = instruction_paths(cwd)?;
    project_paths.reverse(); // 根 → 叶顺序

    project_paths
        .into_iter()
        .map(|path| read_instruction_file(cwd, &path))
        .filter_map(|result| match result {
            Ok(instruction) if instruction.content.is_empty() => None,
            other => Some(other),
        })
        .collect()
}

fn instruction_paths(cwd: &Path) -> Result<Vec<PathBuf>, String> {
    let root = cwd
        .canonicalize()
        .map_err(|_| format!("工作目录不可访问：{}", cwd.display()))?;
    let mut paths = Vec::new();
    let mut current = root.as_path();
    loop {
        let candidate = current.join(WORKSPACE_INSTRUCTIONS_FILE_NAME);
        if candidate.is_file() {
            paths.push(candidate);
        }
        if current.join(".git").is_dir() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    Ok(paths)
}

fn read_instruction_file(cwd: &Path, path: &Path) -> Result<WorkspaceInstruction, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("无法读取工作区指令文件 {}：{error}", path.display()))?;
    Ok(WorkspaceInstruction {
        path: relative_instruction_path(cwd, path),
        content: content.trim().to_owned(),
        provenance: InstructionProvenance::Project,
    })
}

fn relative_instruction_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_workspace_agents_is_skipped() {
        let root = temp_root("agents-missing");
        let child = root.join("child");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(&child).unwrap();

        let instructions = load_project_instructions(&child).unwrap();

        assert!(instructions.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_instructions_load_root_to_leaf_until_git_root() {
        let root = temp_root("agents-instructions");
        let child = root.join("app").join("feature");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(&child).unwrap();
        fs::write(root.join(WORKSPACE_INSTRUCTIONS_FILE_NAME), "root rules").unwrap();
        fs::write(child.join(WORKSPACE_INSTRUCTIONS_FILE_NAME), "leaf rules").unwrap();

        let instructions = load_project_instructions(&child).unwrap();

        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0].content, "root rules");
        assert_eq!(instructions[0].provenance, InstructionProvenance::Project);
        assert_eq!(instructions[1].content, "leaf rules");
        assert_eq!(instructions[1].provenance, InstructionProvenance::Project);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_instructions_do_not_cross_sibling_workspaces() {
        let root = temp_root("workspace-scope");
        let first = root.join("first-project");
        let second = root.join("second-project").join("nested");
        fs::create_dir_all(first.join(".git")).unwrap();
        fs::create_dir_all(root.join("second-project").join(".git")).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join(WORKSPACE_INSTRUCTIONS_FILE_NAME), "first rules").unwrap();
        fs::write(
            root.join("second-project")
                .join(WORKSPACE_INSTRUCTIONS_FILE_NAME),
            "second rules",
        )
        .unwrap();

        let instructions = load_project_instructions(&second).unwrap();

        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0].content, "second rules");
        assert!(
            instructions
                .iter()
                .all(|instruction| !instruction.content.contains("first rules"))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_instructions_load_all_files_from_root_to_leaf() {
        let root = temp_root("agents-deep-instructions");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(WORKSPACE_INSTRUCTIONS_FILE_NAME), "root rules").unwrap();

        let mut child = root.clone();
        for depth in 0..20 {
            child = child.join(format!("level-{depth}"));
            fs::create_dir_all(&child).unwrap();
            fs::write(
                child.join(WORKSPACE_INSTRUCTIONS_FILE_NAME),
                format!("rules {depth}"),
            )
            .unwrap();
        }

        let instructions = load_project_instructions(&child).unwrap();

        assert_eq!(instructions.len(), 21);
        assert_eq!(instructions.first().unwrap().content, "root rules");
        assert_eq!(instructions.last().unwrap().content, "rules 19");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_instructions_preserve_content_beyond_legacy_total_budget() {
        let root = temp_root("agents-without-total-budget");
        let child = root.join("child");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(&child).unwrap();
        let root_content = "A".repeat(40 * 1024);
        let child_content = "中文 rules".repeat(1_000);
        fs::write(root.join(WORKSPACE_INSTRUCTIONS_FILE_NAME), &root_content).unwrap();
        fs::write(child.join(WORKSPACE_INSTRUCTIONS_FILE_NAME), &child_content).unwrap();

        let instructions = load_project_instructions(&child).unwrap();

        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0].content, root_content);
        assert_eq!(instructions[1].content, child_content);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_instructions_developer_fragment_joins_project_rules_without_user_separator() {
        let instructions = vec![
            WorkspaceInstruction {
                path: "AGENTS.md".to_owned(),
                content: "根规则".to_owned(),
                provenance: InstructionProvenance::Project,
            },
            WorkspaceInstruction {
                path: "app/AGENTS.md".to_owned(),
                content: "应用规则".to_owned(),
                provenance: InstructionProvenance::Project,
            },
        ];

        let fragment =
            workspace_instructions_developer_fragment(Some("E:/zbg/axon-code"), &instructions)
                .expect("非空 AGENTS.md 应生成 developer fragment");

        assert_eq!(fragment.role(), "developer");
        assert_eq!(
            fragment.render(),
            "# AGENTS.md instructions for E:/zbg/axon-code\n\n<INSTRUCTIONS>\n根规则\n\n应用规则\n</INSTRUCTIONS>"
        );
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root()
            .join(format!("axon-cli-{label}-{}-{nonce}", std::process::id()))
    }
}
