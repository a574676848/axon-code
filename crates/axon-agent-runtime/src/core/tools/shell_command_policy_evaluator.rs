//! shell_command 的调用级权限判定与写入影响识别。
//!
//! **权限判定只覆盖参数结构**（对齐 DSH）：命令内容不做任何预检：既没有灾难性命令
//! 词表，也不按方言拦截 heredoc / here-string。语法与方言错误一律交给 shell 自身在运行期
//! 回报，而不是由 Axon 的词法分析猜测。内容词表与方言拦截已删除：它们与
//! `shell_command_static_analyzer` 给出的修法指引互相矛盾，导致 Windows 上 heredoc 与
//! here-string 同时被禁，shell_command 丧失多行脚本能力。
//!
//! 本模块**仍然输出**路径与只读性推断（`affected_paths` / `is_read_only` /
//! `write_paths_are_complete`），但它们只服务快照范围与并发写协调，**不构成执行许可**：
//! 推断失败最多让写作用域回退整工作区，绝不会改变命令能否运行。

use std::path::{Path, PathBuf};

use axon_domain::{ToolCallInput, ToolExecutionPermission};

use crate::agent_run_shell_tool_args_from_provider_input;

const FORBIDDEN_PREFIX: &str = "shell_command 禁止执行";
const INVALID_ARGUMENTS_REASON: &str = "调用参数或命令结构无效";
const REDIRECT_TOKENS: &[&str] = &["1>>", "2>>", ">>", "1>", "2>", ">"];
const REDIRECT_NON_PATH_PREFIXES: &[char] = &['=', '>', '&', '|'];
const TOKEN_STOPPERS: &[&str] = &["|", "&&", ";"];
const TEE_FILE_FLAGS: &[&str] = &["-filepath", "-file"];
/// 会写入文件系统的 PowerShell 文件类 cmdlet。
///
/// PowerShell 写法（`Set-Content -Path x`）不经过 shell 重定向，旧的静态提取因此看不到
/// 任何目标：后果不只是写作用域：会话不会为这些路径做前后快照，回执的
/// `filesystem_write_performed` 会**漏报**，`writeIntent` 与 post-run LSP 一并缺失，
/// 而该标记会下发给模型并驱动 TUI diff 与回合结果判定。本表让这些目标进入
/// `affected_paths`，从而被观测到。
///
/// `tee-object` 不在此表：它已由 `collect_tee_paths` 处理，重复登记无意义。
const PATH_BEARING_CMDLETS: &[&str] = &[
    "add-content",
    "clear-content",
    "copy-item",
    "move-item",
    "new-item",
    "out-file",
    "remove-item",
    "rename-item",
    "set-content",
    "set-item",
];
/// cmdlet 中承担**路径**语义的参数名。
const CMDLET_PATH_FLAGS: &[&str] = &[
    "-destination",
    "-filepath",
    "-literalpath",
    "-outfile",
    "-path",
    "-target",
];
/// 取值但**不是路径**的参数名；解析位置参数时必须跳过它们的取值，
/// 否则 `Set-Content -Path x -Value y` 会把 `y` 误登记成写入目标。
const CMDLET_VALUE_FLAGS: &[&str] = &[
    "-computername",
    "-credential",
    "-delimiter",
    "-encoding",
    "-erroraction",
    "-exclude",
    "-filter",
    "-include",
    "-informationaction",
    "-inputobject",
    "-itemtype",
    "-name",
    "-separator",
    "-stream",
    "-value",
    "-value2",
    "-warningaction",
    "-width",
];
const READ_ONLY_COMMANDS: &[&str] = &[
    "cat",
    "cut",
    "diff",
    "findstr",
    "get-childitem",
    "get-command",
    "get-content",
    "get-date",
    "get-item",
    "get-location",
    "get-process",
    "grep",
    "head",
    "ls",
    "measure-object",
    "pwd",
    "readlink",
    "resolve-path",
    "rg",
    "select-object",
    "select-string",
    "sort",
    "sort-object",
    "tail",
    "test-path",
    "tr",
    "uniq",
    "wc",
    "where-object",
    "write-output",
];
/// 允许把写作用域收窄到重定向 / `tee` 目标的最小程序白名单。
///
/// 收窄等价于向并发协调器声明「命令自身的写入 ⊆ 重定向与 `tee` 目标」。
/// 静态分析无法一般性地证明这一点：`cargo test > out.log` 还会写 `target/`，
/// `npm install > install.log` 还会写 `node_modules/`。因此只对
/// **除标准输出与标准错误外不产生任何文件系统副作用**的程序收窄。
///
/// 本白名单**必须保守**：新增条目即为该程序的写入行为背书。即便看似纯输出，
/// 以下程序也**不得**加入：`env`（可执行任意命令）、`sort`（`-o` 写文件）、
/// `sed`（`-i` 原地写）、`awk`（程序内 `print > "file"`）、`tee`（按设计写文件，
/// 纳入即需为 `collect_tee_paths` 的完备性背书）。
const WRITE_SCOPE_COMPLETE_COMMANDS: &[&str] = &[
    "basename",
    "date",
    "dirname",
    "echo",
    "false",
    "get-date",
    "get-host",
    "get-location",
    "hostname",
    "id",
    "printenv",
    "printf",
    "pwd",
    "seq",
    "true",
    "uname",
    "whoami",
    "write-host",
    "write-output",
    "yes",
];
const READ_ONLY_GIT_SUBCOMMANDS: &[&str] = &[
    "cat-file",
    "diff",
    "grep",
    "log",
    "ls-files",
    "ls-tree",
    "merge-base",
    "rev-parse",
    "show",
    "status",
];
const DYNAMIC_EXECUTION_MARKERS: &[&str] = &["$(`", "$(", "`", "${", "@(", "<(", ">("];

/// shell_command 单次调用的权限求值结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandPolicyEvaluation {
    /// 本次调用的最终执行权限；仅当参数结构不合法时为 `Forbidden`。
    pub execution_permission: ToolExecutionPermission,
    /// 本次调用是否可静态证明为只读。
    pub is_read_only: bool,
    /// 静态可确定的潜在写入路径。
    pub affected_paths: Vec<PathBuf>,
    /// `affected_paths` 是否构成该命令的**完整**写入集。
    ///
    /// 为真时调用方可把写作用域收窄到 `affected_paths`；为假时**必须**回退整工作区，
    /// 因为命令自身还可能写入未被静态分析捕获的位置。判据见 `write_scope_is_complete`。
    pub write_paths_are_complete: bool,
}

/// shell_command 共享权限判定器。
#[derive(Debug, Default, Clone, Copy)]
pub struct ShellCommandPolicyEvaluator;

impl ShellCommandPolicyEvaluator {
    /// 使用与执行入口相同的参数解析规则，对本次 shell_command 调用求值。
    #[must_use]
    pub fn evaluate(input: &ToolCallInput, base_cwd: &Path) -> ShellCommandPolicyEvaluation {
        let args = match agent_run_shell_tool_args_from_provider_input(input, base_cwd) {
            Ok(args) => args,
            Err(message) => {
                return ShellCommandPolicyEvaluation {
                    execution_permission: forbidden(format!(
                        "{INVALID_ARGUMENTS_REASON}：{message}"
                    )),
                    is_read_only: false,
                    affected_paths: Vec::new(),
                    write_paths_are_complete: false,
                };
            }
        };
        let Some(script) = input
            .as_json()
            .and_then(|value| value.get("command"))
            .and_then(serde_json::Value::as_str)
        else {
            return ShellCommandPolicyEvaluation {
                execution_permission: forbidden(INVALID_ARGUMENTS_REASON),
                is_read_only: false,
                affected_paths: Vec::new(),
                write_paths_are_complete: false,
            };
        };
        let script = crate::shell_command_static_analyzer::unwrap_outer_shell_wrapper(script);
        let script = script.as_str();
        let DetectedWritePaths {
            paths,
            dropped_target,
        } = detect_script_paths(script, &args.cwd);
        let is_read_only = is_provably_read_only(script);
        // 命令内容不参与权限判定：参数解析成功即放行，内容层面的错误由 shell 回报。
        let execution_permission = ToolExecutionPermission::Allowed;
        ShellCommandPolicyEvaluation {
            execution_permission,
            is_read_only,
            affected_paths: paths,
            write_paths_are_complete: write_scope_is_complete(script, dropped_target),
        }
    }
}

fn forbidden(reason: impl Into<String>) -> ToolExecutionPermission {
    ToolExecutionPermission::Forbidden {
        reason: format!("{FORBIDDEN_PREFIX}：{}", reason.into()),
    }
}

fn is_provably_read_only(script: &str) -> bool {
    if script.trim().is_empty()
        || script.contains(['{', '}'])
        || DYNAMIC_EXECUTION_MARKERS
            .iter()
            .any(|marker| script.contains(marker))
        || contains_redirection(script)
    {
        return false;
    }
    let commands = command_segments(script);
    !commands.is_empty() && commands.iter().all(|command| command_is_read_only(command))
}

/// 判断 `affected_paths` 是否构成该脚本的完整写入集。
///
/// 收窄写作用域等价于向并发协调器**声明**「命令自身的写入 ⊆ `affected_paths`」，
/// 因此必须四道否决全过才成立：
///
/// 1. **动态执行标记**（`$(...)` / `${...}` / 反引号 / `@(...)` / `<(...)`、`>(...)`
///    进程替换）：其中的命令可写任意路径，且出现在**命令参数**位置时根本不经过
///    目标提取：故只能整脚本否决。判据与 `is_provably_read_only` 保持一致，
///    避免同一类危险在两条路径上被判成不同结果。
/// 2. **圆括号与 `::` 静态调用**：PowerShell 的 `(...)` 子表达式（如
///    `(iex 'npm install')`）与 `[Type]::Method(...)` 静态调用、以及 bash 的子 shell
///    求值，其内容无法被本文件的词法分析覆盖：凡出现即不可证完整。此否决**只**
///    用于写作用域判定：只读判定不纳入裸 `(`，避免纯读的 PowerShell 括号命令
///    （如 `Get-Content (Resolve-Path x)`）被过度剥夺审批直通。
/// 3. **花括号展开**：`{a,b}.log` 是单个 token，展开后却是多个真实目标。
/// 4. **目标提取曾丢弃候选**（见 `DetectedWritePaths::dropped_target`）：
///    `$TARGET` / `*.log` 这类被拒绝的 token 仍会被 shell 真实写入。
///
/// 第 5 道才是程序白名单：任一不可信的程序（构建、包管理、脚本解释器等）独立否决整个
/// 脚本，因此 `cargo test | tee out.log` 中的 `cargo` 段会让整条命令回退整工作区。
///
/// **前 4 道是 fail-closed 的关键**：只靠白名单会漏掉「白名单程序 + 动态目标」这一
/// 组合：例如 `Write-Output $(npm install) > a.log` 的段首词在白名单内，但
/// `npm install` 正在写 `node_modules/`。
fn write_scope_is_complete(script: &str, dropped_target: bool) -> bool {
    if dropped_target
        || script.contains(['{', '}'])
        || DYNAMIC_EXECUTION_MARKERS
            .iter()
            .any(|marker| script.contains(marker))
        || script.contains('(')
        || script.contains("::")
    {
        return false;
    }
    let commands = command_segments(script);
    !commands.is_empty()
        && commands.iter().all(|command| {
            shell_words(command)
                .first()
                .and_then(|value| program_name(value))
                .is_some_and(|program| WRITE_SCOPE_COMPLETE_COMMANDS.contains(&program.as_str()))
        })
}

fn contains_redirection(script: &str) -> bool {
    let tokens = shell_words(script);
    tokens.iter().any(|token| {
        REDIRECT_TOKENS.contains(&token.as_str())
            || REDIRECT_TOKENS
                .iter()
                .any(|operator| token.starts_with(operator) && token.len() > operator.len())
    })
}

fn command_segments(script: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in script.chars() {
        match (quote, ch) {
            (Some(active), value) if value == active => {
                quote = None;
                current.push(ch);
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                current.push(ch);
            }
            (None, '\n' | '\r' | ';' | '|') => push_segment(&mut segments, &mut current),
            (None, '&') => push_segment(&mut segments, &mut current),
            _ => current.push(ch),
        }
    }
    push_segment(&mut segments, &mut current);
    segments
}

fn push_segment(segments: &mut Vec<String>, current: &mut String) {
    let value = current.trim();
    if !value.is_empty() && !value.starts_with('#') {
        segments.push(value.to_owned());
    }
    current.clear();
}

fn command_is_read_only(command: &str) -> bool {
    let words = shell_words(command);
    let Some(program) = words.first().and_then(|value| program_name(value)) else {
        return false;
    };
    if program == "git" {
        return words
            .iter()
            .skip(1)
            .find(|word| !word.starts_with('-'))
            .and_then(|word| program_name(word))
            .is_some_and(|subcommand| READ_ONLY_GIT_SUBCOMMANDS.contains(&subcommand.as_str()));
    }
    READ_ONLY_COMMANDS.contains(&program.as_str())
}

/// 静态写入路径提取结果。
struct DetectedWritePaths {
    /// 静态可确定的潜在写入路径（已排序去重）。
    paths: Vec<PathBuf>,
    /// 提取过程中是否**丢弃过候选目标**（含重定向符后缺失目标）。
    ///
    /// 为真即意味着 `paths` **不构成完整写入集**：被丢弃的 token（`$TARGET`、`*.log`、
    /// `$(...)` 产物等）仍会被 shell 真实写入，只是静态分析无法确定其位置。
    dropped_target: bool,
}

fn detect_script_paths(script: &str, cwd: &Path) -> DetectedWritePaths {
    let mut paths = Vec::new();
    let mut dropped_target = false;
    for line in script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let tokens = shell_words(line);
        collect_redirect_paths(&tokens, cwd, &mut paths, &mut dropped_target);
        collect_tee_paths(&tokens, cwd, &mut paths, &mut dropped_target);
        // cmdlet 只认**段首词**：按命令段（`|` / `&` / `;` 分隔）切分后再扫描，
        // 这样 `echo Set-Content -Path ghost.txt` 这类把 cmdlet 名当**参数**的写法
        // 不会产生幻影路径（幻影路径会进入 `writeIntent.affectedPaths` 并误导模型）。
        for segment in command_segments(line) {
            collect_cmdlet_paths(&shell_words(&segment), cwd, &mut paths, &mut dropped_target);
        }
    }
    paths.sort();
    paths.dedup();
    DetectedWritePaths {
        paths,
        dropped_target,
    }
}

fn collect_redirect_paths(
    tokens: &[String],
    cwd: &Path,
    paths: &mut Vec<PathBuf>,
    dropped: &mut bool,
) {
    for (index, token) in tokens.iter().enumerate() {
        if REDIRECT_TOKENS.contains(&token.as_str()) {
            // 重定向符后没有可解析目标（如行尾孤立的 `>`）同样计为丢弃：
            // 无法证明该重定向不写任何路径，按 fail-closed 处理。
            *dropped |= !push_path_token(tokens.get(index + 1), cwd, paths);
        } else if let Some(path) = attached_redirect_path(token) {
            *dropped |= !push_path_value(path, cwd, paths);
        }
    }
}

fn attached_redirect_path(token: &str) -> Option<&str> {
    let path = REDIRECT_TOKENS
        .iter()
        .find_map(|operator| token.strip_prefix(operator))?;
    (!path.is_empty() && !path.starts_with(|ch| REDIRECT_NON_PATH_PREFIXES.contains(&ch)))
        .then_some(path)
}

fn collect_tee_paths(tokens: &[String], cwd: &Path, paths: &mut Vec<PathBuf>, dropped: &mut bool) {
    let mut index = 0usize;
    while index < tokens.len() {
        let Some(command) = program_name(&tokens[index]) else {
            index += 1;
            continue;
        };
        index += 1;
        match command.as_str() {
            "tee" => {
                while index < tokens.len() && !TOKEN_STOPPERS.contains(&tokens[index].as_str()) {
                    if !tokens[index].starts_with('-') {
                        *dropped |= !push_path_value(&tokens[index], cwd, paths);
                    }
                    index += 1;
                }
            }
            "tee-object" => {
                while index < tokens.len() && !TOKEN_STOPPERS.contains(&tokens[index].as_str()) {
                    let lower = tokens[index].to_ascii_lowercase();
                    if TEE_FILE_FLAGS.contains(&lower.as_str()) {
                        *dropped |= !push_path_token(tokens.get(index + 1), cwd, paths);
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
            }
            _ => {}
        }
    }
}

/// 提取 PowerShell 文件类 cmdlet 的写入目标。
///
/// **只认段首词**：调用方按命令段（`|` / `&` / `;` 分隔）切分后传入，因此
/// `echo Set-Content -Path x` 这类把 cmdlet 名当**参数**的写法不会产生幻影路径；
/// `Get-Content a | Set-Content b` 的第二段仍会被正确识别。
///
/// 覆盖三种写法：显式路径参数（`-Path` / `-LiteralPath` / `-FilePath` / `-Destination` /
/// `-Target` / `-OutFile`）、位置参数（`Copy-Item a b`、`Remove-Item d.txt`）、
/// 以及二者混用（`New-Item -ItemType Directory -Force nd`）。
///
/// 动态目标（`$env:TEMP\x`、含通配符）经 `push_path_value` 拒绝并置 `dropped`：
/// 与重定向路径同一口径，因此「写入集不可证完整」的判定同样适用于 cmdlet。
///
/// 未知参数一律按**开关**处理（不吞其后的 token）：把取值误当位置目标只会多登记一个
/// 路径、多一次快照（安全），而把位置目标误当取值会让写入集漏报（不安全）。
fn collect_cmdlet_paths(
    tokens: &[String],
    cwd: &Path,
    paths: &mut Vec<PathBuf>,
    dropped: &mut bool,
) {
    let Some(program) = tokens.first().and_then(|value| program_name(value)) else {
        return;
    };
    if !PATH_BEARING_CMDLETS.contains(&program.as_str()) {
        return;
    }
    // 源与目标都参与变化的 cmdlet 需要两个位置参数。
    let positional_limit = match program.as_str() {
        "copy-item" | "move-item" | "rename-item" => 2,
        _ => 1,
    };
    let mut positionals = 0usize;
    let mut index = 1usize;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        let lower = token.to_ascii_lowercase();
        if CMDLET_PATH_FLAGS.contains(&lower.as_str()) {
            *dropped |= !push_path_token(tokens.get(index + 1), cwd, paths);
            index += 2;
            continue;
        }
        if CMDLET_VALUE_FLAGS.contains(&lower.as_str()) {
            index += 2;
            continue;
        }
        if token.starts_with('-') {
            index += 1;
            continue;
        }
        if positionals < positional_limit {
            *dropped |= !push_path_value(token, cwd, paths);
            positionals += 1;
        }
        index += 1;
    }
}

/// 登记重定向 / `tee` 的目标 token。
///
/// 返回是否成功登记；`false` 表示目标缺失或被拒绝：两种情况都意味着
/// **写入集不可证完整**，调用方必须据此否决收窄。
fn push_path_token(token: Option<&String>, cwd: &Path, paths: &mut Vec<PathBuf>) -> bool {
    token.is_some_and(|token| push_path_value(token, cwd, paths))
}

/// 登记单个路径字面量。
///
/// 返回是否成功登记。含动态 / 通配字符的值**无法确定运行时目标**，此处拒绝登记并
/// 返回 `false`：该信号是「写入集不可证完整」的核心判据，不得静默丢弃。
/// `&` 同样拒绝：`shell_words` 不把它当操作符分词，`echo x >a.log&echo y>b.log`
/// 会提取出假路径 `a.log&echo` 并丢失真目标 `a.log`（R2 审计 N-A-03）。
fn push_path_value(value: &str, cwd: &Path, paths: &mut Vec<PathBuf>) -> bool {
    let cleaned = value.trim().trim_matches('"').trim_matches('\'');
    if cleaned.is_empty()
        || cleaned.starts_with('-')
        || cleaned.chars().any(|ch| {
            matches!(
                ch,
                '$' | '*' | '?' | '{' | '}' | '`' | '<' | '>' | '|' | '&'
            )
        })
    {
        return false;
    }
    let path = PathBuf::from(cleaned);
    paths.push(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    });
    true
}

fn shell_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in value.chars() {
        match (quote, ch) {
            (Some(active), current_char) if current_char == active => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, ' ' | '\t' | '\r' | '\n') => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            (None, '|' | ';') => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                words.push(ch.to_string());
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn program_name(value: &str) -> Option<String> {
    Path::new(value)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
#[path = "shell_command_policy_evaluator_tests.rs"]
mod tests;
