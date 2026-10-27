//! 注入构建号：优先取环境变量，其次取 git 短提交，最后回落 `dev`。

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=AXON_VOICE_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
    let commit = std::env::var("AXON_VOICE_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_commit)
        .unwrap_or_else(|| "dev".to_owned());
    println!("cargo:rustc-env=AXON_VOICE_BUILD_COMMIT={commit}");
}

fn git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!commit.is_empty()).then_some(commit)
}
