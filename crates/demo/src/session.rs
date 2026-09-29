//! One owned, persistent demonstration; reset only verified synthetic files.
use crate::fixtures;
use anyhow::{ensure, Context, Result};
use ds_engine::{
    safe_fs::{self, TaskStore},
    workflow::Task,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub const TASK_ID: &str = "d5de0001-2026-4091-8000-000000000001";
const MARKER: &str = "demo-state.json";
const FORMAT: &str = "downloadsweeper-fixed-demo-v1";
pub struct Session {
    pub base: PathBuf,
    pub root: PathBuf,
    pub task: Task,
    _lock: File,
}

pub fn state(base: &Path) -> Result<Value> {
    let value: Value = serde_json::from_slice(&fs::read(base.join(MARKER))?)?;
    ensure!(
        value["format"] == FORMAT && value["task_id"] == TASK_ID,
        "不是当前版本的演示目录"
    );
    Ok(value)
}
pub fn step(base: &Path, step: u64) -> Result<()> {
    ensure!(step <= 10, "演示步骤无效");
    let mut value = state(base)?;
    value["step"] = json!(step);
    safe_fs::atomic_json(&base.join(MARKER), &value)
}
pub fn effective_step(base: &Path, saved: &Value) -> u64 {
    let stored = saved["step"].as_u64().unwrap_or(0);
    let task = fs::read(base.join("data/tasks").join(TASK_ID).join("task.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Task>(&b).ok());
    let Some(task) = task else { return stored };
    let actual = if task.status == "rolled_back" {
        10
    } else {
        match task.phase {
            5 if task.recycled.iter().any(|r| r.active()) => 9,
            5 if task.status == "completed" => 8,
            5 => 7,
            4 if task.proposal.as_ref().is_some_and(|p| p.scene == "review") => 6,
            4 if task.nodes.iter().any(|n| n.id == "demo-video") => 7,
            4 => 5,
            3 => 4,
            2 if task.proposal.is_some() => 3,
            2 => 2,
            1 => 1,
            _ => 0,
        }
    };
    stored.max(actual)
}
fn no_links(path: &Path) -> Result<()> {
    for item in walkdir::WalkDir::new(path).follow_links(false) {
        let item = item?;
        ensure!(!safe_fs::is_link(item.path()), "演示目录含链接，已停止重置");
    }
    Ok(())
}
fn files(path: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    no_links(path)?;
    let mut result = BTreeMap::new();
    for item in walkdir::WalkDir::new(path).follow_links(false) {
        let item = item?;
        if item.file_type().is_file() {
            ensure!(
                item.metadata()?.len() < 1024 * 1024,
                "演示目录出现非样本文件，已保留文件并停止重置"
            );
            result.insert(
                item.path().strip_prefix(path)?.into(),
                fs::read(item.path())?,
            );
        }
    }
    Ok(result)
}
fn reset(base: &Path, root: &Path) -> Result<()> {
    state(base)?;
    // Recover our tracked files first, including a recycle interrupted by closing the app.
    {
        let store = TaskStore::new(base.join("data/tasks"))?;
        let cancel = CancellationToken::new();
        let progress = |_: usize, _: usize, _: &str| {};
        for mut task in store.list()? {
            ensure!(task.root == root, "发现非演示任务，已停止重置");
            safe_fs::recover(&mut task, &store)?;
            let batches: BTreeSet<_> = task
                .recycled
                .iter()
                .filter(|r| r.active())
                .map(|r| r.batch)
                .collect();
            for batch in batches {
                ds_engine::recycle::undo(&mut task, &store, batch, &cancel, &progress)?;
            }
            if task.operations.iter().any(|o| o.status == "done") {
                safe_fs::rollback(&mut task, &store, &cancel, &progress)?;
            }
        }
    }
    ensure!(files(root)? == files(&base.join("Reference"))?,
        "样本被修改或加入了其他文件；已保留文件并停止重置。请将新增文件另存、恢复被修改的样本后再重置。");
    // Both paths are fixed children of our locked, marker-verified workspace.
    for name in ["Desktop", "data"] {
        let target = base.join(name);
        ensure!(target.parent() == Some(base), "无效演示重置路径");
        no_links(&target)?;
        fs::remove_dir_all(&target)?;
    }
    if base.join("demo-evidence.json").exists() {
        fs::remove_file(base.join("demo-evidence.json"))?;
    }
    fixtures::create(root)?;
    step(base, 0)?;
    let mut saved = state(base)?;
    saved["generation"] = json!(saved["generation"].as_u64().unwrap_or(0) + 1);
    safe_fs::atomic_json(&base.join(MARKER), &saved)?;
    Ok(())
}
pub fn open(base: PathBuf, reset_requested: bool) -> Result<Session> {
    ensure!(!safe_fs::is_link(&base), "演示数据目录不可为链接");
    fs::create_dir_all(&base)?;
    let base = fs::canonicalize(base)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(base.join(".demo.lock"))?;
    lock.try_lock().context(
        "演示已在运行。请回到原窗口；重置可使用页面按钮，或关闭原窗口后运行 Reset-Demo.cmd。",
    )?;
    let root = base.join("Desktop");
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        ensure!(
            root.as_os_str().encode_wide().count() <= 170,
            "演示数据路径过长；请省略 --workspace 或使用较短目录"
        );
    }
    let initialized = base.join(MARKER).exists();
    if !initialized {
        ensure!(
            fs::read_dir(&base)?.all(|e| e.is_ok_and(|e| e.file_name() == ".demo.lock")),
            "目录包含现有内容，拒绝作为演示目录初始化"
        );
        fixtures::create(&root)?;
        fixtures::create(&base.join("Reference"))?;
        safe_fs::atomic_json(
            &base.join(MARKER),
            &json!({"format":FORMAT,"task_id":TASK_ID,"step":0}),
        )?;
    } else {
        state(&base)?;
        if reset_requested {
            reset(&base, &root)?;
        }
    }
    let id = uuid::Uuid::parse_str(TASK_ID)?;
    let task = {
        let store = TaskStore::new(base.join("data/tasks"))?;
        if store.path(id).exists() {
            store.load(id)?
        } else {
            let mut task = Task::new(root.clone(), "desktop", fixtures::permissions())?;
            task.id = id;
            store.save(&task)?;
            store.event(&task,"demo_created",json!({"synthetic":true,"model":"scripted","real_api_cost":0,"fixed_scenario":true}))?;
            task
        }
    };
    ensure!(task.root == root, "演示任务根目录不匹配");
    Ok(Session {
        base,
        root,
        task,
        _lock: lock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopen_and_reset_preserve_identity_and_protect_extra_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = open(dir.path().to_owned(), false).unwrap();
        let root = first.root.clone();
        let original = files(&root).unwrap();
        step(&first.base, 3).unwrap();
        assert!(open(dir.path().to_owned(), false).is_err());
        drop(first);
        let reopened = open(dir.path().to_owned(), false).unwrap();
        assert_eq!(reopened.root, root);
        assert_eq!(state(&reopened.base).unwrap()["step"], 3);
        fs::write(root.join("must-keep.txt"), "user file").unwrap();
        drop(reopened);
        assert!(open(dir.path().to_owned(), true).is_err());
        assert_eq!(
            fs::read_to_string(root.join("must-keep.txt")).unwrap(),
            "user file"
        );
        fs::remove_file(root.join("must-keep.txt")).unwrap();
        let reset = open(dir.path().to_owned(), true).unwrap();
        assert_eq!(state(&reset.base).unwrap()["step"], 0);
        assert_eq!(reset.task.id.to_string(), TASK_ID);
        assert_eq!(files(&root).unwrap(), original);
    }
}
