# axon-output-filter

`axon-output-filter` 是 Axon 的代码内容压缩与结构化过滤 crate，用于在代码进入模型上下文前降低噪声、保留高价值结构，并输出可审计的 token 节省统计。

## 核心能力

- **read_file 代码过滤**：支持 `none`、`minimal`、`aggressive` 三档过滤级别；`aggressive` 优先使用 LSP 符号，失败时回退正则。
- **LSP Symbol 投影**：解析 `textDocument/documentSymbol`，输出符号名称、类型、签名、范围和子符号。
- **正则回退**：在 LSP 不可用时覆盖 JavaScript、TypeScript、Python、Rust、Java、C#、Go 七种常见语言。
- **性能守卫**：通过 `tools/verify-output-filter.ps1` 校验语言覆盖、代码 token 节省区间和过滤耗时。

## 代码过滤

```rust
use axon_output_filter::{
    OutputFilter,
    lsp_symbol_filter::LspFilterLevel,
    regex_fallback::Language,
};

let result = OutputFilter.filter_code(
    "pub fn run() {\n    println!(\"axon\");\n}",
    Language::Rust,
    LspFilterLevel::Aggressive,
)?;

assert!(result.savings_pct > 0);
```

过滤结果包含：

- `filtered_content`：进入模型上下文的过滤后内容。
- `symbols`：LSP 符号提取结果。
- `token_savings`：原始 token、过滤后 token 与节省比例。
- `lsp_used`：本次过滤是否真实使用 LSP。

## 验收命令

```powershell
pwsh -NoLogo -NoProfile -File tools\verify-output-filter.ps1
```

该命令会输出类似：

```text
output-filter acceptance OK: lsp_support=85%, regex_languages=7, read_savings=63%, lsp=0ms, regex=0ms
```

`tools/verify-workspace.ps1` 已包含该验收守卫，workspace 级验证会同步检查输出过滤能力。
