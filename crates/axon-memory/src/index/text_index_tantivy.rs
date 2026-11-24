//! Tantivy 文本检索索引。

use axon_domain::{AxonError, AxonResult, MemoryId};
use tantivy::{
    Index, IndexWriter, TantivyDocument,
    collector::TopDocs,
    directory::MmapDirectory,
    query::QueryParser,
    schema::{
        Field, INDEXED, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions,
        Value as _,
    },
    tokenizer::{Token, TokenStream, Tokenizer},
};

use crate::MemoryConfig;

/// Tantivy writer 默认内存预算。
pub const MEMORY_TANTIVY_WRITER_HEAP_BYTES: usize = 50_000_000;
/// Tantivy schema 版本。
pub const MEMORY_TANTIVY_SCHEMA_VERSION: u32 = 2;
/// Tantivy 默认召回数量。
pub const MEMORY_TANTIVY_DEFAULT_LIMIT: usize = 16;

const FIELD_MEMORY_ID: &str = "memory_id";
const FIELD_CONTENT_REF: &str = "content_ref";
const FIELD_EVIDENCE_REF: &str = "evidence_ref";
const FIELD_TOMBSTONE_REF: &str = "tombstone_ref";
const FIELD_MEMORY_SUMMARY: &str = "memory_summary";
const TOKENIZER_MEMORY_REF: &str = "memory_ref";

/// Tantivy 写入文档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTextDocument {
    /// Memory ID。
    pub memory_id: MemoryId,
    /// 内容引用。
    pub content_ref: Option<String>,
    /// 证据引用。
    pub evidence_ref: Option<String>,
    /// 墓碑引用。
    pub tombstone_ref: Option<String>,
    /// 可检索的脱敏记忆摘要。
    pub memory_summary: Option<String>,
}

/// Tantivy 命中。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryTextHit {
    /// Memory ID。
    pub memory_id: MemoryId,
    /// BM25 分数。
    pub score: f32,
}

/// Memory 文本索引。
pub struct MemoryTextIndex {
    index: Index,
    fields: MemoryTextFields,
}

#[derive(Debug, Clone, Copy)]
struct MemoryTextFields {
    memory_id: Field,
    content_ref: Field,
    evidence_ref: Field,
    tombstone_ref: Field,
    memory_summary: Field,
}

impl MemoryTextIndex {
    /// 打开或创建 Tantivy 索引。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        let (schema, fields) = build_schema();
        reset_incompatible_index(&config.tantivy_index_dir, &schema)?;
        std::fs::create_dir_all(&config.tantivy_index_dir)
            .map_err(|_| AxonError::memory_persistence("无法创建 Memory Tantivy 目录"))?;
        let directory = MmapDirectory::open(&config.tantivy_index_dir)
            .map_err(|_| AxonError::memory_persistence("无法打开 Memory Tantivy 目录"))?;
        let index = Index::open_or_create(directory, schema)
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 索引初始化失败"))?;
        index
            .tokenizers()
            .register(TOKENIZER_MEMORY_REF, MemoryRefTokenizer);
        Ok(Self { index, fields })
    }

    /// 写入或重建单条 Memory 文本索引。
    pub fn upsert_document(&self, document: &MemoryTextDocument) -> AxonResult<()> {
        self.upsert_documents([document])
    }

    /// 批量写入或重建 Memory 文本索引，并只提交一次 writer。
    pub fn upsert_documents<'a>(
        &self,
        documents: impl IntoIterator<Item = &'a MemoryTextDocument>,
    ) -> AxonResult<()> {
        let documents = documents.into_iter().collect::<Vec<_>>();
        if documents.is_empty() {
            return Ok(());
        }
        for document in &documents {
            validate_document(document)?;
        }
        let mut writer = self.writer()?;
        for document in documents {
            writer.delete_term(tantivy::Term::from_field_u64(
                self.fields.memory_id,
                document.memory_id.0,
            ));
            writer
                .add_document(self.tantivy_document(document))
                .map_err(|_| AxonError::memory_persistence("Memory Tantivy 文档写入失败"))?;
        }
        writer
            .commit()
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 文档提交失败"))?;
        Ok(())
    }

    /// 按 Memory ID 批量删除文本投影。
    pub fn delete_documents(&self, memory_ids: &[MemoryId]) -> AxonResult<()> {
        if memory_ids.is_empty() {
            return Ok(());
        }
        if memory_ids.iter().any(|memory_id| memory_id.0 == 0) {
            return Err(AxonError::memory_persistence("Memory Tantivy 删除 ID 无效"));
        }
        let mut writer = self.writer()?;
        for memory_id in memory_ids {
            writer.delete_term(tantivy::Term::from_field_u64(
                self.fields.memory_id,
                memory_id.0,
            ));
        }
        writer
            .commit()
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 删除提交失败"))?;
        Ok(())
    }

    /// BM25 文本召回。
    pub fn search(&self, query_text: &str, limit: usize) -> AxonResult<Vec<MemoryTextHit>> {
        if query_text.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let reader = self
            .index
            .reader()
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy reader 初始化失败"))?;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(
            &self.index,
            vec![
                self.fields.content_ref,
                self.fields.evidence_ref,
                self.fields.tombstone_ref,
                self.fields.memory_summary,
            ],
        );
        let query = parser
            .parse_query(query_text)
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 查询解析失败"))?;
        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 查询失败"))?;
        top_docs
            .into_iter()
            .map(|(score, address)| {
                let doc: TantivyDocument = searcher.doc(address)?;
                let memory_id = doc
                    .get_first(self.fields.memory_id)
                    .and_then(|value| value.as_u64())
                    .ok_or(tantivy::TantivyError::InvalidArgument(
                        "Memory Tantivy memory_id 缺失".to_owned(),
                    ))?;
                Ok(MemoryTextHit {
                    memory_id: MemoryId(memory_id),
                    score,
                })
            })
            .collect::<Result<Vec<_>, tantivy::TantivyError>>()
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 结果解析失败"))
    }

    /// 列出索引中实际存在的 memory_id，按升序有界返回。
    ///
    /// 用于反向核对：只按权威表重建文档无法发现「记录已不存在但文档仍在」的残留。
    pub fn indexed_memory_ids(&self, limit: usize) -> AxonResult<Vec<MemoryId>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let reader = self
            .index
            .reader()
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy reader 初始化失败"))?;
        let searcher = reader.searcher();
        let top_docs = searcher
            .search(
                &tantivy::query::AllQuery,
                &TopDocs::with_limit(limit).order_by_score(),
            )
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 文档枚举失败"))?;
        let mut memory_ids = top_docs
            .into_iter()
            .map(|(_, address)| {
                let doc: TantivyDocument = searcher.doc(address)?;
                doc.get_first(self.fields.memory_id)
                    .and_then(|value| value.as_u64())
                    .map(MemoryId)
                    .ok_or(tantivy::TantivyError::InvalidArgument(
                        "Memory Tantivy memory_id 缺失".to_owned(),
                    ))
            })
            .collect::<Result<Vec<_>, tantivy::TantivyError>>()
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy 文档枚举解析失败"))?;
        memory_ids.sort_unstable_by_key(|memory_id| memory_id.0);
        memory_ids.dedup();
        Ok(memory_ids)
    }

    fn writer(&self) -> AxonResult<IndexWriter> {
        self.index
            .writer(MEMORY_TANTIVY_WRITER_HEAP_BYTES)
            .map_err(|_| AxonError::memory_persistence("Memory Tantivy writer 初始化失败"))
    }

    fn tantivy_document(&self, document: &MemoryTextDocument) -> TantivyDocument {
        let mut tantivy_doc = TantivyDocument::default();
        tantivy_doc.add_u64(self.fields.memory_id, document.memory_id.0);
        add_optional_text(
            &mut tantivy_doc,
            self.fields.content_ref,
            document.content_ref.as_deref(),
        );
        add_optional_text(
            &mut tantivy_doc,
            self.fields.evidence_ref,
            document.evidence_ref.as_deref(),
        );
        add_optional_text(
            &mut tantivy_doc,
            self.fields.tombstone_ref,
            document.tombstone_ref.as_deref(),
        );
        add_optional_text(
            &mut tantivy_doc,
            self.fields.memory_summary,
            document.memory_summary.as_deref(),
        );
        tantivy_doc
    }
}

fn reset_incompatible_index(path: &std::path::Path, expected: &Schema) -> AxonResult<()> {
    if !path.exists() {
        return Ok(());
    }
    let directory = MmapDirectory::open(path)
        .map_err(|_| AxonError::memory_persistence("无法探测 Memory Tantivy schema"))?;
    let incompatible = Index::open(directory).is_ok_and(|index| index.schema() != *expected);
    if incompatible {
        std::fs::remove_dir_all(path)
            .map_err(|_| AxonError::memory_persistence("无法重建 Memory Tantivy v2 索引"))?;
    }
    Ok(())
}

fn build_schema() -> (Schema, MemoryTextFields) {
    let mut builder = Schema::builder();
    let memory_id = builder.add_u64_field(FIELD_MEMORY_ID, INDEXED | STORED);
    let text_options = TextOptions::default().set_stored().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TOKENIZER_MEMORY_REF)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let content_ref = builder.add_text_field(FIELD_CONTENT_REF, text_options.clone());
    let evidence_ref = builder.add_text_field(FIELD_EVIDENCE_REF, text_options.clone());
    let tombstone_ref = builder.add_text_field(FIELD_TOMBSTONE_REF, text_options.clone());
    let memory_summary = builder.add_text_field(FIELD_MEMORY_SUMMARY, text_options);
    (
        builder.build(),
        MemoryTextFields {
            memory_id,
            content_ref,
            evidence_ref,
            tombstone_ref,
            memory_summary,
        },
    )
}

fn validate_document(document: &MemoryTextDocument) -> AxonResult<()> {
    if document.memory_id.0 == 0 {
        return Err(AxonError::memory_persistence(
            "Memory Tantivy memory_id 无效",
        ));
    }
    if [
        document.content_ref.as_deref(),
        document.evidence_ref.as_deref(),
        document.tombstone_ref.as_deref(),
        document.memory_summary.as_deref(),
    ]
    .into_iter()
    .flatten()
    .all(|value| value.trim().is_empty())
    {
        return Err(AxonError::memory_persistence(
            "Memory Tantivy 文档必须包含可检索引用",
        ));
    }
    Ok(())
}

fn add_optional_text(doc: &mut TantivyDocument, field: Field, value: Option<&str>) {
    if let Some(value) = value
        && !value.trim().is_empty()
    {
        doc.add_text(field, value);
    }
}

#[derive(Clone, Default)]
struct MemoryRefTokenizer;

impl Tokenizer for MemoryRefTokenizer {
    type TokenStream<'a> = MemoryRefTokenStream;

    fn token_stream<'a>(&mut self, text: &'a str) -> Self::TokenStream<'a> {
        MemoryRefTokenStream {
            tokens: tokenize_memory_ref(text),
            cursor: 0,
        }
    }
}

fn tokenize_memory_ref(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut ascii_start = None;
    for (offset, ch) in text.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            ascii_start.get_or_insert(offset);
            continue;
        }
        flush_ascii_token(text, &mut tokens, &mut ascii_start, offset);
        if is_cjk(ch) {
            push_token(&mut tokens, offset, offset + ch.len_utf8(), ch.to_string());
        }
    }
    flush_ascii_token(text, &mut tokens, &mut ascii_start, text.len());
    push_cjk_bigrams(text, &mut tokens);
    tokens.sort_by_key(|token| (token.offset_from, token.offset_to, token.text.clone()));
    for (position, token) in tokens.iter_mut().enumerate() {
        token.position = position;
    }
    tokens
}

fn flush_ascii_token(
    text: &str,
    tokens: &mut Vec<Token>,
    ascii_start: &mut Option<usize>,
    end: usize,
) {
    if let Some(start) = ascii_start.take()
        && start < end
    {
        push_token(tokens, start, end, text[start..end].to_ascii_lowercase());
    }
}

fn push_cjk_bigrams(text: &str, tokens: &mut Vec<Token>) {
    let chars = text.char_indices().collect::<Vec<_>>();
    for window in chars.windows(2) {
        let [(left_offset, left), (right_offset, right)] = window else {
            continue;
        };
        if is_cjk(*left) && is_cjk(*right) {
            let end = right_offset + right.len_utf8();
            push_token(
                tokens,
                *left_offset,
                end,
                text[*left_offset..end].to_owned(),
            );
        }
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{20000}'..='\u{2a6df}'
            | '\u{2a700}'..='\u{2b73f}'
            | '\u{2b740}'..='\u{2b81f}'
            | '\u{2b820}'..='\u{2ceaf}'
    )
}

fn push_token(tokens: &mut Vec<Token>, offset_from: usize, offset_to: usize, text: String) {
    if !text.trim().is_empty() {
        tokens.push(Token {
            offset_from,
            offset_to,
            position: 0,
            text,
            position_length: 1,
        });
    }
}

struct MemoryRefTokenStream {
    tokens: Vec<Token>,
    cursor: usize,
}

impl TokenStream for MemoryRefTokenStream {
    fn advance(&mut self) -> bool {
        if self.cursor < self.tokens.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.cursor.saturating_sub(1)]
    }

    fn token_mut(&mut self) -> &mut Token {
        let index = self.cursor.saturating_sub(1);
        &mut self.tokens[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_summary_is_semantically_searchable() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-memory-summary-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = MemoryConfig::from_config_dir(
            &root,
            false,
            crate::MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
        )
        .unwrap();
        let index = MemoryTextIndex::open(&config).unwrap();
        index
            .upsert_document(&MemoryTextDocument {
                memory_id: MemoryId(42),
                content_ref: Some("memory-ref:encoding-policy".to_owned()),
                evidence_ref: None,
                tombstone_ref: None,
                memory_summary: Some("Rust 文件必须使用 UTF-8 无 BOM".to_owned()),
            })
            .unwrap();

        let hits = index.search("Rust UTF-8", 4).unwrap();

        assert_eq!(hits.first().map(|hit| hit.memory_id), Some(MemoryId(42)));
        drop(index);
        let _ = std::fs::remove_dir_all(root);
    }
}
