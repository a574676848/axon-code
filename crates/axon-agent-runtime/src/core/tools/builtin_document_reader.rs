//! 非纯文本文件的安全、有界文本投影。

use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
};

use axon_domain::{AxonError, AxonResult};
use flate2::read::GzDecoder;
use quick_xml::{Reader, XmlVersion, events::Event};
use serde_json::Value;
use tar::Archive;
use zip::ZipArchive;

const DOCUMENT_MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;
const DOCUMENT_MAX_EXTRACTED_BYTES: usize = 2 * 1024 * 1024;
const OFFICE_MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const ARCHIVE_MAX_ENTRIES: usize = 1_000;
const ARCHIVE_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// SQLite 投影：最多列出的表/视图数量。
const SQLITE_MAX_TABLES: usize = 50;
/// SQLite 投影：每张表最多展示的数据行数。
const SQLITE_MAX_ROWS_PER_TABLE: usize = 20;
/// SQLite 投影：每行最多展示的列数。
const SQLITE_MAX_COLUMNS: usize = 40;
/// SQLite 投影：单元格文本的最大字符数。
const SQLITE_MAX_CELL_CHARS: usize = 200;
const DOCUMENT_TRUNCATION_MARKER: &str = "\n...[派生文本已达到上限]";

/// `read_file` 实际返回的内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFileContentKind {
    /// 原始 UTF-8 文本。
    Text,
    /// Jupyter Notebook 派生文本。
    Notebook,
    /// PDF 提取文本。
    Pdf,
    /// Word 文档提取文本。
    Word,
    /// Excel 工作簿提取文本。
    Spreadsheet,
    /// PowerPoint 演示文稿提取文本。
    Presentation,
    /// 压缩包安全目录清单。
    Archive,
    /// SQLite 数据库的只读结构 + 有界数据投影。
    Sqlite,
}

impl ReadFileContentKind {
    /// 返回 provider-visible 稳定名称。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Notebook => "notebook",
            Self::Pdf => "pdf",
            Self::Word => "word",
            Self::Spreadsheet => "spreadsheet",
            Self::Presentation => "presentation",
            Self::Archive => "archive",
            Self::Sqlite => "sqlite",
        }
    }
}

pub(super) struct ExtractedDocument {
    pub(super) content: String,
    pub(super) kind: ReadFileContentKind,
}

pub(super) fn extract_document(path: &Path) -> AxonResult<Option<ExtractedDocument>> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let kind = match extension.as_str() {
        "ipynb" => ReadFileContentKind::Notebook,
        "pdf" => ReadFileContentKind::Pdf,
        "docx" => ReadFileContentKind::Word,
        "xlsx" => ReadFileContentKind::Spreadsheet,
        "pptx" => ReadFileContentKind::Presentation,
        "zip" | "tar" | "tgz" | "gz" => ReadFileContentKind::Archive,
        "db" | "sqlite" | "sqlite3" => ReadFileContentKind::Sqlite,
        _ => return Ok(None),
    };
    enforce_input_limit(path)?;
    let content = match kind {
        ReadFileContentKind::Notebook => extract_notebook(path)?,
        ReadFileContentKind::Pdf => extract_pdf(path)?,
        ReadFileContentKind::Word => extract_docx(path)?,
        ReadFileContentKind::Spreadsheet => extract_xlsx(path)?,
        ReadFileContentKind::Presentation => extract_pptx(path)?,
        ReadFileContentKind::Archive => list_archive(path, &extension)?,
        ReadFileContentKind::Sqlite => extract_sqlite(path)?,
        ReadFileContentKind::Text => unreachable!("文本文件不进入派生文档读取器"),
    };
    Ok(Some(ExtractedDocument {
        content: bounded_document_text(content),
        kind,
    }))
}

fn enforce_input_limit(path: &Path) -> AxonResult<()> {
    let size = path
        .metadata()
        .map_err(|_| document_error("无法读取文件元数据"))?
        .len();
    if size > DOCUMENT_MAX_INPUT_BYTES {
        return Err(document_error("文件超过多格式读取大小上限"));
    }
    Ok(())
}

fn extract_notebook(path: &Path) -> AxonResult<String> {
    let raw =
        std::fs::read_to_string(path).map_err(|_| document_error("Notebook 不是 UTF-8 JSON"))?;
    let root: Value =
        serde_json::from_str(&raw).map_err(|_| document_error("Notebook JSON 无效"))?;
    let cells = root
        .get("cells")
        .and_then(Value::as_array)
        .ok_or_else(|| document_error("Notebook 缺少 cells"))?;
    let mut output = String::new();
    for (index, cell) in cells.iter().enumerate() {
        let cell_type = cell
            .get("cell_type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        output.push_str(&format!("## Cell {} [{}]\n", index + 1, cell_type));
        output.push_str(&json_text(cell.get("source")));
        if cell_type == "code" {
            append_notebook_outputs(&mut output, cell.get("outputs"));
        }
        output.push_str("\n\n");
        if output.len() >= DOCUMENT_MAX_EXTRACTED_BYTES {
            break;
        }
    }
    Ok(output)
}

fn append_notebook_outputs(output: &mut String, value: Option<&Value>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let text = json_text(item.get("text"));
        let plain = item
            .pointer("/data/text/plain")
            .map_or_else(String::new, |value| json_text(Some(value)));
        let error = match (
            item.get("ename").and_then(Value::as_str),
            item.get("evalue").and_then(Value::as_str),
        ) {
            (Some(name), Some(message)) => format!("{name}: {message}"),
            _ => String::new(),
        };
        let body = [text, plain, error]
            .into_iter()
            .find(|value| !value.trim().is_empty());
        if let Some(body) = body {
            output.push_str("\n[output]\n");
            output.push_str(&body);
        }
    }
}

fn json_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

fn extract_pdf(path: &Path) -> AxonResult<String> {
    let text = pdf_extract::extract_text(path).map_err(|_| document_error("PDF 文本提取失败"))?;
    if text.trim().is_empty() {
        return Err(document_error("PDF 未包含可提取文本；扫描型 PDF 需要 OCR"));
    }
    Ok(text)
}

fn extract_docx(path: &Path) -> AxonResult<String> {
    let mut archive = open_zip(path)?;
    let xml = read_zip_text(&mut archive, "word/document.xml")?;
    extract_xml_text(&xml, &["t"], &["p"])
}

fn extract_pptx(path: &Path) -> AxonResult<String> {
    let mut archive = open_zip(path)?;
    let mut names = zip_names(&mut archive, "ppt/slides/slide", ".xml");
    names.sort_by_key(|name| numeric_suffix(name));
    let mut output = String::new();
    for (index, name) in names.into_iter().enumerate() {
        let xml = read_zip_text(&mut archive, &name)?;
        output.push_str(&format!("# Slide {}\n", index + 1));
        output.push_str(&extract_xml_text(&xml, &["t"], &["p"])?);
        output.push_str("\n\n");
    }
    Ok(output)
}

fn extract_xlsx(path: &Path) -> AxonResult<String> {
    let mut archive = open_zip(path)?;
    let shared = match read_zip_text(&mut archive, "xl/sharedStrings.xml") {
        Ok(xml) => (extract_xml_text(&xml, &["t"], &["si"])?)
            .lines()
            .map(ToOwned::to_owned)
            .collect(),
        Err(_) => Vec::new(),
    };
    let mut names = zip_names(&mut archive, "xl/worksheets/sheet", ".xml");
    names.sort_by_key(|name| numeric_suffix(name));
    let mut output = String::new();
    for (index, name) in names.into_iter().enumerate() {
        let xml = read_zip_text(&mut archive, &name)?;
        output.push_str(&format!("# Sheet {}\n", index + 1));
        output.push_str(&extract_worksheet(&xml, &shared)?);
        output.push('\n');
    }
    Ok(output)
}

fn extract_xml_text(xml: &str, text_tags: &[&str], break_tags: &[&str]) -> AxonResult<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut output = String::new();
    let mut capture = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => capture = text_tags.contains(&event.local_name().as_ref()),
            Ok(Event::Text(text)) if capture => output.push_str(&decode_xml_text(&text)?),
            Ok(Event::GeneralRef(reference)) if capture => {
                output.push_str(&decode_xml_reference(&reference)?);
            }
            Ok(Event::End(event)) => {
                let name = event.local_name();
                if text_tags.contains(&name.as_ref()) {
                    capture = false;
                }
                if break_tags.contains(&name.as_ref()) && !output.ends_with('\n') {
                    output.push('\n');
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(document_error("Office XML 无效")),
            _ => {}
        }
    }
    Ok(output)
}

fn extract_worksheet(xml: &str, shared: &[String]) -> AxonResult<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut output = String::new();
    let mut cell_ref = String::new();
    let mut cell_type = String::new();
    let mut value = String::new();
    let mut capture = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.local_name().as_ref() == "c" => {
                cell_ref = xml_attribute(&reader, &event, "r")?;
                cell_type = xml_attribute(&reader, &event, "t")?;
                value.clear();
            }
            Ok(Event::Start(event)) if matches!(event.local_name().as_ref(), "v" | "t") => {
                capture = true;
            }
            Ok(Event::Text(text)) if capture => value.push_str(&decode_xml_text(&text)?),
            Ok(Event::GeneralRef(reference)) if capture => {
                value.push_str(&decode_xml_reference(&reference)?);
            }
            Ok(Event::End(event)) if matches!(event.local_name().as_ref(), "v" | "t") => {
                capture = false;
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == "c" => {
                append_cell(&mut output, &cell_ref, &cell_type, &value, shared);
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(document_error("Excel worksheet XML 无效")),
            _ => {}
        }
    }
    Ok(output)
}

fn xml_attribute(
    _reader: &Reader<&[u8]>,
    event: &quick_xml::events::BytesStart<'_>,
    key: &str,
) -> AxonResult<String> {
    for attribute in event.attributes().flatten() {
        if attribute.key.local_name().as_ref() == key {
            return attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|value| value.into_owned())
                .map_err(|_| document_error("Office XML 属性无效"));
        }
    }
    Ok(String::new())
}

fn append_cell(
    output: &mut String,
    reference: &str,
    cell_type: &str,
    value: &str,
    shared: &[String],
) {
    if reference.is_empty() || value.is_empty() {
        return;
    }
    let resolved = if cell_type == "s" {
        value
            .parse::<usize>()
            .ok()
            .and_then(|index| shared.get(index))
            .map_or(value, String::as_str)
    } else {
        value
    };
    output.push_str(reference);
    output.push('=');
    output.push_str(resolved);
    output.push('\n');
}

fn decode_xml_text(text: &quick_xml::events::BytesText<'_>) -> AxonResult<String> {
    Ok(text.xml_content(XmlVersion::Implicit1_0).into_owned())
}

fn decode_xml_reference(reference: &quick_xml::events::BytesRef<'_>) -> AxonResult<String> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|_| document_error("Office XML 字符引用无效"))?
    {
        return Ok(character.to_string());
    }
    let name = reference.xml_content(XmlVersion::Implicit1_0);
    quick_xml::escape::resolve_predefined_entity(&name)
        .map(ToOwned::to_owned)
        .ok_or_else(|| document_error("Office XML 包含不支持的外部实体"))
}

fn open_zip(path: &Path) -> AxonResult<ZipArchive<File>> {
    let file = File::open(path).map_err(|_| document_error("无法打开 ZIP 容器"))?;
    ZipArchive::new(file).map_err(|_| document_error("ZIP 容器无效"))
}

fn read_zip_text<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> AxonResult<String> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| document_error("Office 容器缺少必要成员"))?;
    if file.size() > OFFICE_MAX_ENTRY_BYTES {
        return Err(document_error("Office XML 成员超过大小上限"));
    }
    let mut text = String::new();
    file.by_ref()
        .take(OFFICE_MAX_ENTRY_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| document_error("Office XML 不是 UTF-8"))?;
    Ok(text)
}

fn zip_names<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    prefix: &str,
    suffix: &str,
) -> Vec<String> {
    (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|file| file.name().to_owned())
        })
        .filter(|name| name.starts_with(prefix) && name.ends_with(suffix))
        .collect()
}

fn numeric_suffix(name: &str) -> usize {
    name.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(usize::MAX)
}

fn list_archive(path: &Path, extension: &str) -> AxonResult<String> {
    match extension {
        "zip" => list_zip(path),
        "tar" => list_tar(File::open(path).map_err(|_| document_error("无法打开 TAR"))?),
        "tgz" | "gz" => list_tar(GzDecoder::new(
            File::open(path).map_err(|_| document_error("无法打开 GZip"))?,
        )),
        _ => Err(document_error("不支持的压缩格式")),
    }
}

fn list_zip(path: &Path) -> AxonResult<String> {
    let mut archive = open_zip(path)?;
    let mut output = String::new();
    let mut total = 0u64;
    for index in 0..archive.len().min(ARCHIVE_MAX_ENTRIES) {
        let file = archive
            .by_index(index)
            .map_err(|_| document_error("ZIP 条目无效"))?;
        total = total.saturating_add(file.size());
        if total > ARCHIVE_MAX_TOTAL_BYTES {
            return Err(document_error("ZIP 解压总量超过安全上限"));
        }
        let kind = if file.is_dir() { "dir" } else { "file" };
        output.push_str(&format!("{kind}\t{}\t{} bytes\n", file.name(), file.size()));
    }
    if archive.len() > ARCHIVE_MAX_ENTRIES {
        output.push_str("...[条目数量已达到上限]\n");
    }
    Ok(output)
}

fn list_tar<R: Read>(reader: R) -> AxonResult<String> {
    let mut archive = Archive::new(reader);
    let mut output = String::new();
    let mut total = 0u64;
    for (index, entry) in archive
        .entries()
        .map_err(|_| document_error("TAR 容器无效"))?
        .enumerate()
    {
        if index >= ARCHIVE_MAX_ENTRIES {
            output.push_str("...[条目数量已达到上限]\n");
            break;
        }
        let entry = entry.map_err(|_| document_error("TAR 条目无效"))?;
        let size = entry.size();
        total = total.saturating_add(size);
        if total > ARCHIVE_MAX_TOTAL_BYTES {
            return Err(document_error("TAR 解压总量超过安全上限"));
        }
        let path = entry.path().map_err(|_| document_error("TAR 路径无效"))?;
        output.push_str(&format!("entry\t{}\t{size} bytes\n", path.display()));
    }
    Ok(output)
}

/// 以只读方式投影 SQLite 数据库：对象清单 + schema + 有界数据行。
///
/// 只读打开（`SQLITE_OPEN_READ_ONLY`），绝不创建、修改或写入目标文件；表、行、列与
/// 单元格长度全部有界，输出再经 `bounded_document_text` 收敛到统一上限。
fn extract_sqlite(path: &Path) -> AxonResult<String> {
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| document_error("SQLite 数据库无法以只读方式打开"))?;
    let objects = sqlite_objects(&connection)?;
    if objects.is_empty() {
        return Err(document_error("SQLite 数据库不包含用户表或视图"));
    }
    let mut output = String::new();
    for (index, (kind, name)) in objects.iter().enumerate() {
        if index >= SQLITE_MAX_TABLES {
            output.push_str("...[对象数量已达到上限]\n");
            break;
        }
        output.push_str(&format!("# {kind} {name}\n"));
        if let Some(schema) = sqlite_schema(&connection, name)? {
            output.push_str(&schema);
            output.push('\n');
        }
        output.push_str(&sqlite_rows(&connection, name)?);
        output.push('\n');
        if output.len() >= DOCUMENT_MAX_EXTRACTED_BYTES {
            break;
        }
    }
    Ok(output)
}

fn sqlite_objects(connection: &rusqlite::Connection) -> AxonResult<Vec<(String, String)>> {
    let mut statement = connection
        .prepare(
            "SELECT type, name FROM sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
             ORDER BY type, name",
        )
        .map_err(|_| document_error("SQLite 对象清单查询失败"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| document_error("SQLite 对象清单查询失败"))?;
    let mut objects = Vec::new();
    for row in rows {
        objects.push(row.map_err(|_| document_error("SQLite 对象清单行无效"))?);
    }
    Ok(objects)
}

fn sqlite_schema(connection: &rusqlite::Connection, name: &str) -> AxonResult<Option<String>> {
    let mut statement = connection
        .prepare("SELECT sql FROM sqlite_master WHERE name = ?1")
        .map_err(|_| document_error("SQLite schema 查询失败"))?;
    let mut rows = statement
        .query([name])
        .map_err(|_| document_error("SQLite schema 查询失败"))?;
    match rows.next() {
        Ok(Some(row)) => Ok(row.get::<_, Option<String>>(0).unwrap_or(None)),
        Ok(None) => Ok(None),
        Err(_) => Err(document_error("SQLite schema 行无效")),
    }
}

fn sqlite_rows(connection: &rusqlite::Connection, name: &str) -> AxonResult<String> {
    // 对象名来自 sqlite_master，按标识符规则转义后再内联，避免被当作 SQL 片段。
    let quoted = name.replace('"', "\"\"");
    let mut statement = connection
        .prepare(&format!(
            "SELECT * FROM \"{quoted}\" LIMIT {SQLITE_MAX_ROWS_PER_TABLE}"
        ))
        .map_err(|_| document_error("SQLite 数据查询失败"))?;
    let column_count = statement.column_count().min(SQLITE_MAX_COLUMNS);
    let columns = statement
        .column_names()
        .into_iter()
        .take(column_count)
        .collect::<Vec<_>>()
        .join(" | ");
    let mut output = format!("columns: {columns}\n");
    let mut rows = statement
        .query([])
        .map_err(|_| document_error("SQLite 数据查询失败"))?;
    while let Some(row) = rows
        .next()
        .map_err(|_| document_error("SQLite 数据行无效"))?
    {
        let cells = (0..column_count)
            .map(|index| sqlite_cell(row, index))
            .collect::<Vec<_>>();
        output.push_str(&cells.join(" | "));
        output.push('\n');
    }
    Ok(output)
}

fn sqlite_cell(row: &rusqlite::Row<'_>, index: usize) -> String {
    let Ok(value) = row.get_ref(index) else {
        return "?".to_owned();
    };
    let rendered = match value {
        rusqlite::types::ValueRef::Null => "NULL".to_owned(),
        rusqlite::types::ValueRef::Integer(number) => number.to_string(),
        rusqlite::types::ValueRef::Real(number) => number.to_string(),
        rusqlite::types::ValueRef::Text(bytes) => {
            String::from_utf8_lossy(bytes).replace('\n', "\\n")
        }
        rusqlite::types::ValueRef::Blob(bytes) => format!("<blob {} bytes>", bytes.len()),
    };
    truncate_sqlite_cell(rendered)
}

fn truncate_sqlite_cell(value: String) -> String {
    if value.chars().count() <= SQLITE_MAX_CELL_CHARS {
        return value;
    }
    let truncated = value
        .chars()
        .take(SQLITE_MAX_CELL_CHARS)
        .collect::<String>();
    format!("{truncated}…")
}

fn bounded_document_text(mut content: String) -> String {
    if content.len() <= DOCUMENT_MAX_EXTRACTED_BYTES {
        return content;
    }
    let mut end = DOCUMENT_MAX_EXTRACTED_BYTES;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    content.truncate(end);
    content.push_str(DOCUMENT_TRUNCATION_MARKER);
    content
}

fn document_error(message: &str) -> AxonError {
    AxonError::runtime_failure(format!("read_file 多格式解析失败：{message}"))
}

#[cfg(test)]
#[path = "builtin_document_reader_tests.rs"]
mod builtin_document_reader_tests;
