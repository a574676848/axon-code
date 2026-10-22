//! 文档读取工具契约测试：ipynb / docx / xlsx / zip / sqlite 的抽取与有界投影。

use std::io::Write as _;
use zip::{ZipWriter, write::SimpleFileOptions};

use super::*;

#[test]
fn notebook_extracts_sources_and_text_outputs() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join("sample.ipynb");
    std::fs::write(
            &path,
            r##"{"cells":[{"cell_type":"markdown","source":["# 标题\n"]},{"cell_type":"code","source":["print(1)\n"],"outputs":[{"text":["1\n"]}]}]}"##,
        )
        .unwrap();

    let document = extract_document(&path).unwrap().unwrap();

    assert_eq!(document.kind, ReadFileContentKind::Notebook);
    assert!(document.content.contains("## Cell 1 [markdown]"));
    assert!(document.content.contains("[output]\n1"));
}

#[test]
fn docx_extracts_paragraph_text_from_fixed_member() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join("sample.docx");
    write_zip(
        &path,
        &[(
            "word/document.xml",
            r#"<w:document xmlns:w="w"><w:p><w:r><w:t>Hello &amp; Axon</w:t></w:r></w:p></w:document>"#,
        )],
    );

    let document = extract_document(&path).unwrap().unwrap();

    assert_eq!(document.kind, ReadFileContentKind::Word);
    assert_eq!(document.content, "Hello & Axon\n");
}

#[test]
fn xlsx_resolves_shared_strings_to_cell_coordinates() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join("sample.xlsx");
    write_zip(
        &path,
        &[
            ("xl/sharedStrings.xml", r"<sst><si><t>Axon</t></si></sst>"),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1"><v>42</v></c></row></sheetData></worksheet>"#,
            ),
        ],
    );

    let document = extract_document(&path).unwrap().unwrap();

    assert_eq!(document.kind, ReadFileContentKind::Spreadsheet);
    assert!(document.content.contains("A1=Axon"));
    assert!(document.content.contains("B1=42"));
}

#[test]
fn zip_read_returns_bounded_directory_listing_without_extraction() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join("sample.zip");
    write_zip(&path, &[("src/lib.rs", "fn main() {}")]);

    let document = extract_document(&path).unwrap().unwrap();

    assert_eq!(document.kind, ReadFileContentKind::Archive);
    assert!(document.content.contains("file\tsrc/lib.rs\t12 bytes"));
    assert!(!root.path().join("src/lib.rs").exists());
}

#[test]
fn sqlite_projects_objects_schema_and_rows_without_writing() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join("sample.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);\n\
                 INSERT INTO users (name) VALUES ('ada'), ('grace');",
        )
        .unwrap();
    drop(connection);
    let before = std::fs::metadata(&path).unwrap().len();

    let document = extract_document(&path).unwrap().unwrap();

    assert_eq!(document.kind, ReadFileContentKind::Sqlite);
    assert!(
        document.content.contains("# table users"),
        "{}",
        document.content
    );
    assert!(
        document.content.contains("id | name"),
        "{}",
        document.content
    );
    assert!(document.content.contains("1 | ada"), "{}", document.content);
    assert!(
        document.content.contains("2 | grace"),
        "{}",
        document.content
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        before,
        "只读投影不得修改数据库文件"
    );
    assert_eq!(document.kind.as_str(), "sqlite");
}

#[test]
fn sqlite_row_projection_is_bounded_per_table() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join("many.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY);")
        .unwrap();
    for _ in 0..(SQLITE_MAX_ROWS_PER_TABLE + 5) {
        connection
            .execute("INSERT INTO items DEFAULT VALUES", [])
            .unwrap();
    }
    drop(connection);

    let document = extract_document(&path).unwrap().unwrap();

    let rendered_rows = document
        .content
        .lines()
        .filter(|line| line.starts_with(|character: char| character.is_ascii_digit()))
        .count();
    assert_eq!(rendered_rows, SQLITE_MAX_ROWS_PER_TABLE);
}

#[test]
fn sqlite_without_user_objects_reports_a_targeted_error() {
    let root = axon_test_support::test_temp_dir();
    let path = root.path().join("empty.db");
    rusqlite::Connection::open(&path).unwrap();

    let error = match extract_document(&path) {
        Ok(_) => panic!("空数据库应报出定向错误"),
        Err(error) => error,
    };

    assert!(
        format!("{error:?}").contains("不包含用户表或视图"),
        "{error:?}"
    );
}

fn write_zip(path: &Path, entries: &[(&str, &str)]) {
    let file = File::create(path).unwrap();
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (name, content) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}
