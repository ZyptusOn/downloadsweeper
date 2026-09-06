//! 下载目录扫描。
//!
//! 两个入口：
//! - `scan(root)`          仅返回文件 Vec（与历史调用兼容）
//! - `scan_with_dirs(root)` 返回 ScanResult（文件 + 目录特征 + 启发式类型建议）
//!
//! 目录特征启发式（建议值建议，用户可覆盖）：
//! - 子目录名像分类名（电影/番剧/文档…） → Container（整理容器）
//! - 含可执行/压缩包，或大量同格式小文件 → Atomic（原子目录）
//! - 其他 → Normal（可拆散）

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use walkdir::WalkDir;

use crate::domain::{DirClass, DirClassState, DirEntry, FileEntry, ScanResult};

/// 分类名关键词（含中英文常见分类目录名）。
const CLASSIFY_KEYWORDS: &[&str] = &[
    "电影",
    "番剧",
    "动漫",
    "动画",
    "剧集",
    "音乐",
    "文档",
    "资料",
    "图片",
    "照片",
    "视频",
    "软件",
    "工具",
    "安装包",
    "压缩包",
    "课件",
    "工作",
    "个人",
    "素材",
    "剪",
    "downloads",
    "movie",
    "movies",
    "video",
    "anime",
    "music",
    "docs",
    "documents",
    "images",
    "photo",
    "software",
    "tools",
    "archive",
];

/// 程序型扩展名（绿色软件/压缩包信号）。
const PROGRAM_EXTS: &[&str] = &[
    "exe", "msi", "dmg", "pkg", "app", "bat", "cmd", "com", "scr", "sh", "deb", "rpm", "zip",
    "rar", "7z", "tar", "gz", "iso", "ova", "vmware", "trainer", "patch", "crack",
];

/// 扫描 `root` 下所有普通文件，返回元数据条目（不含内容哈希，按需后续计算）。
/// 跳过无法读取的条目并在 tracing 中记录。
pub fn scan(root: &Path) -> Vec<FileEntry> {
    scan_with_dirs(root).files
}

/// 扫描下载目录（含目录特征），用于引导整理。
pub fn scan_with_dirs(root: &Path) -> ScanResult {
    let mut files = Vec::new();
    let mut dirs: Vec<DirEntry> = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path().to_path_buf();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skip unreadable entry");
                continue;
            }
        };

        if entry.file_type().is_file() {
            let extension = path
                .extension()
                .map(|s| s.to_string_lossy().to_ascii_lowercase());
            files.push(FileEntry {
                id: path.to_string_lossy().into_owned(),
                path,
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                extension,
                content_hash: None,
            });
        } else if entry.file_type().is_dir() && path != root {
            dirs.push(DirEntry {
                path,
                name: String::new(), // 下面填
                rel_path: String::new(),
                file_count: 0,
                total_size: 0,
                formats: HashMap::new(),
                has_program_files: false,
                subdir_count: 0,
                has_classification_like_subdirs: false,
                class: DirClassState::from_suggested(DirClass::Normal),
            });
        }
    }

    // 第二遍：填充目录特征（直接子文件/子目录统计）
    for d in dirs.iter_mut() {
        d.name = d
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        d.rel_path = d
            .path
            .strip_prefix(root)
            .unwrap_or(&d.path)
            .to_string_lossy()
            .into_owned();

        if let Ok(rd) = std::fs::read_dir(&d.path) {
            for de in rd.filter_map(|e| e.ok()) {
                let dp = de.path();
                if dp.is_dir() {
                    d.subdir_count += 1;
                    let name = de.file_name().to_string_lossy().to_lowercase();
                    if is_classify_like(&name) {
                        d.has_classification_like_subdirs = true;
                    }
                } else if dp.is_file() {
                    d.file_count += 1;
                    if let Ok(m) = de.metadata() {
                        d.total_size += m.len();
                    }
                    let ext = dp
                        .extension()
                        .map(|s| s.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_else(|| "(无)".into());
                    let e = d.formats.entry(ext.clone()).or_insert((0, 0));
                    e.0 += 1;
                    e.1 += dp.metadata().map(|m| m.len()).unwrap_or(0);
                    if PROGRAM_EXTS.contains(&ext.as_str()) {
                        d.has_program_files = true;
                    }
                }
            }
        }

        // 启发式建议类型
        d.class.suggested = heuristic_dir_class(d);
    }
    dirs.sort_by(|a, b| a.path.cmp(&b.path));
    ScanResult {
        root: root.to_path_buf(),
        files,
        dirs,
    }
}

/// 类别名/目录名"像分类名"判断：
/// - 中文关键词：直接 contains（如"电影""番剧""素材"）
/// - 英文关键词：词边界匹配（避免 "movies-hd" / "tools" 拆分误判，但 "GreenTools-v1" 不误判，
///   采用分隔符（[-_. ]）分词后逐词比较）
fn is_classify_like(name: &str) -> bool {
    let lower = name.to_lowercase();
    for k in CLASSIFY_KEYWORDS {
        let kl = k.to_lowercase();
        if is_cjk_or_contains(lower.as_str(), kl.as_str()) {
            return true;
        }
    }
    false
}

fn is_cjk_or_contains(name: &str, keyword: &str) -> bool {
    let is_cjk = |c: char| ('\u{4e00}'..='\u{9fa5}').contains(&c);
    // 中文关键词：任意包含即命中
    if keyword.chars().any(is_cjk) {
        return name.contains(keyword);
    }
    // 英文关键词：词边界匹配
    name.split(|c: char| c == '-' || c == '_' || c == '.' || c == ' ')
        .any(|word| word == keyword)
}

/// 目录类型启发式：Atomic（含程序/素材）优先，其次 Container，最后 Normal。
/// 修正：含可执行文件或大量同格式小文件的目录优先判 Atomic——
/// "绿色软件"虽名含"软件"（分类关键词），但 exe 才是其本质特征。
pub fn heuristic_dir_class(d: &DirEntry) -> DirClass {
    // 原子目录：含程序文件，或大量同格式小文件（>500 个且同格式占比高）
    if d.has_program_files {
        return DirClass::Atomic;
    }
    if d.file_count >= 500 {
        if let Some((max_ct, _)) = d.formats.values().max_by_key(|(c, _)| *c) {
            if *max_ct >= d.file_count / 2 {
                return DirClass::Atomic;
            }
        }
    }
    // 整理容器：
    // ① 目录名本身是分类名关键词（如"电影""番剧"），或
    // ② 子目录名像分类名（已有整理目录结构）
    if is_classify_like(&d.name) || (d.has_classification_like_subdirs && d.subdir_count > 0) {
        return DirClass::Container;
    }
    DirClass::Normal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DirClass;

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ds_scan_{name}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn heuristic_container_classification() {
        let root = tmpdir("ctn");
        std::fs::create_dir_all(root.join("电影")).unwrap();
        std::fs::create_dir_all(root.join("番剧")).unwrap();
        std::fs::write(root.join("电影/a.mp4"), b"x").unwrap();
        let result = scan_with_dirs(&root);
        let d = result.dir(&root.join("电影")).expect("电影 dir");
        assert_eq!(
            d.class.suggested,
            DirClass::Container,
            "分类名子目录应建议 Container"
        );
    }

    #[test]
    fn heuristic_atomic_program_dir() {
        let root = tmpdir("atom");
        // 目录名避开分类关键词，确保 Atomic 信号不被 Container 抢占
        std::fs::create_dir_all(root.join("GreenTools")).unwrap();
        std::fs::write(root.join("GreenTools/tool.exe"), b"exe content").unwrap();
        std::fs::write(root.join("GreenTools/readme.txt"), b"doc").unwrap();
        let result = scan_with_dirs(&root);
        let d = result.dir(&root.join("GreenTools")).expect("dir");
        assert_eq!(d.class.suggested, DirClass::Atomic, "含 exe 应建议 Atomic");
    }

    #[test]
    fn heuristic_atomic_many_small_files() {
        let root = tmpdir("many");
        std::fs::create_dir_all(root.join("icons")).unwrap();
        for i in 0..600 {
            std::fs::write(root.join(format!("icons/icon_{i}.png")), b"i").unwrap();
        }
        let result = scan_with_dirs(&root);
        let d = result.dir(&root.join("icons")).expect("dir");
        assert_eq!(
            d.class.suggested,
            DirClass::Atomic,
            "大量同格式小文件应建议 Atomic"
        );
    }

    #[test]
    fn user_override_wins() {
        let root = tmpdir("ovr");
        // 避开分类关键词，让"含 exe"信号触发 Atomic
        std::fs::create_dir_all(root.join("mixdir")).unwrap();
        std::fs::write(root.join("mixdir/tool.exe"), b"x").unwrap();
        let mut result = scan_with_dirs(&root);
        // 建议为 Atomic（含 exe），用户覆盖为 Normal
        assert!(result.set_dir_class(&root.join("mixdir"), DirClass::Normal));
        let d = result.dir(&root.join("mixdir")).unwrap();
        assert_eq!(d.class.effective(), DirClass::Normal);
        assert_eq!(d.class.suggested, DirClass::Atomic);
    }

    #[test]
    fn scan_legacy_still_works() {
        let root = tmpdir("legacy");
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.mp4"), b"v").unwrap();
        let files = scan(&root);
        assert_eq!(files.len(), 2);
    }
}

// ── 目录类型覆盖存储（用户显式指定，跨扫描保留）──

/// 目录类型覆盖存储：用户手动把某目录标为 Container/Atomic/Normal。
/// 持久化为 JSON 文件（key = rel_path），每次扫描后合并进 ScanResult。
pub struct DirOverrideStore {
    path: std::path::PathBuf,
    overrides: std::collections::HashMap<String, crate::domain::DirClass>,
}

impl DirOverrideStore {
    pub fn load(path: &std::path::Path) -> Self {
        let overrides = if path.exists() {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Default::default()
        };
        Self {
            path: path.to_path_buf(),
            overrides,
        }
    }

    pub fn get(&self, rel_path: &str) -> Option<crate::domain::DirClass> {
        self.overrides.get(rel_path).copied()
    }

    pub fn set(&mut self, rel_path: &str, class: crate::domain::DirClass) -> anyhow::Result<()> {
        self.overrides.insert(rel_path.to_string(), class);
        self.save()
    }

    pub fn remove(&mut self, rel_path: &str) -> anyhow::Result<()> {
        self.overrides.remove(rel_path);
        self.save()
    }

    fn save(&self) -> anyhow::Result<()> {
        let s = serde_json::to_string_pretty(&self.overrides)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, s)?;
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &crate::domain::DirClass)> {
        self.overrides.iter()
    }

    pub fn len(&self) -> usize {
        self.overrides.len()
    }
}

/// 应用覆盖到扫描结果（写入 dirs 的 class.override_class，同时刷新 effective）。
pub fn apply_overrides(result: &mut ScanResult, store: &DirOverrideStore) {
    for d in result.dirs.iter_mut() {
        if let Some(c) = store.get(&d.rel_path) {
            d.class.override_class = Some(c);
        }
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;
    use crate::domain::DirClass;

    #[test]
    fn store_roundtrip_and_apply() {
        let dir = std::env::temp_dir().join(format!("ds_ovr_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("overrides.json");
        let mut store = DirOverrideStore::load(&path);
        store.set("绿色软件", DirClass::Atomic).unwrap();
        store.set("电影", DirClass::Container).unwrap();
        assert_eq!(store.get("绿色软件"), Some(DirClass::Atomic));

        // 重新加载 + 应用
        let store2 = DirOverrideStore::load(&path);
        let mut result = scan_with_dirs(&dir);
        apply_overrides(&mut result, &store2);
        assert!(result.dirs.is_empty()); // dir 下无子目录
        assert_eq!(store2.len(), 2);
    }

    #[test]
    fn remove_clears() {
        let dir = std::env::temp_dir().join(format!("ds_ovr2_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("o.json");
        let mut store = DirOverrideStore::load(&path);
        store.set("x", DirClass::Atomic).unwrap();
        store.remove("x").unwrap();
        let store2 = DirOverrideStore::load(&path);
        assert_eq!(store2.get("x"), None);
    }
}
