//! 外部编辑器集成。

use std::{
    env, fs, io,
    path::PathBuf,
    process::{Command, Stdio},
};

use tempfile::Builder;

const EDITOR_ENV_VISUAL: &str = "VISUAL";
const EDITOR_ENV_EDITOR: &str = "EDITOR";
const DEFAULT_WINDOWS_EDITOR: &str = "notepad";
const DRAFT_FILE_EXT: &str = ".md";

#[derive(Debug)]
pub(crate) enum ExternalEditorError {
    Io(io::Error),
    EmptyEditor,
    #[cfg(not(windows))]
    ParseEditor,
    Failed(i32),
}

pub(crate) fn edit_draft(initial: &str) -> Result<String, ExternalEditorError> {
    let editor = resolve_editor_command()?;
    run_editor(initial, &editor)
}

pub(crate) fn format_editor_error(error: &ExternalEditorError) -> String {
    match error {
        ExternalEditorError::Io(error) => format!("外部编辑器 I/O 失败：{error}"),
        ExternalEditorError::EmptyEditor => {
            "未找到外部编辑器，请设置 VISUAL 或 EDITOR。".to_owned()
        }
        #[cfg(not(windows))]
        ExternalEditorError::ParseEditor => {
            "外部编辑器命令解析失败，请检查 VISUAL 或 EDITOR。".to_owned()
        }
        ExternalEditorError::Failed(code) => format!("外部编辑器退出失败，exit_code={code}。"),
    }
}

fn resolve_editor_command() -> Result<Vec<String>, ExternalEditorError> {
    resolve_editor_command_from_env(|key| env::var(key))
}

fn resolve_editor_command_from_env<F>(mut get_env: F) -> Result<Vec<String>, ExternalEditorError>
where
    F: FnMut(&str) -> Result<String, env::VarError>,
{
    let raw = get_env(EDITOR_ENV_VISUAL)
        .or_else(|_| get_env(EDITOR_ENV_EDITOR))
        .or_else(|_| {
            if cfg!(windows) {
                Ok(DEFAULT_WINDOWS_EDITOR.to_owned())
            } else {
                Err(env::VarError::NotPresent)
            }
        })
        .map_err(|_| ExternalEditorError::EmptyEditor)?;
    parse_editor_command(&raw)
}

#[cfg(windows)]
fn parse_editor_command(value: &str) -> Result<Vec<String>, ExternalEditorError> {
    let command = winsplit::split(value)
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    validate_editor_command(command)
}

#[cfg(not(windows))]
fn parse_editor_command(value: &str) -> Result<Vec<String>, ExternalEditorError> {
    let command = shlex::split(value).ok_or(ExternalEditorError::ParseEditor)?;
    validate_editor_command(command)
}

fn validate_editor_command(command: Vec<String>) -> Result<Vec<String>, ExternalEditorError> {
    if command.is_empty() {
        Err(ExternalEditorError::EmptyEditor)
    } else {
        Ok(command)
    }
}

fn run_editor(initial: &str, editor: &[String]) -> Result<String, ExternalEditorError> {
    let path = draft_path()?;
    fs::write(&path, initial).map_err(ExternalEditorError::Io)?;

    let mut command = editor_command(editor)?;
    if editor.len() > 1 {
        command.args(&editor[1..]);
    }
    let status = command
        .arg(&path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(ExternalEditorError::Io)?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(ExternalEditorError::Failed(code));
    }

    let edited = fs::read_to_string(&path).map_err(ExternalEditorError::Io)?;
    Ok(edited.trim_end_matches(['\r', '\n']).to_owned())
}

fn editor_command(editor: &[String]) -> Result<Command, ExternalEditorError> {
    let Some(program) = editor.first() else {
        return Err(ExternalEditorError::EmptyEditor);
    };
    Ok(Command::new(resolve_editor_program(program)))
}

#[cfg(windows)]
fn resolve_editor_program(program: &str) -> PathBuf {
    which::which(program).unwrap_or_else(|_| PathBuf::from(program))
}

#[cfg(not(windows))]
fn resolve_editor_program(program: &str) -> PathBuf {
    PathBuf::from(program)
}

fn draft_path() -> Result<tempfile::TempPath, ExternalEditorError> {
    Builder::new()
        .prefix("axon-tui-draft-")
        .suffix(DRAFT_FILE_EXT)
        .tempfile()
        .map(tempfile::NamedTempFile::into_temp_path)
        .map_err(ExternalEditorError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_editor_command_prefers_visual() {
        let command = resolve_editor_command_from_env(|key| match key {
            EDITOR_ENV_VISUAL => Ok("visual-editor --wait".to_owned()),
            EDITOR_ENV_EDITOR => Ok("fallback-editor".to_owned()),
            _ => Err(env::VarError::NotPresent),
        })
        .expect("VISUAL 应优先作为外部编辑器");

        assert_eq!(
            command,
            vec!["visual-editor".to_owned(), "--wait".to_owned()]
        );
    }

    #[test]
    fn resolve_editor_command_uses_editor_when_visual_missing() {
        let command = resolve_editor_command_from_env(|key| {
            if key == EDITOR_ENV_EDITOR {
                Ok("fallback-editor --foreground".to_owned())
            } else {
                Err(env::VarError::NotPresent)
            }
        })
        .expect("EDITOR 应在 VISUAL 缺失时生效");

        assert_eq!(
            command,
            vec!["fallback-editor".to_owned(), "--foreground".to_owned()]
        );
    }

    #[test]
    fn run_editor_returns_process_edited_content() {
        let editor = test_editor_command();

        let edited = run_editor("seed", &editor).expect("测试编辑器应写回草稿");

        assert_eq!(edited, "edited");
    }

    #[cfg(not(windows))]
    #[test]
    fn format_editor_error_reports_parse_failure() {
        assert!(format_editor_error(&ExternalEditorError::ParseEditor).contains("命令解析失败"));
    }

    #[cfg(windows)]
    fn test_editor_command() -> Vec<String> {
        vec![
            "powershell".to_owned(),
            "-NoLogo".to_owned(),
            "-NoProfile".to_owned(),
            "-Command".to_owned(),
            "& { param($path) Set-Content -LiteralPath $path -Value edited }".to_owned(),
        ]
    }

    #[cfg(not(windows))]
    fn test_editor_command() -> Vec<String> {
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf edited > \"$1\"".to_owned(),
            "axon-editor-test".to_owned(),
        ]
    }
}
