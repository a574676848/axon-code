//! documentSymbol 辅助上下文 fragment。

use axon_context_fragments::PromptFragment;

/// 命中用户行号引用后的父级符号上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbolContextFragment {
    /// 用户引用的文件路径。
    pub path: String,
    /// 用户引用的 1-based 行号。
    pub line: u32,
    /// LSP 符号类型。
    pub kind: String,
    /// LSP 符号名称。
    pub name: String,
    /// 父函数、方法、类或结构体签名。
    pub signature: String,
    /// 父符号起始行。
    pub start_line: u32,
    /// 父符号结束行。
    pub end_line: u32,
}

impl DocumentSymbolContextFragment {
    /// 创建新的 documentSymbol 辅助上下文 fragment。
    #[must_use]
    pub fn new(
        path: String,
        line: u32,
        kind: String,
        name: String,
        signature: String,
        start_line: u32,
        end_line: u32,
    ) -> Self {
        Self {
            path,
            line,
            kind,
            name,
            signature,
            start_line,
            end_line,
        }
    }
}

impl PromptFragment for DocumentSymbolContextFragment {
    fn metadata(&self) -> axon_context_fragments::PromptFragmentMetadata {
        axon_context_fragments::PromptFragmentMetadata::dynamic_user()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<document-symbol-context>", "</document-symbol-context>")
    }

    fn body(&self) -> String {
        format!(
            "\n  <reference path=\"{}\" line=\"{}\" />\n  <parentSymbol kind=\"{}\" name=\"{}\" range=\"{}-{}\">{}</parentSymbol>\n",
            self.path,
            self.line,
            self.kind,
            self.name,
            self.start_line,
            self.end_line,
            self.signature
        )
    }
}
