//! User-confirmed recycling, separate from AI suggestions and organization moves.
//! Each file travels in a unique, journalled recovery folder. Native Trash retains
//! the original filename inside it; retries never identify items by filename alone.
use crate::{
    cleanup,
    safe_fs::{self, TaskStore},
    workflow::{Progress, Task},
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashSet, fs, io::Read, path::Path};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
mod native;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows_guard;
pub use native::supported;
const PREFIX: &str = "DownloadSweeper-Recycle-";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub id: Uuid,
    pub batch: Uuid,
    pub original_id: String,
    /// Location after organization, before user-confirmed recycling.
    pub path: String,
    pub size: u64,
    pub modified_ms: u64,
    pub hash: String,
    pub status: String,
    pub receipt: Option<String>,
    pub error: Option<String>,
}
impl Record {
    fn stage_name(&self) -> String {
        format!("{PREFIX}{}", self.id)
    }
    fn file_name(&self) -> Result<&str> {
        self.path.rsplit('/').next().context("无效回收路径")
    }
    fn staged_file(&self) -> Result<String> {
        Ok(format!("{}/{}", self.stage_name(), self.file_name()?))
    }
    pub fn active(&self) -> bool {
        self.status != "restored"
    }
}
pub fn is_staging_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .is_some_and(|id| Uuid::parse_str(id).is_ok())
}
pub fn has_pending(task: &Task) -> bool {
    task.recycled.iter().any(Record::active)
}

pub(super) trait Bin {
    fn check(&self, root: &Path) -> Result<()>;
    fn find(&self, stage: &Path) -> Result<Option<String>>;
    fn put(&self, stage: &Path) -> Result<()>;
    fn restore(&self, stage: &Path, receipt: &str) -> Result<()>;
}
fn save(task: &mut Task, store: &TaskStore) -> Result<()> {
    task.touch();
    store.save(task)
}
fn hash(
    task: &Task,
    record: &Record,
    rel: &str,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<String> {
    let mut file = safe_fs::open_evidence(&task.root, rel, record.size, record.modified_ms)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 128 * 1024];
    let mut read = 0usize;
    loop {
        ensure!(!cancel.is_cancelled(), "回收校验已暂停，文件和恢复记录保留");
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        read += count;
        progress(
            read,
            record.size.min(usize::MAX as u64) as usize,
            &format!("全量校验：{}", record.path),
        );
    }
    safe_fs::verify_evidence(&file, record.size, record.modified_ms)?;
    Ok(hasher.finalize().to_hex().to_string())
}
fn current_path(task: &Task, id: &str) -> String {
    task.operations
        .iter()
        .find(|o| o.source == id && o.status == "done")
        .map(|o| o.destination.clone())
        .unwrap_or_else(|| id.into())
}

pub fn recycle(
    task: &mut Task,
    store: &TaskStore,
    selected: &[String],
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    recycle_with(task, store, selected, cancel, progress, &native::Native)
}
fn recycle_with(
    task: &mut Task,
    store: &TaskStore,
    selected: &[String],
    cancel: &CancellationToken,
    progress: &Progress<'_>,
    bin: &impl Bin,
) -> Result<()> {
    ensure!(
        task.status == "completed" && task.phase == 5,
        "请先完成整理，再人工选择回收项目"
    );
    ensure!(
        !selected.is_empty()
            && selected.len() <= 500
            && selected.iter().collect::<HashSet<_>>().len() == selected.len(),
        "请选择 1–500 个不同的候选文件"
    );
    bin.check(&task.root)?;
    let batch = Uuid::new_v4();
    let mut planned = Vec::new();
    // Validate the entire selection before any file is moved.
    for id in selected {
        let candidate = task
            .cleanup
            .iter()
            .find(|e| e["original_id"].as_str() == Some(id))
            .context("所选文件不在当前删除建议中，请刷新后重新选择")?;
        let entry = task
            .entries
            .iter()
            .find(|e| &e.id == id && !e.is_dir())
            .context("候选文件不在扫描快照中")?;
        ensure!(
            !cleanup::protected(task, entry),
            "不能回收整体保护目录中的文件"
        );
        ensure!(
            !task
                .recycled
                .iter()
                .any(|r| r.original_id == *id && r.active()),
            "该文件已有未撤销的回收记录"
        );
        let path = current_path(task, id);
        ensure!(
            candidate["path"].as_str() == Some(&path),
            "候选路径与实际整理记录不一致"
        );
        let mut record = Record {
            id: Uuid::new_v4(),
            batch,
            original_id: id.clone(),
            path,
            size: entry.size,
            modified_ms: entry.modified_ms,
            hash: String::new(),
            status: "prepared".into(),
            receipt: None,
            error: None,
        };
        record.hash = hash(task, &record, &record.path, cancel, progress)?;
        ensure!(
            !safe_fs::checked_path(&task.root, &record.stage_name())?.try_exists()?,
            "恢复目录已被占用"
        );
        planned.push(record);
    }
    let first = task.recycled.len();
    task.recycled.extend(planned);
    save(task, store)?;
    store.event(
        task,
        "recycle_batch_confirmed",
        json!({"batch":batch,"selected":selected}),
    )?;
    for index in first..task.recycled.len() {
        if cancel.is_cancelled() {
            anyhow::bail!("回收已暂停；可一键撤销已处理和暂存文件");
        }
        let record = task.recycled[index].clone();
        let result = (|| -> Result<()> {
            ensure!(
                hash(task, &record, &record.path, cancel, progress)? == record.hash,
                "文件发生变化，拒绝回收"
            );
            let source = safe_fs::checked_path(&task.root, &record.path)?;
            let stage = safe_fs::checked_path(&task.root, &record.stage_name())?;
            fs::create_dir(&stage)?;
            let staged = safe_fs::checked_path(&task.root, &record.staged_file()?)?;
            safe_fs::move_noreplace(&source, &staged)?;
            task.recycled[index].status = "staged".into();
            save(task, store)?;
            ensure!(
                hash(task, &record, &record.staged_file()?, cancel, progress)? == record.hash,
                "暂存文件校验失败，已保留恢复记录"
            );
            ensure!(
                fs::read_dir(&stage)?.count() == 1,
                "恢复目录中出现其他项目，停止回收"
            );
            task.recycled[index].status = "recycling".into();
            save(task, store)?;
            store.event(
                task,
                "recycle_intent",
                json!({"record":record.id,"path":record.path,"stage":record.stage_name()}),
            )?;
            progress(
                index - first,
                selected.len(),
                &format!("移入系统回收站：{}", record.path),
            );
            bin.put(&stage)?;
            let receipt = bin
                .find(&stage)?
                .context("尚未确认回收站条目；保留日志，请点击撤销进行恢复检查")?;
            ensure!(!stage.try_exists()?, "原暂存目录仍存在，回收状态待核对");
            task.recycled[index].receipt = Some(receipt);
            task.recycled[index].status = "trashed".into();
            save(task, store)?;
            store.event(
                task,
                "recycled",
                json!({"record":record.id,"path":record.path}),
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            task.recycled[index].error = Some(error.to_string());
            save(task, store)?;
            return Err(error);
        }
    }
    progress(
        selected.len(),
        selected.len(),
        "已移入回收站，可一键撤销本批操作",
    );
    Ok(())
}

pub fn undo(
    task: &mut Task,
    store: &TaskStore,
    batch: Uuid,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    undo_with(task, store, batch, cancel, progress, &native::Native)
}
fn undo_with(
    task: &mut Task,
    store: &TaskStore,
    batch: Uuid,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
    bin: &impl Bin,
) -> Result<()> {
    ensure!(
        task.recycled.iter().any(|r| r.batch == batch),
        "回收批次不属于当前任务"
    );
    let mut errors = Vec::new();
    for index in (0..task.recycled.len()).rev() {
        let record = task.recycled[index].clone();
        if record.batch != batch || !record.active() {
            continue;
        }
        ensure!(
            !cancel.is_cancelled(),
            "撤销已暂停，已恢复文件与剩余记录保留"
        );
        let result = (|| -> Result<()> {
            let stage = safe_fs::checked_path(&task.root, &record.stage_name())?;
            let destination = safe_fs::checked_path(&task.root, &record.path)?;
            let staged = safe_fs::checked_path(&task.root, &record.staged_file()?)?;
            let receipt = bin.find(&stage)?;
            if !staged.try_exists()?
                && receipt.is_none()
                && destination.try_exists()?
                && matches!(record.status.as_str(), "prepared" | "restoring")
            {
                ensure!(
                    hash(task, &record, &record.path, cancel, progress)? == record.hash,
                    "原位文件已改变，不能确认恢复"
                );
            } else {
                ensure!(
                    !destination.try_exists()?,
                    "恢复位置已存在同名文件，未覆盖：{}",
                    record.path
                );
                task.recycled[index].status = "restoring".into();
                save(task, store)?;
                store.event(
                    task,
                    "recycle_restore_intent",
                    json!({"record":record.id,"path":record.path}),
                )?;
                if let Some(receipt) = receipt {
                    ensure!(!stage.try_exists()?, "暂存目录已占用，不能恢复回收站条目");
                    if let Some(saved) = &record.receipt {
                        ensure!(saved == &receipt, "回收站标识已变化，拒绝恢复其他条目");
                    }
                    bin.restore(&stage, &receipt)?;
                }
                ensure!(
                    staged.is_file(),
                    "未找到待恢复文件；请检查系统回收站是否被清空或手动移动"
                );
                ensure!(
                    hash(task, &record, &record.staged_file()?, cancel, progress)? == record.hash,
                    "回收文件已改变，停止自动恢复"
                );
                safe_fs::checked_path(&task.root, &record.path)?;
                safe_fs::move_noreplace(&staged, &destination)?;
            }
            task.recycled[index].status = "restored".into();
            task.recycled[index].error = None;
            save(task, store)?;
            store.event(
                task,
                "recycle_restored",
                json!({"record":record.id,"path":record.path}),
            )?;
            // Only remove our empty staging folder. Never recursively delete anything.
            if stage.is_dir() && !safe_fs::is_link(&stage) {
                let _ = fs::remove_dir(&stage);
            }
            Ok(())
        })();
        if let Err(error) = result {
            task.recycled[index].error = Some(error.to_string());
            save(task, store)?;
            errors.push(error.to_string());
        }
        progress(
            index,
            task.recycled.len(),
            "正在撤销回收；恢复冲突会保留文件与记录",
        );
    }
    ensure!(errors.is_empty(), "部分文件未恢复：{}", errors.join("；"));
    Ok(())
}
