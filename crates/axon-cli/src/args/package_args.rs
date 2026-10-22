use std::path::PathBuf;

use clap::Args;

use crate::OutputFormat;

/// `axon package` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct PackageCommand {
    /// 目标平台 triple；默认使用当前平台推导值。
    #[arg(long, conflicts_with = "verify_manifest")]
    pub target: Option<String>,
    /// 已构建的 axon 二进制路径；默认读取 target/release/axon。
    #[arg(long, conflicts_with = "verify_manifest")]
    pub binary: Option<PathBuf>,
    /// 输出目录；默认 dist。
    #[arg(long, conflicts_with = "verify_manifest")]
    pub output_dir: Option<PathBuf>,
    /// 同步生成 portable ZIP 归档和 `.zip.sha256` 旁路校验文件。
    #[arg(long, conflicts_with = "verify_manifest")]
    pub archive: bool,
    /// 校验已有 portable package manifest。
    #[arg(long)]
    pub verify_manifest: Option<PathBuf>,
    /// 校验 manifest 时跳过 `--version` smoke。
    #[arg(long, requires = "verify_manifest")]
    pub no_smoke: bool,
    /// 输出格式。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}
