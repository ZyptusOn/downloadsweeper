use ds_dev::sync::sync;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(source.join("main.rs"), "fn main() {}\n").unwrap();
    let build = dir.path().join("build");
    (dir, source, build)
}
fn put(root: &Path, name: &str, contents: &str) {
    let p = root.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}
#[test]
fn private_data_cache_and_unmanaged_files_survive_managed_deletion() {
    let (_dir, source, build) = fixture();
    put(&source, ".env", "DS_TEST_KEY=\n");
    sync(&source, &build, false).unwrap();
    assert!(!build.join(".env").exists());
    put(&build, ".env", "private local configuration");
    put(&build, "target/cache", "cache");
    put(&build, "notes.txt", "unmanaged");
    fs::remove_file(source.join("main.rs")).unwrap();
    assert_eq!(sync(&source, &build, false).unwrap().2, 1);
    assert!(!build.join("main.rs").exists());
    for (name, value) in [
        (".env", "private local configuration"),
        ("target/cache", "cache"),
        ("notes.txt", "unmanaged"),
    ] {
        assert_eq!(fs::read_to_string(build.join(name)).unwrap(), value);
    }
}
#[test]
fn conflict_preflight_prevents_partial_update_and_delete() {
    let (_dir, source, build) = fixture();
    sync(&source, &build, false).unwrap();
    put(&build, "main.rs", "local edit");
    put(&source, "Cargo.toml", "[workspace]\nresolver=\"2\"\n");
    assert!(sync(&source, &build, false)
        .unwrap_err()
        .to_string()
        .contains("Local edit"));
    assert_eq!(
        fs::read_to_string(build.join("Cargo.toml")).unwrap(),
        "[workspace]\n"
    );
    fs::remove_file(source.join("main.rs")).unwrap();
    assert!(sync(&source, &build, false)
        .unwrap_err()
        .to_string()
        .contains("Local edit"));
}
#[test]
fn dry_run_never_creates_destination_and_roots_cannot_be_nested_or_reversed() {
    let (_dir, source, build) = fixture();
    assert_eq!(sync(&source, &build, true).unwrap(), (2, 2, 0));
    assert!(!build.exists());
    assert!(sync(&source, &source.join("build"), false).is_err());
    sync(&source, &build, false).unwrap();
    assert_eq!(sync(&source, &build, true).unwrap(), (2, 0, 0));
    assert!(sync(&build, &source, false).is_err());
}
#[test]
fn nonempty_unmanaged_target_and_link_are_rejected() {
    let (_dir, source, build) = fixture();
    put(&build, "notes.txt", "unmanaged");
    assert!(sync(&source, &build, false).is_err());
    let link = source.join("linked.rs");
    #[cfg(unix)]
    std::os::unix::fs::symlink(source.join("main.rs"), &link).unwrap();
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(source.join("main.rs"), &link).is_err() {
            return;
        }
    }
    assert!(sync(&source, &build.with_file_name("clean"), false).is_err());
}
#[test]
fn interrupted_copy_accepts_current_source_hash_before_manifest_update() {
    let (_dir, source, build) = fixture();
    sync(&source, &build, false).unwrap();
    put(&source, "main.rs", "fn main() { println!(\"new\"); }\n");
    fs::copy(source.join("main.rs"), build.join("main.rs")).unwrap();
    sync(&source, &build, false).unwrap();
    assert_eq!(sync(&source, &build, true).unwrap(), (2, 0, 0));
}
#[test]
fn malicious_manifest_cannot_escape_or_claim_private_files() {
    let (_dir, source, build) = fixture();
    sync(&source, &build, false).unwrap();
    for name in ["../outside.txt", ".env", "target/cache"] {
        let marker = serde_json::json!({"version":1,"source":source,"files":{name:"invalid"}});
        put(&build, ds_dev::sync::MARKER, &marker.to_string());
        assert!(sync(&source, &build, false).is_err());
    }
}
