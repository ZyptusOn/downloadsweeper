//! Recoverable same-volume moves. No overwrite, trash or recursive deletion API.
use crate::workflow::{modified_ms, safe_relative, under, Progress, Task};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;
mod content_hash;
pub use content_hash::{fingerprint, fingerprint_matches};

pub fn is_link(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

pub fn checked_path(root: &Path, rel: &str) -> Result<PathBuf> {
    safe_relative(rel)?;
    let mut path = root.to_path_buf();
    ensure!(
        root.is_dir() && !is_link(root),
        "扫描根目录不可用或已被替换成链接"
    );
    for part in rel.split('/') {
        path.push(part);
        ensure!(!is_link(&path), "路径包含符号链接或 Windows 联接点：{rel}");
    }
    Ok(path)
}

/// Validate the opened handle as well as the path, so a Windows junction swap
/// between path validation and open cannot send content from outside the root.
pub fn open_evidence(root: &Path, rel: &str, size: u64, modified: u64) -> Result<File> {
    let path = checked_path(root, rel)?;
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        // Hold a read lease during extraction: existing/new writers or deletion are refused.
        OpenOptions::new().read(true).share_mode(1).open(&path)?
    };
    #[cfg(unix)]
    let file = open_beneath(root, rel)?;
    #[cfg(not(any(windows, unix)))]
    let file = File::open(&path)?;
    #[cfg(not(windows))]
    let _ = path;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
        let mut buffer = vec![0u16; 32768];
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as _,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                0,
            )
        } as usize;
        ensure!(
            length > 0 && length < buffer.len(),
            "无法确认文件句柄的实际位置"
        );
        let actual = String::from_utf16(&buffer[..length])?.to_lowercase();
        let root = fs::canonicalize(root)?
            .to_string_lossy()
            .trim_end_matches('\\')
            .to_lowercase()
            + "\\";
        ensure!(actual.starts_with(&root), "文件句柄位于授权目录之外");
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let actual = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
        ensure!(
            actual.starts_with(fs::canonicalize(root)?),
            "文件句柄位于授权目录之外"
        );
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
        let mut buffer = [0u8; libc::PATH_MAX as usize];
        ensure!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } != -1,
            "无法确认文件句柄的实际位置"
        );
        let length = buffer
            .iter()
            .position(|b| *b == 0)
            .context("文件路径过长")?;
        let actual = Path::new(std::ffi::OsStr::from_bytes(&buffer[..length]));
        ensure!(
            actual.starts_with(fs::canonicalize(root)?),
            "文件句柄位于授权目录之外"
        );
    }
    verify_evidence(&file, size, modified)?;
    Ok(file)
}

// Resolve each component relative to an already opened directory. O_NOFOLLOW
// applies to every component, closing the check/open symlink race on Unix.
#[cfg(unix)]
fn open_beneath(root: &Path, rel: &str) -> Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::OpenOptionsExt,
    };
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let parts: Vec<_> = rel.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        let name = std::ffi::CString::new(*part)?;
        let last = index + 1 == parts.len();
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if last {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}
pub fn verify_evidence(file: &File, size: u64, modified: u64) -> Result<()> {
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() == size && modified_ms(&metadata) == modified,
        "文件在扫描或读取后发生变化，请重新扫描"
    );
    Ok(())
}

pub fn atomic_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    atomic_json_with_cancel(path, value, &CancellationToken::new(), &|_, _, _| {})
}
pub fn atomic_json_with_cancel(
    path: &Path,
    value: &impl serde::Serialize,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "保存已暂停，原文件保持不变");
    let data = serde_json::to_vec_pretty(value)?;
    let parent = path.parent().context("没有父目录")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let result = (|| -> Result<()> {
        for (index, chunk) in data.chunks(64 * 1024).enumerate() {
            ensure!(!cancel.is_cancelled(), "保存已暂停，原文件保持不变");
            file.write_all(chunk)?;
            progress(
                (index * 64 * 1024 + chunk.len()).min(data.len()),
                data.len(),
                "正在写入临时快照",
            );
        }
        file.sync_all()?;
        drop(file);
        ensure!(!cancel.is_cancelled(), "保存已暂停，原文件保持不变");
        fs::rename(&tmp, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[derive(Clone)]
pub struct TaskStore {
    pub directory: PathBuf,
    _lock: std::sync::Arc<File>,
}
impl TaskStore {
    pub fn new(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(".lock"))?;
        lock.try_lock().context(
            "任务数据正在被另一个 DownloadSweeper 进程使用；请先关闭该进程，避免并发执行",
        )?;
        Ok(Self {
            directory,
            _lock: std::sync::Arc::new(lock),
        })
    }
    pub fn path(&self, id: uuid::Uuid) -> PathBuf {
        self.directory.join(id.to_string()).join("task.json")
    }
    pub fn save(&self, task: &Task) -> Result<()> {
        atomic_json(&self.path(task.id), task)
    }
    pub fn load(&self, id: uuid::Uuid) -> Result<Task> {
        let task: Task = serde_json::from_slice(&fs::read(self.path(id))?)?;
        ensure!(
            task.id == id && task.schema_version == 2,
            "任务文件版本或标识无效"
        );
        Ok(task)
    }
    pub fn list(&self) -> Result<Vec<Task>> {
        let mut tasks = vec![];
        for e in fs::read_dir(&self.directory)? {
            let e = e?;
            if let Ok(id) = uuid::Uuid::parse_str(&e.file_name().to_string_lossy()) {
                if let Ok(task) = self.load(id) {
                    tasks.push(task);
                }
            }
        }
        tasks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(tasks)
    }
    pub fn event(&self, task: &Task, kind: &str, detail: Value) -> Result<()> {
        let path = self.path(task.id).with_file_name("trajectory.jsonl");
        fs::create_dir_all(path.parent().unwrap())?;
        let mut file = OpenOptions::new().append(true).create(true).open(path)?;
        let event = json!({"schema_version":2,"task_id":task.id,"revision":task.revision,
            "timestamp":chrono::Utc::now().to_rfc3339(),"kind":kind,"detail":detail});
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
        file.sync_all()?;
        Ok(())
    }
    pub fn events(&self, id: uuid::Uuid) -> Result<Vec<Value>> {
        let path = self.path(id).with_file_name("trajectory.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        Ok(fs::read_to_string(path)?
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }
}

/// Both Windows and macOS provide a kernel-level no-replace rename.
pub fn move_noreplace(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let src: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
        // No REPLACE_EXISTING and no COPY_ALLOWED: atomic, same-volume, never overwrite.
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(src.as_ptr(), dst.as_ptr(), 0)
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let src = std::ffi::CString::new(src.as_os_str().as_bytes())?;
        let dst = std::ffi::CString::new(dst.as_os_str().as_bytes())?;
        if unsafe { libc::renamex_np(src.as_ptr(), dst.as_ptr(), libc::RENAME_EXCL) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let src = std::ffi::CString::new(src.as_os_str().as_bytes())?;
        let dst = std::ffi::CString::new(dst.as_os_str().as_bytes())?;
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                src.as_ptr(),
                libc::AT_FDCWD,
                dst.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    anyhow::bail!("当前平台尚未实现无覆盖移动");
    Ok(())
}

/// Deterministic metadata-only manifest; content fingerprints are still recorded before moving.
pub fn directory_manifest(path: &Path, cancel: &CancellationToken) -> Result<String> {
    directory_manifest_with_progress(path, cancel, &|_, _, _| {})
}

pub fn directory_manifest_with_progress(path: &Path, cancel: &CancellationToken, progress: &Progress<'_>) -> Result<String> {
    let mut hash = blake3::Hasher::new();
    for (index, item) in walkdir::WalkDir::new(path).follow_links(false).sort_by_file_name().into_iter().enumerate() {
        ensure!(!cancel.is_cancelled(), "目录核对已取消");
        if index % 128 == 0 { progress(index, 0, "正在核对完整文件夹的目录清单，不读取文件正文"); }
        let item = item?;
        ensure!(!is_link(item.path()), "整体目录包含链接，不能移动");
        let meta = item.metadata()?;
        ensure!(meta.is_dir() || meta.is_file(), "目录包含不支持的文件类型");
        let rel = item.path().strip_prefix(path)?.to_string_lossy();
        hash.update(&serde_json::to_vec(&(rel, meta.is_dir(), if meta.is_file() { meta.len() } else { 0 }, modified_ms(&meta)))?);
    }
    Ok(hash.finalize().to_hex().to_string())
}

fn verify_snapshot(task: &Task, source: &str, manifest: Option<&str>, cancel: &CancellationToken, progress: &Progress<'_>) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "任务已取消");
    let original = task
        .entries
        .iter()
        .find(|e| e.id == source)
        .context("计划来源不在扫描快照中")?;
    let path = checked_path(&task.root, source)?;
    let meta = fs::metadata(&path).with_context(|| format!("源文件不存在：{source}"))?;
    ensure!(
        meta.is_dir() == original.is_dir(),
        "源文件类型已改变：{source}"
    );
    if !original.is_dir() {
        ensure!(
            meta.len() == original.size && modified_ms(&meta) == original.modified_ms,
            "扫描后文件已改变，请重新扫描：{source}"
        );
    } else if task.mode == "desktop" {
        ensure!(original.parent.is_empty() && original.class == crate::domain::DirClass::Atomic,
            "桌面只能整体移动一级原子目录");
        ensure!(Some(directory_manifest_with_progress(&path, cancel, progress)?.as_str()) == manifest,
            "目录内容在规划后发生变化或缺少目录快照，请重新生成 AI 计划");
    } else {
        let expected: std::collections::HashMap<_, _> = task
            .entries
            .iter()
            .filter(|e| under(&e.id, source))
            .map(|e| (e.id.clone(), e))
            .collect();
        let mut count = 0;
        for e in walkdir::WalkDir::new(&path)
            .min_depth(1)
            .follow_links(false)
        {
            ensure!(!cancel.is_cancelled(), "任务已取消");
            let e = e?;
            ensure!(!is_link(e.path()), "整体目录包含链接");
            let rel = crate::workflow::relative(&task.root, e.path())?;
            let old = expected
                .get(&rel)
                .context("整体目录在扫描后新增了内容，请重新扫描")?;
            let m = e.metadata()?;
            ensure!(
                m.is_dir() == old.is_dir()
                    && (m.is_dir() || (m.len() == old.size && modified_ms(&m) == old.modified_ms)),
                "整体目录内容已改变：{rel}"
            );
            count += 1;
        }
        ensure!(count == expected.len(), "整体目录内容已被删除，请重新扫描");
    }
    Ok(())
}

pub fn validate_operations(task: &Task) -> Result<()> {
    validate_operations_with_cancel(task, &CancellationToken::new(), &|_, _, _| {})
}

fn validate_operations_with_cancel(
    task: &Task,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    let ops: Vec<_> = task
        .operations
        .iter()
        .filter(|o| o.selected && o.status == "pending")
        .collect();
    let mut destinations = std::collections::HashSet::new();
    let mut sources = std::collections::HashSet::new();
    let atomic: Vec<_> = task
        .entries
        .iter()
        .filter(|e| e.is_dir() && e.class == crate::domain::DirClass::Atomic)
        .collect();
    let moving_directories: Vec<_> = ops.iter().filter(|o| o.kind == "directory").collect();
    for (index, op) in ops.iter().enumerate() {
        if task.mode == "desktop" {
            let entry = task
                .entries
                .iter()
                .find(|e| e.id == op.source)
                .context("源文件不在桌面快照中")?;
            ensure!(
                !crate::workflow::desktop_retained(entry) && op.kind == entry.kind,
                "桌面模式不能移动受保护项目"
            );
        }
        verify_snapshot(task, &op.source, op.directory_manifest.as_deref(), cancel, progress)?;
        if index % 100 == 0 {
            progress(index, ops.len(), "执行前核对扫描快照与目标位置");
        }
        ensure!(sources.insert(&op.source), "计划中有重复源文件");
        ensure!(
            op.source != op.destination && !under(&op.destination, &op.source),
            "不能移动到自身或自身内部"
        );
        let dst = checked_path(&task.root, &op.destination)?;
        ensure!(
            !dst.try_exists()?,
            "目标位置已被占用，请重新生成计划：{}",
            op.destination
        );
        ensure!(
            destinations.insert(op.destination.to_lowercase()),
            "多个文件指向同一目标"
        );
        for d in &atomic {
            ensure!(!under(&op.source, &d.id), "不能拆散整体保护目录：{}", d.id);
            ensure!(
                !under(&op.destination, &d.id),
                "不能将文件写入整体保护目录：{}",
                d.id
            );
        }
        for other in &moving_directories {
            if other.id != op.id {
                ensure!(
                    !under(&op.source, &other.source) && !under(&op.destination, &other.source),
                    "计划中的目录移动相互重叠"
                );
            }
        }
    }
    Ok(())
}

pub fn execute(
    task: &mut Task,
    store: &TaskStore,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    ensure!(
        task.phase == 5 && task.reviewed && task.status == "planned",
        "必须审查并确认当前计划后才能执行"
    );
    execute_remaining(task, store, cancel, progress)
}

/// Only an explicitly resumed, reviewed job may continue a partially executed plan.
pub fn resume_execute(
    task: &mut Task,
    store: &TaskStore,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    ensure!(
        task.phase == 5
            && task.reviewed
            && matches!(
                task.status.as_str(),
                "planned" | "partial" | "recovery_required" | "completed"
            ),
        "当前计划不能继续执行"
    );
    ensure!(
        task.operations
            .iter()
            .filter(|o| o.selected)
            .all(|o| matches!(o.status.as_str(), "pending" | "done") && o.error.is_none()),
        "存在失败或位置未确定的操作，请先检查轨迹；不会自动重试文件移动"
    );
    for (index, op) in task
        .operations
        .iter()
        .enumerate()
        .filter(|(_, o)| o.status == "done")
    {
        progress(
            index,
            task.operations.len(),
            "核对已完成的移动，避免重复执行",
        );
        ensure!(
            !checked_path(&task.root, &op.source)?.try_exists()?,
            "已移动文件的原路径重新出现，请人工检查"
        );
        ensure!(
            content_hash::matches_with_progress(
                &checked_path(&task.root, &op.destination)?,
                op.fingerprint.as_deref().context("已完成操作缺少指纹")?,
                cancel,
                &mut content_hash::reporter(
                    progress,
                    index,
                    task.operations.len(),
                    &op.destination
                )
            )?,
            "已移动的文件发生变化，不能继续旧计划"
        );
    }
    execute_remaining(task, store, cancel, progress)
}

fn execute_remaining(
    task: &mut Task,
    store: &TaskStore,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    progress(
        0,
        task.operations.len(),
        "执行前重新检查文件、冲突和整体保护规则",
    );
    validate_operations_with_cancel(task, cancel, progress)?;
    task.status = "executing".into();
    task.touch();
    store.save(task)?;
    store.event(
        task,
        "execution_started",
        json!({"operations":task.operations.len()}),
    )?;
    let total = task.operations.iter().filter(|o| o.selected).count();
    let mut done = task
        .operations
        .iter()
        .filter(|o| o.selected && o.status == "done")
        .count();
    for i in 0..task.operations.len() {
        if cancel.is_cancelled() {
            break;
        }
        if !task.operations[i].selected || task.operations[i].status != "pending" {
            continue;
        }
        let op = task.operations[i].clone();
        progress(done, total, &format!("检查文件状态 {}", op.source));
        let result = (|| -> Result<()> {
            verify_snapshot(task, &op.source, op.directory_manifest.as_deref(), cancel, progress)?;
            let src = checked_path(&task.root, &op.source)?;
            let dst = checked_path(&task.root, &op.destination)?;
            let hash = content_hash::fingerprint_with_progress(
                &src,
                cancel,
                &mut content_hash::reporter(progress, done, total, &op.source),
            )?;
            ensure!(!cancel.is_cancelled(), "任务已取消");
            verify_snapshot(task, &op.source, op.directory_manifest.as_deref(), cancel, progress)?;
            task.operations[i].fingerprint = Some(hash.clone());
            task.operations[i].status = "moving".into();
            store.save(task)?;
            let algorithm = hash.split_once(':').context("新指纹缺少算法标记")?.0;
            store.event(task,"move_intent",json!({"operation_id":op.id,"source":op.source,"destination":op.destination,"fingerprint":hash,"fingerprint_algorithm":algorithm}))?;
            progress(done, total, &format!("指纹已保存，正在移动 {}", op.source));
            fs::create_dir_all(dst.parent().context("目标没有父目录")?)?;
            checked_path(&task.root, &op.destination)?;
            move_noreplace(&src, &dst).with_context(|| {
                format!(
                    "移动失败（目标不可覆盖，跨卷移动不自动执行）：{}",
                    op.source
                )
            })?;
            task.operations[i].status = "done".into();
            store.save(task)?;
            store.event(
                task,
                "move_done",
                json!({"operation_id":op.id,"source":op.source,"destination":op.destination}),
            )?;
            Ok(())
        })();
        if let Err(e) = result {
            // If a move succeeded but recording failed, keep the intent for recovery.
            if cancel.is_cancelled() && task.operations[i].status == "pending" {
                task.operations[i].error = None;
            } else if !matches!(task.operations[i].status.as_str(), "done" | "moving") {
                task.operations[i].status = "failed".into();
                task.operations[i].error = Some(format!("{e:#}"));
            }
            task.status = "partial".into();
            store.save(task)?;
            store.event(task, "execution_stopped", json!({"error":format!("{e:#}")}))?;
            break;
        }
        done += 1;
        progress(done, total, "文件移动已记录，可随时停止");
    }
    let remaining = task
        .operations
        .iter()
        .any(|o| o.selected && o.status != "done");
    task.status = if remaining { "partial" } else { "completed" }.into();
    task.prepare_cleanup();
    task.touch();
    store.save(task)?;
    store.event(
        task,
        "execution_finished",
        json!({"status":task.status,"completed":done,"cancelled":cancel.is_cancelled()}),
    )?;
    Ok(())
}

pub fn rollback(
    task: &mut Task,
    store: &TaskStore,
    cancel: &CancellationToken,
    progress: &Progress<'_>,
) -> Result<()> {
    ensure!(
        !crate::recycle::has_pending(task),
        "请先撤销删除建议中的回收批次，再恢复整理操作"
    );
    ensure!(
        matches!(
            task.status.as_str(),
            "completed" | "partial" | "recovery_required"
        ),
        "当前任务没有可恢复的执行记录"
    );
    let total = task
        .operations
        .iter()
        .filter(|o| o.status == "done")
        .count();
    let mut done = 0;
    for i in (0..task.operations.len()).rev() {
        if cancel.is_cancelled() {
            break;
        }
        if task.operations[i].status != "done" {
            continue;
        }
        let op = task.operations[i].clone();
        progress(done, total, &format!("恢复 {}", op.source));
        let result = (|| -> Result<()> {
            let current = checked_path(&task.root, &op.destination)?;
            let original = checked_path(&task.root, &op.source)?;
            ensure!(
                !original.try_exists()?,
                "原位置已被占用，保留当前文件：{}",
                op.source
            );
            let expected = op
                .fingerprint
                .as_deref()
                .context("缺少恢复指纹，已停止操作")?;
            ensure!(
                content_hash::matches_with_progress(
                    &current,
                    expected,
                    cancel,
                    &mut content_hash::reporter(progress, done, total, &op.destination)
                )?,
                "整理后的文件已被修改，保留当前文件：{}",
                op.destination
            );
            task.operations[i].status = "restoring".into();
            store.save(task)?;
            store.event(
                task,
                "restore_intent",
                json!({"operation_id":op.id,"source":op.destination,"destination":op.source}),
            )?;
            fs::create_dir_all(original.parent().unwrap())?;
            checked_path(&task.root, &op.source)?;
            progress(done, total, &format!("指纹一致，正在恢复 {}", op.source));
            move_noreplace(&current, &original)?;
            task.operations[i].status = "restored".into();
            task.operations[i].error = None;
            store.save(task)?;
            store.event(task, "restore_done", json!({"operation_id":op.id}))?;
            Ok(())
        })();
        if let Err(e) = result {
            task.operations[i].error = Some(format!("{e:#}"));
            // Preserve the intent: the rename may have succeeded before persistence
            // failed. Recovery must reconcile both paths before another attempt.
            store.save(task)?;
            break;
        }
        done += 1;
    }
    let remaining = task
        .operations
        .iter()
        .any(|o| matches!(o.status.as_str(), "done" | "moving" | "restoring"));
    task.status = if remaining { "partial" } else { "rolled_back" }.into();
    task.prepare_cleanup();
    task.touch();
    store.save(task)?;
    store.event(
        task,
        "rollback_finished",
        json!({"status":task.status,"restored":done}),
    )?;
    Ok(())
}

/// Reconcile only journalled moves. Never guess or mutate files during startup.
pub fn recover(task: &mut Task, store: &TaskStore) -> Result<()> {
    if task.status != "executing"
        && !task
            .operations
            .iter()
            .any(|o| matches!(o.status.as_str(), "moving" | "restoring"))
    {
        return Ok(());
    }
    for op in &mut task.operations {
        if !matches!(op.status.as_str(), "moving" | "restoring") {
            continue;
        }
        let restoring = op.status == "restoring";
        let (from, to) = if restoring {
            (&op.destination, &op.source)
        } else {
            (&op.source, &op.destination)
        };
        let src = checked_path(&task.root, from)?;
        let dst = checked_path(&task.root, to)?;
        if !src.try_exists()?
            && dst.try_exists()?
            && fingerprint_matches(
                &dst,
                op.fingerprint.as_deref().context("中断记录缺少恢复指纹")?,
                &CancellationToken::new(),
            )?
        {
            op.status = if restoring { "restored" } else { "done" }.into();
            op.error = None;
        } else if src.try_exists()? && !dst.try_exists()? {
            op.status = if restoring { "done" } else { "pending" }.into();
            op.error = None;
        } else {
            op.error = Some("中断时的文件位置不明确，需要人工检查".into());
        }
    }
    task.status = "recovery_required".into();
    task.touch();
    store.save(task)?;
    store.event(task, "recovered", json!({"status":task.status}))?;
    Ok(())
}
