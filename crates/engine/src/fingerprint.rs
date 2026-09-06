//! 内容指纹与分类缓存。
//!
//! 指纹 = SHA-256(文件大小 + 前 64KB + 后 64KB)。大小、头部或尾部变化即视为不同文件，
//! 相比单取头部更能区分"头部相同但内容不同"的文件（如不同缩略图、带尾部签名的大文件）。
//! 缓存键为指纹，值为分类结果。文件未变则复用缓存，不重复调用 LLM（零成本）。
//!
//! 缓存持久化为 JSON 文件，重启后复用。

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

/// 内容指纹（hex 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentFingerprint(pub String);

/// 单侧刷取的字节数。
const TAIL_BYTES: usize = 65536;

/// 计算 (文件大小 + 前 64KB + 后 64KB) 的 SHA-256 指纹。
/// 文件小于 2×64KB 时整体参与哈希；> 128KB 时前后各取 64KB。
pub fn fingerprint(path: &Path) -> Result<ContentFingerprint> {
    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    hasher.update(size.to_le_bytes());

    let mut head = vec![0u8; TAIL_BYTES];
    let n_head = file.read(&mut head)?;
    head.truncate(n_head);
    hasher.update(&head);

    // 文件超过一个 TAIL 时，再取尾部
    if size > TAIL_BYTES as u64 * 2 {
        file.seek(SeekFrom::End(-(TAIL_BYTES as i64)))?;
        let mut tail = vec![0u8; TAIL_BYTES];
        let n_tail = file.read(&mut tail)?;
        tail.truncate(n_tail);
        hasher.update(&tail);
    }

    Ok(ContentFingerprint(format!("{:x}", hasher.finalize())))
}

/// 一条缓存条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedEntry {
    pub category: String,
    pub subfolder: String,
    pub classified_at: String,
    /// "rule" | "llm"
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, CachedEntry>,
}

/// 线程安全的分类缓存。
pub struct ClassificationCache {
    entries: Mutex<HashMap<String, CachedEntry>>,
    path: PathBuf,
}

impl ClassificationCache {
    /// 从文件加载缓存；文件不存在则创建空缓存。
    pub fn load(path: &Path) -> Result<Self> {
        let entries = if path.exists() {
            let s = std::fs::read_to_string(path)?;
            let file: CacheFile = serde_json::from_str(&s).unwrap_or_default();
            file.entries
        } else {
            HashMap::new()
        };
        Ok(Self {
            entries: Mutex::new(entries),
            path: path.to_path_buf(),
        })
    }

    pub fn get(&self, fp: &ContentFingerprint) -> Option<CachedEntry> {
        let entries = self.entries.lock().expect("cache mutex poisoned");
        entries.get(&fp.0).cloned()
    }

    pub fn put(&self, fp: ContentFingerprint, entry: CachedEntry) {
        let mut entries = self.entries.lock().expect("cache mutex poisoned");
        entries.insert(fp.0, entry);
    }

    /// 持久化到磁盘。
    pub fn save(&self) -> Result<()> {
        let entries = self.entries.lock().expect("cache mutex poisoned");
        let file = CacheFile {
            version: 1,
            entries: entries.clone(),
        };
        let s = serde_json::to_string_pretty(&file)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, s)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.lock().expect("cache mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 构造一个带当前时间戳的 CachedEntry。
pub fn cached_entry(category: &str, subfolder: &str, source: &str) -> CachedEntry {
    CachedEntry {
        category: category.to_string(),
        subfolder: subfolder.to_string(),
        classified_at: Utc::now().to_rfc3339(),
        source: source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_stable_and_size_sensitive() {
        let dir = std::env::temp_dir().join(format!("ds_fp_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"hello world").unwrap();
        let fp1 = fingerprint(&f).unwrap();
        let fp2 = fingerprint(&f).unwrap();
        assert_eq!(fp1, fp2, "同一文件指纹应稳定");

        // 内容变化（大小变化）→ 指纹变化
        std::fs::write(&f, b"hello world!").unwrap();
        let fp3 = fingerprint(&f).unwrap();
        assert_ne!(fp1, fp3, "内容变化后指纹应不同");
    }

    #[test]
    fn fingerprint_tail_sensitive_for_large_files() {
        let dir = std::env::temp_dir().join(format!("ds_fp_tail_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // 构造 > 128KB 的两文件：头部完全相同，仅在接近文件末尾处不同
        let f1 = dir.join("big1.bin");
        let f2 = dir.join("big2.bin");
        let seed = vec![0x41u8; 70 * 1024]; // 70KB 单一字符
        let mut d1 = seed.clone();
        let mut d2 = seed.clone();
        // 尾部各 70KB，只有尾部最后一段不同 → 总长 140KB > 128KB，尾部参与哈希
        d1.extend_from_slice(&vec![0xFFu8; 70 * 1024]);
        d2.extend_from_slice(&vec![0x00u8; 70 * 1024]);
        std::fs::write(&f1, &d1).unwrap();
        std::fs::write(&f2, &d2).unwrap();
        assert_eq!(d1.len(), d2.len()); // 两文件大小相同
        let fp1 = fingerprint(&f1).unwrap();
        let fp2 = fingerprint(&f2).unwrap();
        assert_ne!(fp1, fp2, "头部相同仅尾部不同的大文件，指纹应不同");
    }

    #[test]
    fn cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ds_cache_{}", uuid::Uuid::new_v4()));
        let path = dir.join("cache.json");
        let cache = ClassificationCache::load(&path).unwrap();
        let fp = ContentFingerprint("abc123".into());
        cache.put(fp.clone(), cached_entry("番剧", "番剧", "llm"));
        assert_eq!(cache.len(), 1);
        cache.save().unwrap();

        // 重新加载
        let cache2 = ClassificationCache::load(&path).unwrap();
        assert_eq!(cache2.len(), 1);
        let entry = cache2.get(&fp).unwrap();
        assert_eq!(entry.category, "番剧");
        assert_eq!(entry.source, "llm");
    }
}
