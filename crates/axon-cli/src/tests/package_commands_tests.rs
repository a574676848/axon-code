use super::*;

#[test]
fn package_plan_rejects_invalid_target() {
    let command = PackageCommand {
        target: Some("../bad".to_owned()),
        binary: None,
        output_dir: None,
        archive: false,
        verify_manifest: None,
        no_smoke: false,
        format: None,
    };

    assert!(PackagePlan::from_command(&command).is_err());
}

#[test]
fn package_command_creates_portable_manifest_without_remote_or_self_replace() {
    let root = temp_root("package-manifest");
    let binary = root.join(package_binary_name(&current_target_triple()));
    fs::create_dir_all(&root).unwrap();
    fs::write(&binary, b"axon-fixture").unwrap();
    fs::write(
        root.join(package_launcher_name("x86_64-unknown-linux-gnu")),
        b"launcher",
    )
    .unwrap();
    let output_dir = root.join("dist");
    let command = PackageCommand {
        target: Some("x86_64-unknown-linux-gnu".to_owned()),
        binary: Some(binary.clone()),
        output_dir: Some(output_dir),
        archive: false,
        verify_manifest: None,
        no_smoke: false,
        format: None,
    };

    let report = PackagePlan::from_command(&command)
        .unwrap()
        .execute()
        .unwrap();

    assert_eq!(report.status, PACKAGE_STATUS_READY);
    assert_eq!(report.kind, PACKAGE_KIND_PORTABLE);
    assert!(!report.auto_install_allowed);
    assert!(!report.self_replace_allowed);
    assert!(!report.remote_trigger_allowed);
    assert!(!report.network_publish_allowed);
    assert!(Path::new(&report.binary_path).is_file());
    assert!(Path::new(&report.install_script_path).is_file());
    assert!(Path::new(&report.manifest_path).is_file());
    let manifest = fs::read_to_string(&report.manifest_path).unwrap();
    assert!(manifest.contains("\"binarySha256\""));
    assert!(manifest.contains("\"installScript\""));
    assert!(manifest.contains("\"installScriptSha256\""));
    assert!(manifest.contains("\"manualInstallOnly\": true"));
    assert!(!manifest.contains("axon-fixture"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn package_command_creates_portable_archive_and_sha256_sidecar() {
    let root = temp_root("package-archive");
    let binary = root.join(package_binary_name(&current_target_triple()));
    fs::create_dir_all(&root).unwrap();
    fs::write(&binary, b"axon-fixture").unwrap();
    fs::write(
        root.join(package_launcher_name("x86_64-unknown-linux-gnu")),
        b"launcher",
    )
    .unwrap();
    let output_dir = root.join("dist");
    let command = PackageCommand {
        target: Some("x86_64-unknown-linux-gnu".to_owned()),
        binary: Some(binary),
        output_dir: Some(output_dir),
        archive: true,
        verify_manifest: None,
        no_smoke: false,
        format: None,
    };

    let report = PackagePlan::from_command(&command)
        .unwrap()
        .execute()
        .unwrap();
    let archive = report.archive.as_ref().expect("应生成归档报告");
    let archive_bytes = fs::read(&archive.path).unwrap();
    let sidecar = fs::read_to_string(&archive.sidecar_path).unwrap();

    assert_eq!(archive.format, "zip");
    assert_eq!(&archive_bytes[0..4], b"PK\x03\x04");
    assert_eq!(archive.bytes, archive_bytes.len() as u64);
    assert!(sidecar.contains(&archive.sha256));
    assert!(!report.auto_install_allowed);
    assert!(!report.network_publish_allowed);
    assert!(!report.remote_trigger_allowed);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn package_verify_manifest_recomputes_hash_and_rejects_unsafe_flags() {
    let root = temp_root("package-verify");
    let binary = root.join(package_binary_name(&current_target_triple()));
    fs::create_dir_all(&root).unwrap();
    fs::write(&binary, b"axon-fixture").unwrap();
    fs::write(
        root.join(package_launcher_name("x86_64-unknown-linux-gnu")),
        b"launcher",
    )
    .unwrap();
    let output_dir = root.join("dist");
    let command = PackageCommand {
        target: Some("x86_64-unknown-linux-gnu".to_owned()),
        binary: Some(binary),
        output_dir: Some(output_dir),
        archive: false,
        verify_manifest: None,
        no_smoke: false,
        format: None,
    };
    let report = PackagePlan::from_command(&command)
        .unwrap()
        .execute()
        .unwrap();

    let verified = PackageVerifyPlan::new(PathBuf::from(&report.manifest_path), true)
        .execute()
        .unwrap();
    assert_eq!(verified.status, PACKAGE_STATUS_VERIFIED);
    assert_eq!(verified.binary_sha256, report.binary_sha256);
    assert_eq!(verified.install_script_sha256, report.install_script_sha256);
    assert!(!verified.smoke_checked);
    assert!(!verified.remote_trigger_allowed);
    assert!(!verified.network_publish_allowed);

    let original_script = fs::read_to_string(&report.install_script_path).unwrap();
    fs::write(&report.install_script_path, "tampered").unwrap();
    let script_error = PackageVerifyPlan::new(PathBuf::from(&report.manifest_path), true)
        .execute()
        .unwrap_err();
    assert!(script_error.contains("安装脚本 sha256"));
    fs::write(&report.install_script_path, original_script).unwrap();

    let unsafe_manifest = fs::read_to_string(&report.manifest_path).unwrap().replace(
        "\"autoInstallAllowed\": false",
        "\"autoInstallAllowed\": true",
    );
    fs::write(&report.manifest_path, unsafe_manifest).unwrap();
    let error = PackageVerifyPlan::new(PathBuf::from(&report.manifest_path), true)
        .execute()
        .unwrap_err();
    assert!(error.contains("安全标记"));

    let _ = fs::remove_dir_all(root);
}

fn temp_root(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root().join(format!("axon-package-{label}-{nonce}"))
}
