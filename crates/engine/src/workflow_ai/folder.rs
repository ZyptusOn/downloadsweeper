//! Bounded one-level directory evidence; child names and excerpts obey both permission tiers.
use super::*;

pub(super) fn preview(task: &Task, entry: &Entry, cap: usize) -> Result<Value> {
    let path = checked_path(&task.root, &entry.id)?;
    let folder_tier = entry_tier(task, entry);
    ensure!(entry.is_dir() && folder_tier != AccessTier::None, "目录读取未授权");
    let mut items = std::fs::read_dir(&path)?.take(129).collect::<std::io::Result<Vec<_>>>()?;
    let limited = items.len() > 128;
    items.truncate(128);
    items.sort_by_key(|e| e.file_name());
    let mut children = vec![];
    let mut withheld = 0;
    let mut truncated = limited;
    let mut bytes = 0;
    let mut excerpts = 0;
    for item in items {
        let name = item.file_name().to_string_lossy().to_string();
        let extension = item.path().extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        if crate::safe_fs::is_link(&item.path()) || name.starts_with('.') || name.starts_with("~$")
            || ["lnk", "url", "webloc", "tmp", "part", "crdownload"].contains(&extension.as_str()) {
            continue;
        }
        let metadata = item.metadata()?;
        if !metadata.is_file() && !metadata.is_dir() { continue; }
        let child = Entry {
            id: crate::workflow::relative(&task.root, &item.path())?, parent: entry.id.clone(), name,
            extension, kind: if metadata.is_dir() { "directory" } else { "file" }.into(),
            size: if metadata.is_file() { metadata.len() } else { 0 },
            modified_ms: crate::workflow::modified_ms(&metadata), class: crate::domain::DirClass::Atomic,
            suggested: crate::domain::DirClass::Atomic, reason: String::new(), direct_files: 0, total_files: 0,
        };
        let tier = entry_tier(task, &child).min(folder_tier);
        if tier == AccessTier::None { withheld += 1; continue; }
        if children.len() >= 24 { truncated = true; break; }
        let mut value = file_descriptor(task, &child)?.context("目录证据权限已改变")?;
        if tier == AccessTier::FilenameOnly {
            value.as_object_mut().unwrap().remove("size");
            value.as_object_mut().unwrap().remove("modified_ms");
        }
        // Do not recurse or decode images inside a folder; at most two short text/Office excerpts.
        if !child.is_dir() && tier == AccessTier::ContentSlice && excerpts < 2 {
            if let Some(context) = file_context_capped(task, &child, 256.min(cap.saturating_sub(bytes)))? {
                for key in ["text_excerpt", "content_preview"] {
                    if let Some(v) = context.get(key) { value[key] = v.clone(); }
                }
            }
            excerpts += 1;
        }
        let size = serde_json::to_vec(&value)?.len();
        if bytes + size > cap.saturating_sub(160) { truncated = true; break; }
        bytes += size;
        children.push(value);
    }
    file_descriptor(task, entry)?.context("目录证据权限已改变")?;
    Ok(json!({"status":"ok","kind":"directory","atomic":true,"children":children,
        "withheld_entries":withheld,"truncated":truncated,"sampling":"first_level_only; at_most_128_entries_examined; at_most_24_returned; no_individual_moves"}))
}
