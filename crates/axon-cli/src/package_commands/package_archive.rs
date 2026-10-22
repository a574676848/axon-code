use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_crypto::sha256_hex;

const ZIP_LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const ZIP_CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const ZIP_END_SIGNATURE: u32 = 0x0605_4b50;
const ZIP_VERSION_NEEDED: u16 = 20;
const ZIP_STORE_METHOD: u16 = 0;
const ZIP_DOS_TIME: u16 = 0;
const ZIP_DOS_DATE: u16 = 0;
const ZIP_ARCHIVE_FORMAT: &str = "zip";

pub(crate) struct PackageArchiveReport {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub sidecar_path: PathBuf,
    pub format: &'static str,
}

struct ZipEntry {
    name: String,
    bytes: Vec<u8>,
    crc32: u32,
    offset: u32,
}

pub(crate) fn create_portable_zip_archive(
    package_dir: &Path,
    output_dir: &Path,
    package_name: &str,
) -> Result<PackageArchiveReport, String> {
    let archive_path = output_dir.join(format!("{package_name}.zip"));
    if archive_path.exists() {
        return Err(format!(
            "package archive 已存在: {}",
            archive_path.display()
        ));
    }

    let mut entries = collect_zip_entries(package_dir, package_name)?;
    let mut archive_bytes = Vec::new();
    for entry in &mut entries {
        entry.offset = u32_len(archive_bytes.len(), "zip local header offset")?;
        write_local_file_header(&mut archive_bytes, entry)?;
        archive_bytes.extend_from_slice(&entry.bytes);
    }

    let central_dir_offset = u32_len(archive_bytes.len(), "zip central directory offset")?;
    for entry in &entries {
        write_central_directory_header(&mut archive_bytes, entry)?;
    }
    let central_dir_size = u32_len(
        archive_bytes.len() - central_dir_offset as usize,
        "zip central directory size",
    )?;
    write_end_of_central_directory(
        &mut archive_bytes,
        u16_len(entries.len(), "zip entry count")?,
        central_dir_size,
        central_dir_offset,
    );

    fs::write(&archive_path, &archive_bytes)
        .map_err(|error| format!("写入 package archive 失败: {error}"))?;
    let archive_sha256 = sha256_hex(&archive_bytes);
    let sidecar_path = archive_path.with_extension("zip.sha256");
    let archive_name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "package archive 文件名无效".to_owned())?;
    fs::write(&sidecar_path, format!("{archive_sha256}  {archive_name}\n"))
        .map_err(|error| format!("写入 package archive sha256 失败: {error}"))?;

    Ok(PackageArchiveReport {
        path: archive_path,
        sha256: archive_sha256,
        bytes: archive_bytes.len() as u64,
        sidecar_path,
        format: ZIP_ARCHIVE_FORMAT,
    })
}

fn collect_zip_entries(package_dir: &Path, package_name: &str) -> Result<Vec<ZipEntry>, String> {
    let mut files = Vec::new();
    collect_files(package_dir, package_dir, package_name, &mut files)?;
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

fn collect_files(
    root: &Path,
    current: &Path,
    package_name: &str,
    files: &mut Vec<ZipEntry>,
) -> Result<(), String> {
    let mut children = fs::read_dir(current)
        .map_err(|error| format!("读取 package 目录失败: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取 package 目录项失败: {error}"))?;
    children.sort_by_key(std::fs::DirEntry::path);

    for child in children {
        let path = child.path();
        if path.is_dir() {
            collect_files(root, &path, package_name, files)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "package archive 路径必须位于 package 目录内".to_owned())?;
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let name = format!("{package_name}/{relative}");
        let bytes = fs::read(&path).map_err(|error| format!("读取 package 文件失败: {error}"))?;
        files.push(ZipEntry {
            name,
            crc32: crc32(&bytes),
            bytes,
            offset: 0,
        });
    }
    Ok(())
}

fn write_local_file_header(output: &mut Vec<u8>, entry: &ZipEntry) -> Result<(), String> {
    write_u32(output, ZIP_LOCAL_HEADER_SIGNATURE);
    write_u16(output, ZIP_VERSION_NEEDED);
    write_u16(output, 0);
    write_u16(output, ZIP_STORE_METHOD);
    write_u16(output, ZIP_DOS_TIME);
    write_u16(output, ZIP_DOS_DATE);
    write_u32(output, entry.crc32);
    write_u32(output, u32_len(entry.bytes.len(), "zip file size")?);
    write_u32(output, u32_len(entry.bytes.len(), "zip file size")?);
    write_u16(output, u16_len(entry.name.len(), "zip file name length")?);
    write_u16(output, 0);
    output.extend_from_slice(entry.name.as_bytes());
    Ok(())
}

fn write_central_directory_header(output: &mut Vec<u8>, entry: &ZipEntry) -> Result<(), String> {
    write_u32(output, ZIP_CENTRAL_HEADER_SIGNATURE);
    write_u16(output, ZIP_VERSION_NEEDED);
    write_u16(output, ZIP_VERSION_NEEDED);
    write_u16(output, 0);
    write_u16(output, ZIP_STORE_METHOD);
    write_u16(output, ZIP_DOS_TIME);
    write_u16(output, ZIP_DOS_DATE);
    write_u32(output, entry.crc32);
    write_u32(output, u32_len(entry.bytes.len(), "zip file size")?);
    write_u32(output, u32_len(entry.bytes.len(), "zip file size")?);
    write_u16(output, u16_len(entry.name.len(), "zip file name length")?);
    write_u16(output, 0);
    write_u16(output, 0);
    write_u16(output, 0);
    write_u16(output, 0);
    write_u32(output, 0);
    write_u32(output, entry.offset);
    output.extend_from_slice(entry.name.as_bytes());
    Ok(())
}

fn write_end_of_central_directory(
    output: &mut Vec<u8>,
    entry_count: u16,
    central_dir_size: u32,
    central_dir_offset: u32,
) {
    write_u32(output, ZIP_END_SIGNATURE);
    write_u16(output, 0);
    write_u16(output, 0);
    write_u16(output, entry_count);
    write_u16(output, entry_count);
    write_u32(output, central_dir_size);
    write_u32(output, central_dir_offset);
    write_u16(output, 0);
}

fn write_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn u16_len(value: usize, label: &str) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| format!("{label} 超过 zip32 上限"))
}

fn u32_len(value: usize, label: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{label} 超过 zip32 上限"))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
