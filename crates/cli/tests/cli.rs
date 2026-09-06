use serde_json::{json, Value};
use std::process::Command;
#[test]
fn cli_rules_review_execute_rollback_and_readonly_archives() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Downloads");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("sample.txt"), "CLI roundtrip fixture").unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!("scan_root={}\n", json!(root.to_string_lossy())),
    )
    .unwrap();
    let run = |args: Vec<String>, ok: bool| -> String {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ds"));
        c.args(args)
            .current_dir(dir.path())
            .env_clear()
            .env("DS_DATA_DIR", dir.path().join("data"))
            .env("DS_CONFIG", &config);
        for name in [
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "TMPDIR",
            "HOME",
            "USERPROFILE",
            "PATH",
        ] {
            if let Some(v) = std::env::var_os(name) {
                c.env(name, v);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
        }
        let o = c.output().unwrap();
        assert_eq!(
            o.status.success(),
            ok,
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap().trim().into()
    };
    let task = run(vec!["create".into(), root.to_string_lossy().into()], true);
    let call = |action: &str| run(vec![action.into(), task.clone()], true);
    run(vec!["execute".into(), task.clone()], false);
    for _ in 0..3 {
        call("next");
    }
    let t: Value = serde_json::from_str(&call("show")).unwrap();
    assert_eq!(t["phase"], 3);
    assert_eq!(t["plan_source"], "rules");
    assert!(t["calls"].as_array().unwrap().is_empty());
    assert_eq!(t["operations"].as_array().unwrap().len(), 1);
    assert!(root.join("sample.txt").exists());
    call("next");
    let t: Value = serde_json::from_str(&call("show")).unwrap();
    assert_eq!(t["phase"], 4);
    assert_eq!(t["operations"].as_array().unwrap().len(), 1);
    call("approve");
    call("execute");
    assert!(!root.join("sample.txt").exists());
    call("rollback");
    assert_eq!(
        std::fs::read_to_string(root.join("sample.txt")).unwrap(),
        "CLI roundtrip fixture"
    );
    let archive = dir.path().join("session.json");
    run(
        vec![
            "export".into(),
            task.clone(),
            archive.to_string_lossy().into(),
        ],
        true,
    );
    run(
        vec!["import".into(), archive.to_string_lossy().into()],
        true,
    );
    let list: Value = serde_json::from_str(&run(vec!["archives".into()], true)).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let id = list[0]["id"].as_str().unwrap();
    let a: Value =
        serde_json::from_str(&run(vec!["archive-show".into(), id.into()], true)).unwrap();
    let payload: Value = serde_json::from_str(a["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["task"]["id"], task);
    assert!(!payload["trajectory"].as_array().unwrap().is_empty());
    run(vec!["execute".into(), id.into()], false);
    let resumed = run(vec!["resume".into(), id.into()], true);
    assert_ne!(resumed, task);
    let t: Value = serde_json::from_str(&run(vec!["show".into(), resumed], true)).unwrap();
    assert_eq!(t["scanned"], false);
}
