use ds_dev::share::*;
use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Write},
    path::Path,
    process::Command,
};
fn secret() -> String {
    ["synthetic-", "verification-secret-123456"].concat()
}
fn checker() -> Checker {
    Checker::new([secret().into_bytes()])
}
fn git(root: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
        .status
        .success());
}
fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        z.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}
#[test]
fn private_paths_and_usable_examples() {
    for p in [
        "config.toml",
        "dir_class_overrides.json",
        "crates/.DS_Store",
        "src-tauri/gen/schemas/desktop-schema.json",
        "%SystemDrive%/cache.json",
        "node_modules/package/index.js",
        ".venv/bin/python",
        ".ENV.local",
    ] {
        assert!(private_path(Path::new(p)), "{p}");
    }
    for p in [
        "config.example.toml",
        ".env.example",
        "Cargo.lock",
        "frontend/vendor/react.production.min.js",
    ] {
        assert!(!private_path(Path::new(p)), "{p}");
    }
    let mut c = checker();
    c.data(
        "config.example.toml",
        include_bytes!("../../../config.example.toml"),
    );
    c.data(".env.example", include_bytes!("../../../.env.example"));
    assert!(c.failures.is_empty(), "{:?}", c.failures);
}
#[test]
fn index_checks_actual_staged_blob_even_after_working_file_cleaned() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    fs::write(d.path().join("notes.txt"), secret()).unwrap();
    git(d.path(), &["add", "notes.txt"]);
    fs::write(d.path().join("notes.txt"), "clean").unwrap();
    let mut c = checker();
    check_index(d.path(), &mut c).unwrap();
    assert!(c.failures.iter().any(|f| f.contains("Git index/")));
    assert!(c.failures.iter().all(|f| !f.contains(&secret())));
}
#[test]
fn binary_utf16_and_unknown_provider_keys_are_detected_without_echo() {
    let key = secret();
    for bytes in [
        format!("\0{key}\0").into_bytes(),
        key.encode_utf16().flat_map(u16::to_le_bytes).collect(),
    ] {
        let mut c = checker();
        c.data("binary.exe", &bytes);
        assert!(!c.failures.is_empty());
        assert!(c.failures.iter().all(|f| !f.contains(&key)));
    }
    let unknown = ["sk-", &"a1".repeat(16)].concat();
    let mut c = Checker::new([]);
    c.data("unknown.txt", unknown.as_bytes());
    assert!(!c.failures.is_empty());
    assert!(c.failures.iter().all(|f| !f.contains(&unknown)));
}
#[test]
fn opaque_toml_yaml_and_environment_literals_fail() {
    for (name, data) in [
        ("config.example.toml", format!("api_key = \"{}\"", secret())),
        ("settings.yaml", format!("api_key: {}", secret())),
        ("run.sh", format!("export PROVIDER_API_KEY={}", secret())),
    ] {
        let mut c = Checker::new([]);
        c.data(name, data.as_bytes());
        assert!(!c.failures.is_empty(), "{name}");
        assert!(c.failures.iter().all(|f| !f.contains(&secret())));
    }
}
#[test]
fn compressed_packages_private_entries_bad_zip_and_missing_package() {
    let d = tempfile::tempdir().unwrap();
    let data = zip(&[("bin/app.exe", secret().as_bytes()), (".env", b"")]);
    let p = d.path().join("portable.zip");
    fs::write(&p, data).unwrap();
    let mut c = checker();
    c.package(&p);
    assert!(c.failures.iter().any(|f| f.contains("known local")));
    assert!(c.failures.iter().any(|f| f.contains("private/runtime")));
    let p = d.path().join("corrupt.zip");
    fs::write(&p, b"not zip").unwrap();
    let mut c = checker();
    c.package(&p);
    c.package(&d.path().join("missing"));
    assert_eq!(c.failures.len(), 2);
}
#[test]
fn custom_key_environment_and_private_file_discovery() {
    let d = tempfile::tempdir().unwrap();
    fs::write(
        d.path().join("config.toml"),
        "[llm]\napi_key_env=\"MY_MODEL_KEY\"\n",
    )
    .unwrap();
    fs::write(
        d.path().join(".env.local"),
        format!("MY_MODEL_KEY={}\n", secret()),
    )
    .unwrap();
    let known = known_credentials(d.path(), &HashMap::new()).unwrap();
    assert!(known.contains(secret().as_bytes()));
    let (files, _) = project_files(d.path()).unwrap();
    assert!(!files.contains(Path::new(".env.local")));
    assert!(files.contains(Path::new("config.toml")));
    fs::write(d.path().join("accidental.txt"), secret()).unwrap();
    let c = verify(d.path(), &[]).unwrap();
    assert!(!c.failures.is_empty());
}
#[test]
fn ignored_but_tracked_private_files_remain_visible() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    fs::write(d.path().join(".gitignore"), ".env\n").unwrap();
    fs::write(d.path().join(".env"), b"").unwrap();
    git(d.path(), &["add", "-f", ".env"]);
    let (files, tracked) = project_files(d.path()).unwrap();
    assert!(files.contains(Path::new(".env")) && tracked.contains(Path::new(".env")));
    assert!(private_path(Path::new(".env")));
    assert!(!verify(d.path(), &[]).unwrap().failures.is_empty());
}
#[test]
fn cli_reports_failure_without_secret_in_output() {
    let d = tempfile::tempdir().unwrap();
    fs::write(
        d.path().join("config.toml"),
        format!("[llm]\napi_key=\"{}\"\n", secret()),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ds-dev"))
        .args(["verify-share", "--project-only", "--root"])
        .arg(d.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(&secret()));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(&secret()));
}
#[test]
fn deleted_working_files_are_still_verified_in_git_index() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    fs::write(d.path().join("deleted.txt"), secret()).unwrap();
    git(d.path(), &["add", "deleted.txt"]);
    fs::remove_file(d.path().join("deleted.txt")).unwrap();
    fs::write(
        d.path().join(".env"),
        format!("MODEL_API_KEY={}\n", secret()),
    )
    .unwrap();
    let (files, tracked) = project_files(d.path()).unwrap();
    assert!(!files.contains(Path::new("deleted.txt")));
    assert!(tracked.contains(Path::new("deleted.txt")));
    assert!(!verify(d.path(), &[]).unwrap().failures.is_empty());
}
