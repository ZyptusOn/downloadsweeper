//! Metadata-only review candidates, never a deletion plan or a duplicate-content claim.
use crate::{
    domain::DirClass,
    workflow::{under, Entry, Task},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub large_mib: u64,
    pub stale_days: u64,
    pub limit: usize,
    pub categories: Vec<String>,
}
pub const CATEGORIES: &[&str] = &[
    "large",
    "temporary",
    "incomplete",
    "installer",
    "archive",
    "copy",
    "empty",
];
impl Default for Options {
    fn default() -> Self {
        Self {
            large_mib: 1024,
            stale_days: 180,
            limit: 200,
            categories: CATEGORIES.iter().map(|s| s.to_string()).collect(),
        }
    }
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=1_048_576).contains(&self.large_mib)
                && (7..=3650).contains(&self.stale_days)
                && (1..=500).contains(&self.limit)
                && self.categories.len() <= CATEGORIES.len()
                && self
                    .categories
                    .iter()
                    .all(|s| CATEGORIES.contains(&s.as_str())),
            "清理筛选无效：大文件 1–1048576 MiB，未修改 7–3650 天，最多 1–500 条"
        );
        Ok(())
    }
}
pub fn protected(task: &Task, entry: &Entry) -> bool {
    task.entries
        .iter()
        .any(|e| e.is_dir() && e.class == DirClass::Atomic && under(&entry.id, &e.id))
}
fn copy_key(name: &str) -> String {
    let lower = name.to_lowercase();
    let (stem, ext) = lower.rsplit_once('.').unwrap_or((&lower, ""));
    let stem = stem
        .strip_suffix(" - copy")
        .or_else(|| stem.strip_suffix(" - 副本"))
        .unwrap_or(stem);
    let stem = if let Some((base, suffix)) = stem.rsplit_once(" (") {
        if suffix
            .strip_suffix(')')
            .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        {
            base
        } else {
            stem
        }
    } else {
        stem
    };
    format!("{stem}.{ext}")
}
pub fn prepare(task: &mut Task, now_ms: u64) {
    let _ = prepare_with_cancel(
        task,
        now_ms,
        &tokio_util::sync::CancellationToken::new(),
        &|_, _, _| {},
    );
}
/// Build off to the side; cancellation never publishes an incomplete candidate list.
pub fn prepare_with_cancel(
    task: &mut Task,
    now_ms: u64,
    cancel: &tokio_util::sync::CancellationToken,
    progress: &crate::workflow::Progress<'_>,
) -> Result<()> {
    ensure!(!cancel.is_cancelled(), "清理筛选已暂停");
    let options = &task.cleanup_options;
    let mut copies: HashMap<(String, u64), HashSet<String>> = HashMap::new();
    let protected_ids: HashSet<_> = task
        .entries
        .iter()
        .filter(|e| e.is_dir() && e.class == DirClass::Atomic)
        .map(|e| e.id.as_str())
        .collect();
    let files: Vec<_> = task
        .entries
        .iter()
        .filter(|e| {
            if e.is_dir() {
                return false;
            }
            let mut parent = e.parent.as_str();
            while !parent.is_empty() {
                if protected_ids.contains(parent) {
                    return false;
                }
                parent = parent.rsplit_once('/').map(|p| p.0).unwrap_or("");
            }
            true
        })
        .collect();
    for (index, e) in files.iter().enumerate() {
        ensure!(!cancel.is_cancelled(), "清理筛选已暂停");
        if index % 256 == 0 {
            progress(index, files.len() * 2, "建立清理候选索引；不读取正文");
        }
        copies
            .entry((copy_key(&e.name), e.size))
            .or_default()
            .insert(e.name.to_lowercase());
    }
    let moved: HashMap<_, _> = task
        .operations
        .iter()
        .filter(|o| o.status == "done")
        .map(|o| (o.source.as_str(), o.destination.as_str()))
        .collect();
    let moved_dirs: HashMap<_, _> = task
        .operations
        .iter()
        .filter(|o| o.status == "done" && o.kind == "directory")
        .map(|o| (o.source.as_str(), o.destination.as_str()))
        .collect();
    let destination = |e: &Entry| {
        if let Some(path) = moved.get(e.id.as_str()) {
            return (*path).to_owned();
        }
        let mut parent = e.parent.as_str();
        while !parent.is_empty() {
            if let Some(path) = moved_dirs.get(parent) {
                return format!("{}{}", path, &e.id[parent.len()..]);
            }
            parent = parent.rsplit_once('/').map(|p| p.0).unwrap_or("");
        }
        e.id.clone()
    };
    let mut found = vec![];
    for (index, e) in files.iter().enumerate() {
        ensure!(!cancel.is_cancelled(), "清理筛选已暂停");
        if index % 256 == 0 {
            progress(
                files.len() + index,
                files.len() * 2,
                "筛选清理建议；不删除文件",
            );
        }
        let age = if e.modified_ms > 0 && e.modified_ms <= now_ms {
            Some((now_ms - e.modified_ms) / 86_400_000)
        } else {
            None
        };
        let stale = age.is_some_and(|d| d >= options.stale_days);
        let mut reasons: Vec<(&str, &str)> = vec![];
        if e.size >= options.large_mib.saturating_mul(1024 * 1024) {
            reasons.push(("large", "体积较大；请确认是否仍需本地保存"));
        }
        if (stale && ["tmp", "temp", "log", "bak", "old"].contains(&e.extension.as_str()))
            || (e.size < 65536 && ["tmp", "log"].contains(&e.extension.as_str()))
        {
            reasons.push((
                "temporary",
                "临时、日志或备份格式；可能仍在使用或有恢复价值，请确认后再处理",
            ));
        }
        if stale && ["part", "crdownload", "download", "partial"].contains(&e.extension.as_str()) {
            reasons.push((
                "incomplete",
                "疑似未完成下载且较久未修改；先确认下载器是否仍需续传",
            ));
        }
        if stale && ["exe", "msi", "msix", "appx", "dmg", "pkg"].contains(&e.extension.as_str()) {
            reasons.push((
                "installer",
                "较久未修改的安装或可执行文件；可能是唯一离线安装包或绿色软件，请确认用途",
            ));
        }
        if stale
            && ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso"].contains(&e.extension.as_str())
        {
            reasons.push((
                "archive",
                "较久未修改的压缩包或镜像；先确认是否已解压及是否有其他完整备份",
            ));
        }
        if e.size > 0
            && copies
                .get(&(copy_key(&e.name), e.size))
                .is_some_and(|names| names.len() > 1)
        {
            reasons.push((
                "copy",
                "存在名称去除副本编号后相近且同大小的文件；未比较内容，不代表重复，需人工核对",
            ));
        }
        if e.size == 0 && stale && !e.name.starts_with('.') {
            reasons.push((
                "empty",
                "零字节且较久未修改；可能是占位、锁或程序标记，请确认用途",
            ));
        }
        reasons.retain(|(kind, _)| options.categories.iter().any(|s| s == kind));
        if reasons.is_empty() {
            continue;
        }
        found.push(
            json!({"original_id":e.id,"path":destination(e),"size":e.size,
            "category":reasons[0].0,"categories":reasons.iter().map(|r|r.0).collect::<Vec<_>>(),
            "reason":reasons.iter().map(|r|r.1).collect::<Vec<_>>().join("；"),
            "source":"local","age_days":age,"confidence":"needs_review"}),
        );
    }
    // Stable and useful when capped: reclaimable size first, then the original ID.
    found.sort_by(|a, b| {
        b["size"]
            .as_u64()
            .cmp(&a["size"].as_u64())
            .then_with(|| a["original_id"].as_str().cmp(&b["original_id"].as_str()))
    });
    ensure!(!cancel.is_cancelled(), "清理筛选已暂停");
    progress(files.len() * 2, files.len() * 2, "清理候选已计算，正在保存");
    task.cleanup_summary = json!({"matched":found.len(),"shown":found.len().min(options.limit),
        "protected_files":task.entries.iter().filter(|e|!e.is_dir()).count()-files.len(),"generated_at_ms":now_ms,
        "basis":"扫描快照的大小、名称、修改时间；未读取正文或检查内容重复"});
    found.truncate(options.limit);
    task.cleanup = found;
    Ok(())
}
