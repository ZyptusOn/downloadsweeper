#![cfg(unix)]
use ds_engine::{safe_fs, workflow};
use std::{
    fs,
    io::Read,
    os::unix::{ffi::OsStringExt, fs::symlink},
};

#[test]
fn unix_names_preserve_identity_and_links_cannot_supply_evidence() {
    let root = std::env::temp_dir().join(format!("ds-unix-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("inside")).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let name = "片段:版本\\原稿.txt";
    let path = root.join("inside").join(name);
    fs::write(&path, b"authorized").unwrap();
    let rel = workflow::relative(&root, &path).unwrap();
    assert_eq!(rel, format!("inside/{name}"));
    workflow::safe_relative(&rel).unwrap();
    let meta = fs::metadata(&path).unwrap();
    let mut handle =
        safe_fs::open_evidence(&root, &rel, meta.len(), workflow::modified_ms(&meta)).unwrap();
    let collision = workflow::collision_path(&root, &rel, &mut Default::default()).unwrap();
    assert_eq!(collision, "inside/片段:版本\\原稿 (1).txt");
    fs::rename(&path, root.join("original.txt")).unwrap();
    fs::write(&path, b"replacement").unwrap();
    let mut text = String::new();
    handle.read_to_string(&mut text).unwrap();
    assert_eq!(text, "authorized");
    symlink(root.join("original.txt"), root.join("link.txt")).unwrap();
    symlink(root.join("inside"), root.join("linked-dir")).unwrap();
    assert!(
        safe_fs::open_evidence(&root, "link.txt", meta.len(), workflow::modified_ms(&meta))
            .is_err()
    );
    assert!(safe_fs::open_evidence(&root, &format!("linked-dir/{name}"), 11, 0).is_err());
    for invalid in ["../outside", "/absolute", "a//b", "a/./b", "a/../b", "a\0b"] {
        assert!(workflow::safe_relative(invalid).is_err());
    }
    let invalid_utf8 = root.join(std::ffi::OsString::from_vec(vec![0xff]));
    assert!(workflow::relative(&root, &invalid_utf8).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unix_moves_never_overwrite_and_private_env_permissions_are_repaired() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("ds-unix-move-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a"), b"a").unwrap();
    fs::write(root.join("b"), b"b").unwrap();
    assert!(safe_fs::move_noreplace(&root.join("a"), &root.join("b")).is_err());
    safe_fs::move_noreplace(&root.join("a"), &root.join("c")).unwrap();
    assert_eq!(fs::read(root.join("b")).unwrap(), b"b");
    fs::write(root.join(".env"), "# private fixture\n").unwrap();
    fs::set_permissions(root.join(".env"), fs::Permissions::from_mode(0o644)).unwrap();
    let mut cfg = ds_engine::config::AppConfig::default();
    cfg.store_model_key(&root.join("config.toml"), "synthetic-unix-fixture")
        .unwrap();
    assert_eq!(
        fs::metadata(root.join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::remove_dir_all(root).unwrap();
}
