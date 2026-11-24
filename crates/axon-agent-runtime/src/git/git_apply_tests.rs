use super::*;
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn run(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .output()
        .expect("spawn ok");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn init_repo() -> tempfile::TempDir {
    let dir = axon_test_support::test_temp_dir();
    let root = dir.path();
    let _ = run(root, &["git", "init"]);
    let _ = run(root, &["git", "config", "user.email", "axon@example.com"]);
    let _ = run(root, &["git", "config", "user.name", "Axon"]);
    dir
}

#[test]
fn extract_paths_from_patch_handles_quoted_and_dev_null_headers() {
    let diff = "diff --git \"a/hello world.txt\" \"b/hello world.txt\"\nnew file mode 100644\n--- /dev/null\n+++ b/hello world.txt\n@@ -0,0 +1 @@\n+hi\n";
    let paths = extract_paths_from_patch(diff);
    assert_eq!(paths, vec!["hello world.txt".to_string()]);

    let dev_null = "diff --git a/dev/null b/ok.txt\nnew file mode 100644\n--- /dev/null\n+++ b/ok.txt\n@@ -0,0 +1 @@\n+hi\n";
    assert_eq!(extract_paths_from_patch(dev_null), vec!["ok.txt".to_string()]);
}

#[test]
fn parse_output_unescapes_quoted_paths() {
    let stderr = "error: patch failed: \"hello\\tworld.txt\":1\n";
    let (applied, skipped, conflicted) = parse_git_apply_output("", stderr);
    assert_eq!(applied, Vec::<String>::new());
    assert_eq!(conflicted, Vec::<String>::new());
    assert_eq!(skipped, vec!["hello\tworld.txt".to_string()]);
}

#[test]
fn apply_add_success() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();

    let diff = "diff --git a/hello.txt b/hello.txt\nnew file mode 100644\n--- /dev/null\n+++ b/hello.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n";
    let req = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: false,
    };
    let r = apply_git_patch(&req).expect("run apply");
    assert_eq!(r.exit_code, 0, "exit code 0");
    assert!(root.join("hello.txt").exists());
}

#[test]
fn preflight_blocks_partial_changes() {
    let _g = env_lock().lock().unwrap();
    let repo = init_repo();
    let root = repo.path();

    let diff = "diff --git a/ok.txt b/ok.txt\nnew file mode 100644\n--- /dev/null\n+++ b/ok.txt\n@@ -0,0 +1,2 @@\n+alpha\n+beta\n";

    let req1 = ApplyGitRequest {
        cwd: root.to_path_buf(),
        diff: diff.to_string(),
        revert: false,
        preflight: true,
    };
    let r1 = apply_git_patch(&req1).expect("preflight apply");
    assert_ne!(r1.exit_code, 0, "preflight reports failure");
    assert!(
        !root.join("ok.txt").exists(),
        "preflight must prevent adding ok.txt"
    );
    assert!(
        r1.cmd_for_log.contains("--check"),
        "preflight path recorded --check"
    );
}
