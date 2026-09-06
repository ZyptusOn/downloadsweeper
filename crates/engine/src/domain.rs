//! 领域类型。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 文件唯一标识。当前用内容哈希的前缀；扫描阶段为空则退化为路径字符串。
pub type FileId = String;

/// 批次号：一次“计划→执行”或一次 Agent 任务用一个批次号贯穿。
pub type BatchId = Uuid;

/// 下载目录中已有目录的视同类型：
/// 决定该目录在整理时是被复用、整体保留（不可拆散）还是可拆散重排。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirClass {
    /// 整理容器：已有整理目录，可复用为本轮整理目标（映射为一级目标目录）。
    Container,
    /// 原子目录：不可拆散，整体移动/改名（绿色软件、素材库、大量同格式小文件）。
    Atomic,
    /// 普通目录：可拆散进本次整理。
    Normal,
}

impl Default for DirClass {
    fn default() -> Self {
        DirClass::Normal
    }
}

impl DirClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            DirClass::Container => "container",
            DirClass::Atomic => "atomic",
            DirClass::Normal => "normal",
        }
    }
}

/// 目录类型判定状态：建议值（扫描启发式） + 用户覆盖值（显式指定）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirClassState {
    pub suggested: DirClass,
    /// 用户显式覆盖后为非 None；优先级高于 suggested。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_class: Option<DirClass>,
}

impl DirClassState {
    pub fn effective(&self) -> DirClass {
        self.override_class.unwrap_or(self.suggested)
    }

    pub fn from_suggested(c: DirClass) -> Self {
        Self {
            suggested: c,
            override_class: None,
        }
    }
}
/// 下载目录中一个已有目录的特征（扫描时计算），供启发式分类与 AI 决策。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub path: PathBuf,
    /// 目录名（不含父路径）。
    pub name: String,
    /// 相对扫描根的路径分段。
    pub rel_path: String,
    /// 直接子文件数（不含递归）。
    pub file_count: usize,
    /// 直接子文件总大小（字节）。
    pub total_size: u64,
    /// 直接子文件扩展名分布：ext -> (count, size)。
    pub formats: HashMap<String, (usize, u64)>,
    /// 是否含可执行/压缩包等"程序型"文件（绿色软件信号）。
    pub has_program_files: bool,
    /// 子目录数量（直接）。
    pub subdir_count: usize,
    /// 子目录名是否呈现"分类名"特征（如"电影/番剧/文档"）。
    pub has_classification_like_subdirs: bool,
    /// 当前类型状态（建议 + 用户覆盖）。
    pub class: DirClassState,
}

/// 扫描结果：文件 + 目录特征。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
    /// 扫描到的所有目录（含根下各级），按路径排序。
    pub dirs: Vec<DirEntry>,
}

impl ScanResult {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn dir(&self, path: &std::path::Path) -> Option<&DirEntry> {
        self.dirs.iter().find(|d| d.path == path)
    }

    pub fn dir_mut(&mut self, path: &std::path::Path) -> Option<&mut DirEntry> {
        self.dirs.iter_mut().find(|d| d.path == path)
    }

    /// 设置用户覆盖的目录类型（写入 class.override_class）。
    pub fn set_dir_class(&mut self, path: &std::path::Path, c: DirClass) -> bool {
        if let Some(d) = self.dir_mut(path) {
            d.class.override_class = Some(c);
            true
        } else {
            false
        }
    }
}

/// 扫描得到的一个文件条目。仅含元数据，不含内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: FileId,
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    /// 小写、不含点号的扩展名，如 `mp4`、`docx`。无扩展名为 `None`。
    pub extension: Option<String>,
    /// 内容哈希（部分哈希，仅扫描/对账时计算）。可空。
    pub content_hash: Option<String>,
}

impl FileEntry {
    /// 文件名（含扩展名）。
    pub fn basename(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// 从路径快速构造一个不含哈希的条目（用于工具内即时读取）。
    pub fn from_path(path: PathBuf) -> std::io::Result<Self> {
        let meta = std::fs::metadata(&path)?;
        let extension = path
            .extension()
            .map(|s| s.to_string_lossy().to_ascii_lowercase());
        Ok(Self {
            id: path.to_string_lossy().into_owned(),
            path,
            size: meta.len(),
            modified: meta.modified()?,
            extension,
            content_hash: None,
        })
    }
}

/// 工具内统一的错误信息字符串（序列化进 trajectory 与返回给 LLM）。
/// 有意保持简单：工具返回 JSON，错误用 `{"error": "..."}` 表达。
pub fn deny(reason: &str) -> serde_json::Value {
    serde_json::json!({ "error": reason })
}
