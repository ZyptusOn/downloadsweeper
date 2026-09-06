use ds_engine::{
    archive::Archive,
    chat_memory, cleanup,
    config::AppConfig,
    domain::DirClass,
    llm::Message,
    permission::PermissionConfig,
    safe_fs::TaskStore,
    workflow::{ChatTurn, Task},
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn fixture() -> (Task, TaskStore) {
    let base = std::env::temp_dir().join(format!("ds-archive-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(base.join("files")).unwrap();
    let task = Task::new(base.join("files"), "organize", PermissionConfig::default()).unwrap();
    let store = TaskStore::new(base.join("state")).unwrap();
    (task, store)
}
#[test]
fn archive_preserves_full_state_but_is_never_executable() {
    let (mut task, store) = fixture();
    task.pending_calls
        .push(json!({"id":"uncertain","reserved":100}));
    task.messages.push(ChatTurn {
        role: "user".into(),
        scene: "tree".into(),
        content: "保留素材".into(),
    });
    store.save(&task).unwrap();
    store
        .event(&task, "file_move_intent", json!({"example":true}))
        .unwrap();
    let archive = store.export_archive(task.id).unwrap();
    assert_eq!(archive.trajectory.len(), 1);
    let expected = serde_json::to_value(&archive).unwrap();
    let id = store.import_archive(archive).unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.load(id).is_err());
    std::fs::rename(&task.root, task.root.with_file_name("not-here")).unwrap();
    let loaded = store.load_archive(id).unwrap();
    assert_eq!(serde_json::to_value(&loaded).unwrap(), expected);
    assert!(loaded.task.clone().imported().is_err());
    let mut restored = loaded.task;
    restored.root = task.root.with_file_name("not-here");
    let derived = restored.imported().unwrap();
    assert!(!derived.scanned && derived.operations.is_empty() && derived.pending_calls.is_empty());
    assert_eq!(derived.messages.len(), 1);
    assert_eq!(store.load_archive(id).unwrap().task.pending_calls.len(), 1);
    let mut tampered: Archive = serde_json::from_value(expected).unwrap();
    tampered.task.messages[0].content = "changed".into();
    assert!(store.import_archive(tampered).is_err());
}
#[test]
fn archive_rejects_truncated_trajectory_instead_of_silent_omission() {
    let (task, store) = fixture();
    store.save(&task).unwrap();
    store.event(&task, "saved", json!({})).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(store.path(task.id).with_file_name("trajectory.jsonl"))
        .unwrap()
        .write_all(b"{broken")
        .unwrap();
    assert!(store.export_archive(task.id).is_err());
}
#[test]
fn history_uses_available_context_and_stays_in_scene() {
    let (mut task, _) = fixture();
    for i in 0..40 {
        task.messages.push(ChatTurn {
            role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("message-{i}"),
            scene: "tree".into(),
        });
    }
    task.messages.push(ChatTurn {
        role: "user".into(),
        scene: "permissions".into(),
        content: "HIDDEN_SCENE".into(),
    });
    let mut cfg = AppConfig::default();
    cfg.llm.context_length = 64_000;
    cfg.llm.max_output_tokens = 8_000;
    let (messages, info) = chat_memory::with_history(
        &task,
        &cfg,
        "tree",
        false,
        vec![Message::system("rules")],
        "new",
    );
    assert_eq!(info["included"], 40);
    assert_eq!(messages.last().unwrap().content, "new");
    assert!(!messages.iter().any(|m| m.content.contains("HIDDEN_SCENE")));
    cfg.token_budget = Some(2000);
    task.pending_calls.push(json!({"reserved_tokens":500}));
    let (_, limited) = chat_memory::with_history(
        &task,
        &cfg,
        "tree",
        false,
        vec![Message::system("rules")],
        "new",
    );
    assert!(limited["included"].as_u64().unwrap() < 40);
    assert!(limited["input_bytes"].as_u64().unwrap() <= limited["input_limit"].as_u64().unwrap());
    cfg.token_budget = None;
    task.pending_calls.clear();
    for m in &mut task.messages {
        m.content = m.content.repeat(150);
    }
    cfg.llm.context_length = 8_000;
    cfg.llm.max_output_tokens = 2_000;
    let (messages, info) = chat_memory::with_history(
        &task,
        &cfg,
        "tree",
        false,
        vec![Message::system("rules")],
        "new",
    );
    assert!(info["included"].as_u64().unwrap() < 40);
    assert!(info["excerpted"].as_u64().unwrap() > 0);
    assert!(info["input_bytes"].as_u64().unwrap() <= info["input_limit"].as_u64().unwrap());
    assert!(messages.iter().any(|m| m.content.contains("message-39")));
}
#[test]
fn cleanup_covers_categories_protects_packages_and_does_not_read_or_mutate_files() {
    let (mut task, _) = fixture();
    std::fs::create_dir(task.root.join("Portable")).unwrap();
    for name in [
        "old.zip",
        "setup.msi",
        "old.log",
        "broken.part",
        "report.pdf",
        "report (1).pdf",
        "Portable/program.exe",
        "fresh.part",
    ] {
        std::fs::write(task.root.join(name), b"fixture").unwrap();
    }
    std::fs::write(task.root.join("empty.txt"), b"").unwrap();
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    let now = 2000 * 86_400_000u64;
    for e in &mut task.entries {
        e.modified_ms = now - 400 * 86_400_000;
        if e.name == "Portable" {
            e.class = DirClass::Atomic;
        }
        if e.name == "fresh.part" {
            e.modified_ms = now;
        }
    }
    task.cleanup_options.large_mib = 1;
    task.entries
        .iter_mut()
        .find(|e| e.name == "old.zip")
        .unwrap()
        .size = 2 * 1024 * 1024;
    cleanup::prepare(&mut task, now);
    let categories: Vec<_> = task
        .cleanup
        .iter()
        .flat_map(|e| e["categories"].as_array().unwrap())
        .filter_map(|s| s.as_str())
        .collect();
    for name in cleanup::CATEGORIES {
        assert!(categories.contains(name), "missing {name}");
    }
    assert!(!task
        .cleanup
        .iter()
        .any(|e| e["path"] == "Portable/program.exe" || e["path"] == "fresh.part"));
    assert_eq!(task.cleanup_summary["protected_files"], 1);
    assert_eq!(
        std::fs::read(task.root.join("old.zip")).unwrap(),
        b"fixture"
    );
    task.cleanup_options.limit = 1;
    cleanup::prepare(&mut task, now);
    assert_eq!(task.cleanup.len(), 1);
    assert!(task.cleanup_summary["matched"].as_u64().unwrap() > 1);
    task.cleanup_options.categories.clear();
    cleanup::prepare(&mut task, now);
    assert!(task.cleanup.is_empty());
}
