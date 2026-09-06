use super::*;
use crate::permission::PermissionConfig;
use std::{cell::Cell, path::PathBuf};
struct FakeBin {
    root: PathBuf,
    fail_after_put: Cell<bool>,
}
impl Bin for FakeBin {
    fn check(&self, _: &Path) -> Result<()> {
        Ok(())
    }
    fn find(&self, stage: &Path) -> Result<Option<String>> {
        let path = self.root.join(stage.file_name().unwrap());
        Ok(path.exists().then(|| path.to_string_lossy().into_owned()))
    }
    fn put(&self, stage: &Path) -> Result<()> {
        safe_fs::move_noreplace(stage, &self.root.join(stage.file_name().unwrap()))?;
        ensure!(
            !self.fail_after_put.get(),
            "simulated process lost reply after native move"
        );
        Ok(())
    }
    fn restore(&self, stage: &Path, receipt: &str) -> Result<()> {
        safe_fs::move_noreplace(Path::new(receipt), stage)
    }
}
struct Fixture {
    task: Task,
    store: TaskStore,
    bin: FakeBin,
    root: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!("ds-recycle-test-{}", Uuid::new_v4()));
    let files = root.join("Desktop");
    fs::create_dir_all(&files).unwrap();
    fs::write(files.join("cache.log"), b"test data only").unwrap();
    fs::write(files.join("keep.txt"), b"keep").unwrap();
    let mut task = Task::new(files, "desktop", PermissionConfig::default()).unwrap();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    task.scan(&c, &p).unwrap();
    task.prepare_desktop(&c, &p).unwrap();
    task.reviewed = true;
    task.advance().unwrap();
    let store = TaskStore::new(root.join("state")).unwrap();
    safe_fs::execute(&mut task, &store, &c, &p).unwrap();
    let bin = FakeBin {
        root: root.join("bin"),
        fail_after_put: Cell::new(false),
    };
    fs::create_dir(&bin.root).unwrap();
    Fixture {
        task,
        store,
        bin,
        root,
    }
}
#[test]
fn selection_cannot_inject_paths_and_changed_file_aborts_before_move() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    assert!(recycle_with(
        &mut f.task,
        &f.store,
        &["../outside".into()],
        &c,
        &p,
        &f.bin
    )
    .is_err());
    let path = current_path(&f.task, "cache.log");
    fs::write(f.task.root.join(&path), "changed").unwrap();
    assert!(recycle_with(&mut f.task, &f.store, &["cache.log".into()], &c, &p, &f.bin).is_err());
    assert!(f.task.recycled.is_empty());
    assert!(f.task.root.join(path).is_file());
}
#[test]
fn recycle_and_undo_survive_reload_and_refuse_collisions() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    recycle_with(&mut f.task, &f.store, &["cache.log".into()], &c, &p, &f.bin).unwrap();
    let mut task = f.store.load(f.task.id).unwrap();
    let record = task.recycled[0].clone();
    let original = task.root.join(&record.path);
    assert!(!original.exists());
    assert!(safe_fs::rollback(&mut task, &f.store, &c, &p).is_err());
    fs::write(&original, "new occupant").unwrap();
    assert!(undo_with(&mut task, &f.store, record.batch, &c, &p, &f.bin).is_err());
    assert_eq!(fs::read_to_string(&original).unwrap(), "new occupant");
    fs::rename(&original, f.root.join("occupant-preserved.txt")).unwrap();
    undo_with(&mut task, &f.store, record.batch, &c, &p, &f.bin).unwrap();
    assert_eq!(fs::read_to_string(&original).unwrap(), "test data only");
    assert!(!has_pending(&task));
    safe_fs::rollback(&mut task, &f.store, &c, &p).unwrap();
    assert!(task.root.join("cache.log").is_file());
}
#[test]
fn missing_native_reply_and_duplicate_names_have_unambiguous_recovery() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    f.bin.fail_after_put.set(true);
    assert!(recycle_with(&mut f.task, &f.store, &["cache.log".into()], &c, &p, &f.bin).is_err());
    let mut task = f.store.load(f.task.id).unwrap();
    assert_eq!(task.recycled[0].status, "recycling");
    assert!(task.recycled[0].receipt.is_none());
    let batch = task.recycled[0].batch;
    undo_with(&mut task, &f.store, batch, &c, &p, &f.bin).unwrap();
    assert!(!has_pending(&task));
    assert_eq!(
        fs::read_to_string(task.root.join(&task.recycled[0].path)).unwrap(),
        "test data only"
    );
}
#[test]
fn cancellation_and_missing_bin_never_claim_success() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    c.cancel();
    assert!(recycle_with(&mut f.task, &f.store, &["cache.log".into()], &c, &p, &f.bin).is_err());
    assert!(f.task.recycled.is_empty());
    let c = CancellationToken::new();
    recycle_with(&mut f.task, &f.store, &["cache.log".into()], &c, &p, &f.bin).unwrap();
    let record = f.task.recycled[0].clone();
    let item = f.bin.root.join(record.stage_name());
    // Simulate a user moving a bin entry elsewhere; never empty a real recycle bin.
    fs::rename(&item, f.root.join("externally-moved")).unwrap();
    assert!(undo_with(&mut f.task, &f.store, record.batch, &c, &p, &f.bin).is_err());
    assert!(has_pending(&f.task));
}

#[test]
#[ignore = "Explicitly run against the OS recycle bin with synthetic fixture files only"]
fn native_recycle_roundtrip() {
    let mut f = fixture();
    let c = CancellationToken::new();
    let p = |_, _, _: &str| {};
    let result = recycle(&mut f.task, &f.store, &["cache.log".into()], &c, &p);
    if let Some(record) = f.task.recycled.first() {
        let batch = record.batch;
        // Always attempt to restore our fixture even when receipt confirmation fails.
        undo(&mut f.task, &f.store, batch, &c, &p).unwrap();
    }
    result.unwrap();
    assert!(!has_pending(&f.task));
    assert_eq!(
        fs::read_to_string(f.task.root.join(current_path(&f.task, "cache.log"))).unwrap(),
        "test data only"
    );
}
