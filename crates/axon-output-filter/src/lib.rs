//! Axon 输出过滤能力的独立 crate。
//!
//! 本 crate 承载代码读取与正则回退过滤逻辑，保持与工具运行时解耦。

use axon_domain::AxonResult;

use crate::{
    lsp_symbol_filter::{LspFilterLevel, LspFilterResult},
    regex_fallback::{Language, RegexFallbackFilter},
};

#[path = "filters/lsp_symbol_filter.rs"]
pub mod lsp_symbol_filter;
#[path = "filters/regex_fallback.rs"]
pub mod regex_fallback;

pub use crate::lsp_symbol_filter::{
    ExtractedSymbol, LspDocumentSymbolHit, LspDocumentSymbolProvider, LspDocumentSymbolQuery,
    LspSymbolFilter, SymbolRange,
};

/// 输出过滤统一入口。
#[derive(Debug, Default, Clone, Copy)]
pub struct OutputFilter;

impl OutputFilter {
    /// 过滤代码内容。
    ///
    /// # Errors
    ///
    /// 当过滤输入无效时返回错误。
    pub fn filter_code(
        &self,
        content: &str,
        language: Language,
        level: LspFilterLevel,
    ) -> AxonResult<LspFilterResult> {
        match level {
            LspFilterLevel::None => Ok(LspSymbolFilter.filter_none(content)),
            LspFilterLevel::Minimal => {
                let filtered = RegexFallbackFilter.filter_minimal(content, language);
                Ok(LspSymbolFilter.filter_minimal_content(content, filtered))
            }
            LspFilterLevel::Aggressive => {
                let filtered = RegexFallbackFilter.filter_aggressive(content, language);
                Ok(LspSymbolFilter.filter_minimal_content(content, filtered))
            }
        }
    }
}
