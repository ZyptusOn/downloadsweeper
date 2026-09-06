//! Authoritative six-stage workflow. Frontends submit edits, never executable paths.
use crate::{cost::Usage, domain::DirClass, permission::PermissionConfig, tree::RuleType};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub type Progress<'a> = dyn Fn(usize, usize, &str) + Send + Sync + 'a;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub parent: String,
    pub name: String,
    pub kind: String,
    pub extension: String,
    pub size: u64,
    pub modified_ms: u64,
    pub class: DirClass,
    pub suggested: DirClass,
    pub reason: String,
    pub direct_files: usize,
    pub total_files: usize,
}
impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == "directory"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    #[serde(default)]
    pub rule_type: RuleType,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub mapping: Option<String>,
    #[serde(default)]
    pub position: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub source: String,
    pub destination: String,
    pub kind: String,
    pub size: u64,
    pub modified_ms: u64,
    pub reason: String,
    pub selected: bool,
    /// pending -> moving -> done -> restoring -> restored; failures never count as success.
    pub status: String,
    pub error: Option<String>,
    /// Versioned digest: full `blake3`, adaptive v1 (16 MiB) / v2 (1 MiB), or legacy SHA-256.
    pub fingerprint: Option<String>,
    /// Metadata snapshot captured while planning a shallow-scanned desktop folder move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_manifest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub id: String,
    pub timestamp: String,
    pub purpose: String,
    pub model: String,
    pub usage: Usage,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub billing: Option<crate::pricing::Charge>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub max_output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
    pub scene: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub kind: String,
    pub target: String,
    pub label: String,
    pub before: Value,
    pub after: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub scene: String,
    pub revision: u64,
    pub message: String,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectionGroup {
    pub id: String,
    pub label: String,
    pub files: usize,
    pub eligible: usize,
    pub withheld: usize,
    pub inspected: usize,
    pub batches: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inspection {
    pub source_key: String,
    pub status: String,
    pub overview: String,
    pub groups: Vec<InspectionGroup>,
    pub protected_files: usize,
    pub batch_size: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationDecision {
    pub source: String,
    pub node_id: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationBatch {
    pub id: String,
    pub branch: String,
    pub files: usize,
    pub status: String,
    pub decisions: Vec<ClassificationDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    pub source_key: String,
    pub status: String,
    pub thinking: bool,
    pub batch_size: usize,
    pub total: usize,
    pub completed: usize,
    pub skipped: usize,
    pub protected_files: usize,
    pub batches: Vec<ClassificationBatch>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub schema_version: u32,
    pub id: Uuid,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub root: PathBuf,
    pub mode: String,
    pub phase: u8,
    pub status: String,
    pub scanned: bool,
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
    pub permissions: PermissionConfig,
    pub nodes: Vec<Node>,
    pub operations: Vec<Operation>,
    pub retained: Vec<Value>,
    pub reviewed: bool,
    #[serde(default)]
    pub plan_source: Option<String>,
    pub calls: Vec<CallRecord>,
    /// Reservations without a confirmed response (including interrupted processes).
    #[serde(default)]
    pub pending_calls: Vec<Value>,
    /// Local run scope for durable responses; never reused by imported tasks.
    #[serde(default)]
    pub runtime_run: Option<Uuid>,
    /// Per-file rename results; does not modify the reviewed operation list.
    #[serde(default)]
    pub rename_checkpoint: Option<Value>,
    pub messages: Vec<ChatTurn>,
    #[serde(default)]
    pub chat_context: Option<Value>,
    pub proposal: Option<Proposal>,
    #[serde(default)]
    pub inspection: Option<Inspection>,
    #[serde(default)]
    pub classification: Option<Classification>,
    #[serde(default)]
    pub review_classification: Option<Classification>,
    pub rename_extensions: Vec<String>,
    #[serde(default)]
    pub rename_web_search: bool,
    #[serde(default)]
    pub search_calls: Vec<Value>,
    pub cleanup: Vec<Value>,
    #[serde(default)]
    pub recycled: Vec<crate::recycle::Record>,
    #[serde(default)]
    pub cleanup_options: crate::cleanup::Options,
    #[serde(default)]
    pub cleanup_summary: Value,
}

impl Task {
    pub fn is_organizing(&self) -> bool {
        matches!(self.mode.as_str(), "organize" | "desktop")
    }
    pub fn new(root: PathBuf, mode: &str, permissions: PermissionConfig) -> Result<Self> {
        ensure!(
            root.is_absolute() && root.is_dir(),
            "请选择存在的绝对目录路径"
        );
        ensure!(
            matches!(mode, "organize" | "rename" | "desktop"),
            "未知任务类型"
        );
        let root = std::fs::canonicalize(root)?;
        let now = chrono::Utc::now().to_rfc3339();
        Ok(Self {
            schema_version: 2,
            id: Uuid::new_v4(),
            revision: 0,
            created_at: now.clone(),
            updated_at: now,
            root,
            mode: mode.into(),
            phase: 0,
            status: "draft".into(),
            scanned: false,
            entries: vec![],
            warnings: vec![],
            permissions,
            nodes: vec![],
            operations: vec![],
            retained: vec![],
            reviewed: false,
            plan_source: None,
            calls: vec![],
            pending_calls: vec![],
            runtime_run: None,
            rename_checkpoint: None,
            messages: vec![],
            chat_context: None,
            proposal: None,
            inspection: None,
            classification: None,
            review_classification: None,
            rename_extensions: vec![],
            rename_web_search: false,
            search_calls: vec![],
            cleanup: vec![],
            recycled: vec![],
            cleanup_options: Default::default(),
            cleanup_summary: Value::Null,
        })
    }
    pub fn usage(&self) -> Usage {
        let mut total = Usage::default();
        for call in &self.calls {
            total += &call.usage;
        }
        total
    }
    pub fn touch(&mut self) {
        self.revision += 1;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
    pub fn imported(mut self) -> Result<Self> {
        ensure!(
            matches!(self.mode.as_str(), "organize" | "rename" | "desktop"),
            "未知任务类型"
        );
        ensure!(
            self.schema_version == 2 && self.root.is_absolute() && self.root.is_dir(),
            "任务版本或目录无效"
        );
        self.root = std::fs::canonicalize(&self.root)?;
        self.id = Uuid::new_v4();
        self.scanned = false;
        self.invalidate(0);
        self.cleanup.clear();
        self.recycled.clear();
        self.cleanup_summary = Value::Null;
        self.chat_context = None;
        self.cleanup_options.validate()?;
        self.pending_calls.clear();
        self.runtime_run = None;
        self.validate_graph(false)?;
        self.validate_permissions()?;
        Ok(self)
    }
    pub fn editable(&self) -> Result<()> {
        ensure!(
            !matches!(
                self.status.as_str(),
                "executing" | "completed" | "partial" | "rolled_back" | "recovery_required"
            ),
            "此任务已执行，请恢复或创建新任务；不能修改已执行的计划"
        );
        Ok(())
    }
    pub fn invalidate(&mut self, phase: u8) {
        if phase <= 1 {
            self.inspection = None;
        }
        self.phase = self.phase.min(phase);
        self.operations.clear();
        self.retained.clear();
        self.classification = None;
        self.review_classification = None;
        self.rename_checkpoint = None;
        self.reviewed = false;
        self.plan_source = None;
        self.proposal = None;
        self.status = "draft".into();
        self.touch();
    }
    pub fn go_back(&mut self, phase: u8) -> Result<()> {
        self.editable()?;
        ensure!(phase < self.phase, "只能返回之前的阶段");
        if phase >= 3 && self.status == "planned" {
            // Planning and review share the same draft; only upstream edits invalidate it.
            self.phase = phase;
            self.reviewed = false;
            self.proposal = None;
            self.touch();
        } else {
            self.invalidate(phase);
        }
        Ok(())
    }
    pub fn scan(&mut self, cancel: &CancellationToken, progress: &Progress<'_>) -> Result<()> {
        self.editable()?;
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        progress(0, 0, "正在扫描目录与文件元数据");
        let walk = walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .max_depth(if self.mode == "desktop" {
                1
            } else {
                usize::MAX
            })
            .into_iter()
            .filter_entry(|e| {
                e.depth() == 0
                    || (!crate::safe_fs::is_link(e.path())
                        && e.file_name() != ".ds-data"
                        && !crate::recycle::is_staging_name(&e.file_name().to_string_lossy()))
            });
        for item in walk {
            ensure!(!cancel.is_cancelled(), "扫描已取消");
            let item = match item {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(e.to_string());
                    continue;
                }
            };
            if item.depth() == 0 || item.file_type().is_symlink() {
                continue;
            }
            let meta = match item.metadata() {
                Ok(m) => m,
                Err(e) => {
                    warnings.push(e.to_string());
                    continue;
                }
            };
            if !meta.is_file() && !meta.is_dir() {
                continue;
            }
            let id = match relative(&self.root, item.path()) {
                Ok(id) => id,
                Err(error) => {
                    warnings.push(error.to_string());
                    continue;
                }
            };
            let parent = id
                .rsplit_once('/')
                .map(|(p, _)| p)
                .unwrap_or("")
                .to_string();
            let name = item.file_name().to_string_lossy().to_string();
            let ext = item
                .path()
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            entries.push(Entry {
                id,
                parent,
                name,
                extension: ext,
                kind: if meta.is_dir() { "directory" } else { "file" }.into(),
                size: if meta.is_file() { meta.len() } else { 0 },
                modified_ms: modified_ms(&meta),
                class: DirClass::Normal,
                suggested: DirClass::Normal,
                reason: String::new(),
                direct_files: 0,
                total_files: 0,
            });
            if entries.len() % 100 == 0 {
                progress(entries.len(), 0, "正在读取目录结构");
            }
        }
        let mut stats: HashMap<String, (usize, u64, HashMap<String, usize>)> = HashMap::new();
        let mut totals: HashMap<String, (usize, u64)> = HashMap::new();
        for f in entries.iter().filter(|e| !e.is_dir()) {
            ensure!(!cancel.is_cancelled(), "扫描已暂停，原快照保留");
            let s = stats.entry(f.parent.clone()).or_default();
            s.0 += 1;
            s.1 += f.size;
            *s.2.entry(f.extension.clone()).or_default() += 1;
            let mut parent = f.parent.as_str();
            while !parent.is_empty() {
                let s = totals.entry(parent.to_string()).or_default();
                s.0 += 1;
                s.1 += f.size;
                parent = parent.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            }
        }
        let categories = [
            "视频",
            "电影",
            "番剧",
            "文档",
            "图片",
            "音乐",
            "音频",
            "软件",
            "压缩包",
            "资料",
            "videos",
            "movies",
            "documents",
            "pictures",
            "music",
        ];
        for d in entries.iter_mut().filter(|e| e.is_dir()) {
            ensure!(!cancel.is_cancelled(), "扫描已暂停，原快照保留");
            let s = stats.get(&d.id).cloned().unwrap_or_default();
            let total = totals.get(&d.id).copied().unwrap_or_default();
            d.direct_files = s.0;
            d.total_files = total.0;
            d.size = total.1;
            let program = ["exe", "dll", "so", "dylib"]
                .iter()
                .any(|ext| s.2.contains_key(*ext));
            let many = s.0 >= 500 && s.2.values().max().copied().unwrap_or(0) >= s.0 / 2;
            let bundle = d.extension == "app";
            let (class, reason) = if self.mode == "desktop" {
                (DirClass::Atomic, "桌面模式：可按权限识别并整体移动，不拆分内部内容")
            } else if program || bundle {
                (DirClass::Atomic, "检测到程序文件或应用包，建议整体保护")
            } else if many {
                (DirClass::Atomic, "大量同格式小文件，建议作为素材库整体保护")
            } else if categories.contains(&d.name.to_lowercase().as_str()) {
                (DirClass::Container, "目录名符合已有分类，建议复用")
            } else {
                (DirClass::Normal, "普通目录，可逐个整理内部文件")
            };
            d.class = class;
            d.suggested = class;
            d.reason = reason.into();
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        // Rescanning refreshes the snapshot, without discarding the user's rules or
        // imported context. Only explicit directory overrides survive detection.
        for entry in entries.iter_mut().filter(|e| e.is_dir()) {
            ensure!(!cancel.is_cancelled(), "扫描已暂停，原快照保留");
            if let Some(old) = self
                .entries
                .iter()
                .find(|old| old.id == entry.id && old.is_dir() && old.class != old.suggested)
            {
                if self.mode != "desktop" || old.class == DirClass::Container {
                    entry.class = old.class;
                }
            }
        }
        ensure!(!cancel.is_cancelled(), "扫描已暂停，原快照保留");
        self.entries = entries;
        self.warnings = warnings;
        self.scanned = true;
        self.invalidate(0);
        self.phase = 0;
        if self.nodes.is_empty() {
            self.nodes = if self.mode == "desktop" {
                desktop_template(&self.entries)
            } else {
                template(&self.entries)
            };
        } else {
            for node in &mut self.nodes {
                node.examples
                    .retain(|id| self.entries.iter().any(|e| &e.id == id && !e.is_dir()));
                if node.mapping.as_ref().is_some_and(|id| {
                    !self
                        .entries
                        .iter()
                        .any(|e| &e.id == id && e.is_dir() && e.class == DirClass::Container)
                }) {
                    node.mapping = None;
                }
            }
        }
        progress(
            self.entries.len(),
            self.entries.len(),
            "扫描完成；尚未读取文件内容",
        );
        Ok(())
    }
    pub fn set_directory(&mut self, id: &str, class: DirClass) -> Result<()> {
        self.editable()?;
        ensure!(
            self.mode != "desktop" || matches!(class, DirClass::Atomic | DirClass::Container),
            "桌面模式允许整体保护或复用容器；不允许拆散内部文件"
        );
        ensure!(self.scanned, "请先扫描");
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.id == id && e.is_dir())
            .context("目录不在本次扫描中")?;
        e.class = class;
        self.inspection = None;
        if class != DirClass::Container {
            for n in &mut self.nodes {
                if n.mapping.as_deref() == Some(id) {
                    n.mapping = None;
                }
            }
        }
        self.invalidate(2);
        Ok(())
    }
    pub fn advance(&mut self) -> Result<()> {
        self.editable()?;
        match self.phase {
            0 => {
                ensure!(self.scanned, "请先完成扫描");
                self.phase = 1;
            }
            1 => {
                self.validate_permissions()?;
                self.phase = 2;
            }
            2 => {
                if self.mode == "rename" {
                    ensure!(
                        !self.rename_extensions.is_empty(),
                        "请先选择要重生的文件类别"
                    );
                } else {
                    self.validate_graph(true)?;
                }
                self.phase = 3;
            }
            3 => {
                ensure!(self.status == "planned", "请先生成具体计划");
                self.phase = 4;
                self.reviewed = false;
            }
            4 => {
                ensure!(self.reviewed, "请先确认已审查计划");
                self.phase = 5;
            }
            _ => bail!("已到执行阶段"),
        }
        self.touch();
        Ok(())
    }
    pub fn validate_permissions(&self) -> Result<()> {
        ensure!(
            self.permissions.content_slice_bytes <= 65536,
            "内容切片上限为 65536 字节"
        );
        for r in &self.permissions.rules {
            ensure!(
                r.category.as_ref().is_none_or(|c| c.len() <= 80),
                "权限规则类别名称过长"
            );
            ensure!(
                r.min_bytes.unwrap_or(0) <= r.max_bytes.unwrap_or(u64::MAX),
                "权限规则大小区间无效"
            );
        }
        Ok(())
    }
    pub fn active(&self, id: &str) -> bool {
        let mut seen = HashSet::new();
        let mut current = id;
        loop {
            if current == "root" {
                return true;
            }
            if !seen.insert(current) {
                return false;
            }
            match self
                .nodes
                .iter()
                .find(|n| n.id == current)
                .and_then(|n| n.parent.as_deref())
            {
                Some(p) => current = p,
                None => return false,
            }
        }
    }
    pub fn ancestry(&self, id: &str) -> Result<Vec<&Node>> {
        let mut path = Vec::new();
        let mut current = id;
        let mut seen = HashSet::new();
        while current != "root" {
            ensure!(seen.insert(current), "目录图不能形成环");
            let node = self
                .nodes
                .iter()
                .find(|n| n.id == current)
                .context("节点不存在")?;
            path.push(node);
            current = node.parent.as_deref().context("孤儿节点不参与整理")?;
        }
        path.reverse();
        Ok(path)
    }
    pub fn node_path(&self, id: &str) -> Result<String> {
        let parts = self.ancestry(id)?;
        Ok(parts
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if i == 0 {
                    n.mapping.as_deref().unwrap_or(&n.name)
                } else {
                    &n.name
                }
            })
            .collect::<Vec<_>>()
            .join("/"))
    }
    pub fn validate_graph(&self, ready: bool) -> Result<()> {
        ensure!(self.nodes.len() <= 2000, "目录节点最多 2000 个");
        let ids: HashSet<_> = self.nodes.iter().map(|n| &n.id).collect();
        ensure!(
            ids.len() == self.nodes.len() && !ids.contains(&"root".to_string()),
            "节点 ID 重复或使用了保留 ID"
        );
        let mut siblings = HashSet::new();
        let mut exts = HashSet::new();
        let mut mappings = HashSet::new();
        for node in &self.nodes {
            valid_name(&node.name)?;
            ensure!(node.id.len() <= 128 && !node.id.is_empty(), "节点 ID 无效");
            ensure!(
                node.note.len() <= 16000 && node.examples.len() <= 30,
                "备注或示例过多"
            );
            if let Some(p) = &node.parent {
                ensure!(p == "root" || ids.contains(p), "父节点不存在");
            }
            let mut seen = HashSet::new();
            let mut current = Some(node.id.as_str());
            while let Some(id) = current {
                ensure!(seen.insert(id), "目录图不能形成环");
                ensure!(seen.len() <= 64, "目录层级不能超过 64");
                current = self
                    .nodes
                    .iter()
                    .find(|n| n.id == id)
                    .and_then(|n| n.parent.as_deref());
            }
            if node.parent.is_some() {
                ensure!(
                    siblings.insert((node.parent.clone(), node.name.to_lowercase())),
                    "同一父目录下的文件夹名称重复：{}",
                    node.name
                );
            }
            if let Some(pos) = node.position {
                ensure!(
                    pos.iter().all(|v| v.is_finite() && v.abs() < 1e7),
                    "节点坐标无效"
                );
            }
            for example in &node.examples {
                ensure!(
                    self.entries.iter().any(|e| &e.id == example && !e.is_dir()),
                    "示例必须是本次扫描中的具体文件"
                );
            }
            if ready && self.active(&node.id) && node.rule_type == RuleType::Simple {
                ensure!(
                    !node.extensions.is_empty(),
                    "简单规则目录「{}」尚未设置扩展名",
                    node.name
                );
            }
            if node.parent.as_deref() == Some("root") {
                ensure!(
                    node.rule_type == RuleType::Simple,
                    "一级目录只能使用扩展名规则"
                );
                if ready {
                    ensure!(
                        !node.extensions.is_empty(),
                        "一级目录「{}」尚未设置扩展名",
                        node.name
                    );
                }
                for ext in &node.extensions {
                    ensure!(
                        ext == "*"
                            || ext == "@folder"
                            || (ext.len() <= 32
                                && ext
                                    .chars()
                                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')),
                        "扩展名无效：{ext}"
                    );
                    ensure!(
                        exts.insert(ext.to_lowercase()),
                        "一级目录的扩展名不能重复：{ext}"
                    );
                }
                if let Some(mapping) = &node.mapping {
                    ensure!(!mapping.contains('/'), "只能映射到已有的一级目录");
                    ensure!(
                        self.entries.iter().any(|e| e.id == *mapping
                            && e.is_dir()
                            && e.class == DirClass::Container),
                        "映射目录必须标记为复用容器"
                    );
                    ensure!(
                        mappings.insert(mapping.to_lowercase()),
                        "同一已有目录不能映射给两个分类"
                    );
                }
            } else {
                ensure!(node.mapping.is_none(), "仅一级目录支持映射");
            }
        }
        if ready {
            ensure!(
                self.nodes
                    .iter()
                    .any(|n| n.parent.as_deref() == Some("root")),
                "请至少连接一个一级分类"
            );
        }
        // Detect aliases produced by mapping vs normal folder names.
        let mut paths = HashSet::new();
        for n in self.nodes.iter().filter(|n| self.active(&n.id)) {
            ensure!(
                paths.insert(self.node_path(&n.id)?.to_lowercase()),
                "目标路径重复，请检查目录映射"
            );
        }
        Ok(())
    }
    pub fn candidates(&self) -> Vec<&Entry> {
        let atomic: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.is_dir() && e.class == DirClass::Atomic)
            .collect();
        self.entries
            .iter()
            .filter(|e| {
                (!e.is_dir() || e.class == DirClass::Atomic)
                    && !atomic.iter().any(|d| d.id != e.id && under(&e.id, &d.id))
            })
            .collect()
    }
    pub fn rule_node(&self, entry: &Entry) -> Option<&Node> {
        let ext = if entry.is_dir() {
            "@folder"
        } else {
            &entry.extension
        };
        let top = self
            .nodes
            .iter()
            .find(|n| {
                n.parent.as_deref() == Some("root")
                    && n.extensions.iter().any(|x| x.eq_ignore_ascii_case(ext))
            })
            .or_else(|| {
                self.nodes.iter().find(|n| {
                    n.parent.as_deref() == Some("root") && n.extensions.contains(&"*".to_string())
                })
            })?;
        self.descend_simple(&top.id, ext)
    }

    pub fn descend_simple(&self, node_id: &str, ext: &str) -> Option<&Node> {
        let mut node = self.nodes.iter().find(|n| n.id == node_id)?;
        while let Some(child) = self.nodes.iter().find(|n| {
            n.parent.as_deref() == Some(&node.id)
                && n.rule_type == RuleType::Simple
                && n.extensions.iter().any(|x| x.eq_ignore_ascii_case(ext))
        }) {
            node = child;
        }
        Some(node)
    }

    /// Every simple ancestor remains a hard format constraint, even below an AI node.
    pub fn semantic_options(&self, entry: &Entry) -> Vec<&Node> {
        if entry.is_dir() && entry.class == DirClass::Atomic {
            // A folder is a single semantic item, not a file with the @folder extension.
            // Its contents may belong to any first-level format category (e.g. spreadsheets).
            return self.nodes.iter().filter(|n| {
                n.parent.as_deref() == Some("root") && self.node_path(&n.id).is_ok_and(|path| {
                    path != entry.id && !under(&path, &entry.id)
                })
            }).collect();
        }
        let Some(base) = self.rule_node(entry) else {
            return vec![];
        };
        self.nodes
            .iter()
            .filter(|n| {
                n.rule_type == RuleType::Complex
                    && self.active(&n.id)
                    && self.ancestry(&n.id).is_ok_and(|anc| {
                        anc.iter().any(|a| a.id == base.id)
                            && anc
                                .iter()
                                .filter(|a| {
                                    a.rule_type == RuleType::Simple
                                        && a.parent.as_deref() != Some("root")
                                })
                                .all(|a| {
                                    a.extensions
                                        .iter()
                                        .any(|x| x.eq_ignore_ascii_case(&entry.extension))
                                })
                    })
            })
            .collect()
    }
    pub fn generate_rules(
        &mut self,
        cancel: &CancellationToken,
        progress: &Progress<'_>,
    ) -> Result<()> {
        self.editable()?;
        ensure!(
            matches!(self.mode.as_str(), "organize" | "desktop"),
            "命名任务请生成重命名计划"
        );
        ensure!(
            self.phase == 3 || self.phase == 4,
            "请完成目标结构设置后再生成计划"
        );
        self.validate_graph(true)?;
        let mut operations = vec![];
        let mut retained = vec![];
        let mut reserved = HashSet::new();
        let candidates = self.candidates();
        for (i, e) in candidates.iter().enumerate() {
            ensure!(!cancel.is_cancelled(), "规划已取消");
            if self.mode == "desktop" && desktop_retained(e) {
                retained.push(json!({"source":e.id,"reason":"桌面保护：快捷方式、隐藏或临时文件保留原位"}));
                continue;
            }
            if self.mode == "desktop" && e.is_dir() {
                retained.push(json!({"source":e.id,"reason":"完整文件夹等待 AI 按名称和获准内容归类；不拆分内部文件"}));
                continue;
            }
            progress(
                i,
                candidates.len(),
                "按扩展名分类；保护整体目录并复用已有结构",
            );
            if ["crdownload", "part", "download", "tmp"].contains(&e.extension.as_str()) {
                retained.push(json!({"source":e.id,"reason":"可能正在下载或写入，保留原位"}));
                continue;
            }
            let Some(node) = self.rule_node(e) else {
                retained.push(json!({"source":e.id,"reason":"没有匹配规则，保留原位"}));
                continue;
            };
            let folder = self.node_path(&node.id)?;
            let top = self.ancestry(&node.id)?[0];
            let mapped = top.mapping.as_ref();
            if mapped.is_some_and(|m| under(&e.id, m)) {
                retained.push(json!({"source":e.id,"reason":"已在复用目录中，保留现有内部结构"}));
                continue;
            }
            let destination = format!("{folder}/{}", e.name);
            if destination == e.id {
                retained.push(json!({"source":e.id,"reason":"已在正确位置"}));
                continue;
            }
            if e.is_dir() && under(&destination, &e.id) {
                retained.push(json!({"source":e.id,"reason":"目标位于自身内部，保留整体目录"}));
                continue;
            }
            let destination = collision_path(&self.root, &destination, &mut reserved)?;
            operations.push(Operation {
                id: Uuid::new_v4().to_string(),
                source: e.id.clone(),
                destination,
                kind: e.kind.clone(),
                size: e.size,
                modified_ms: e.modified_ms,
                reason: if e.is_dir() {
                    "整体移动，保留内部结构"
                } else {
                    "扩展名规则"
                }
                .into(),
                selected: true,
                status: "pending".into(),
                error: None,
                fingerprint: None,
                directory_manifest: None,
            });
        }
        ensure!(!cancel.is_cancelled(), "规划已暂停，原计划保留");
        self.operations = operations;
        self.retained = retained;
        self.reviewed = false;
        self.plan_source = Some("rules".into());
        self.classification = None;
        self.review_classification = None;
        self.status = "planned".into();
        self.touch();
        progress(
            self.operations.len(),
            self.operations.len(),
            "基础规则计划已生成，可继续 AI 细化或进入审查",
        );
        Ok(())
    }
    /// Compact desktop workflow: no content reads or model calls, with atomic draft commit.
    pub fn prepare_desktop(
        &mut self,
        cancel: &CancellationToken,
        progress: &Progress<'_>,
    ) -> Result<()> {
        self.editable()?;
        ensure!(
            self.mode == "desktop" && self.scanned && self.phase < 5,
            "请先扫描桌面目录"
        );
        let mut draft = self.clone();
        draft.validate_permissions()?;
        // Compatibility shortcut: preserve the user's target tree.
        // GUI and CLI now use the full six-stage Agent workflow.
        draft.phase = 3;
        draft.generate_rules(cancel, progress)?;
        draft.advance()?;
        ensure!(!cancel.is_cancelled(), "规划已暂停，原计划保留");
        *self = draft;
        Ok(())
    }
    pub fn apply_proposal(&mut self, proposal_id: &str, ids: &[String], scene: &str) -> Result<()> {
        self.apply_proposal_with_progress(proposal_id, ids, scene, &CancellationToken::new(), &|_, _, _| {})
    }
    pub fn apply_proposal_with_progress(&mut self, proposal_id: &str, ids: &[String], scene: &str, cancel: &CancellationToken, progress: &Progress<'_>) -> Result<()> {
        self.editable()?;
        let p = self.proposal.clone().context("没有待处理建议")?;
        ensure!(
            p.id == proposal_id && p.scene == scene && p.revision == self.revision,
            "建议已过期或不属于当前场景，请重新生成"
        );
        ensure!(
            scene == self.scene() || (scene == "directories" && self.phase == 2),
            "只能修改当前可见阶段"
        );
        ensure!(
            !ids.is_empty() && ids.iter().all(|id| p.changes.iter().any(|c| &c.id == id)),
            "请选择有效的建议改动"
        );
        let mut edited = self.clone();
        // Apply the selected graph as a whole: deleting a former parent must not
        // undo an explicit child reparenting just because changes arrived out of order.
        let deleted: HashSet<_> = p
            .changes
            .iter()
            .filter(|c| ids.contains(&c.id) && c.kind == "node" && c.after.is_null())
            .map(|c| c.target.as_str())
            .collect();
        for change in p.changes.iter().filter(|c| ids.contains(&c.id)) {
            match (scene, change.kind.as_str()) {
                ("tree" | "review", "node") => {
                    let old = edited.nodes.iter().position(|n| n.id == change.target);
                    if change.after.is_null() {
                        if let Some(i) = old {
                            edited.nodes.remove(i);
                        }
                    } else {
                        let node: Node = serde_json::from_value(change.after.clone())?;
                        ensure!(node.id == change.target, "建议节点 ID 不一致");
                        if let Some(i) = old {
                            edited.nodes[i] = node;
                        } else {
                            edited.nodes.push(node);
                        }
                    }
                }
                ("permissions", "permissions") => {
                    edited.inspection = None;
                    edited.permissions = serde_json::from_value(change.after.clone())?;
                }
                ("permissions" | "directories", "directory") => {
                    edited.inspection = None;
                    let class: DirClass = serde_json::from_value(change.after.clone())?;
                    ensure!(
                        edited.mode != "desktop"
                            || matches!(class, DirClass::Atomic | DirClass::Container),
                        "桌面模式仅允许整体保护或复用容器，不能拆散内部文件"
                    );
                    let e = edited
                        .entries
                        .iter_mut()
                        .find(|e| e.id == change.target && e.is_dir())
                        .context("目录不存在")?;
                    e.class = class;
                    if class != DirClass::Container {
                        for node in &mut edited.nodes {
                            if node.mapping.as_deref() == Some(&change.target) {
                                node.mapping = None;
                            }
                        }
                    }
                }
                ("review", "placement") => {}, // Validated against the final graph below.
                _ => bail!("AI 建议越过当前场景权限"),
            }
        }
        for node in &mut edited.nodes {
            if node
                .parent
                .as_deref()
                .is_some_and(|id| deleted.contains(id))
            {
                node.parent = None;
            }
        }
        for node in &edited.nodes {
            if let Some(parent) = &node.parent {
                if parent != "root" && !edited.nodes.iter().any(|n| &n.id == parent) {
                    if let Some(change) = p.changes.iter().find(|c| {
                        c.kind == "node"
                            && &c.target == parent
                            && c.before.is_null()
                            && !c.after.is_null()
                    }) {
                        bail!(
                            "「{}」依赖新增父节点「{}」，请同时勾选其父节点后合并",
                            node.name,
                            change.label
                        );
                    }
                }
            }
        }
        edited.validate_permissions()?;
        edited.validate_graph(false)?;
        if scene == "review" {
            crate::workflow_ai::revise_plan(self, &mut edited, &p.changes.iter().filter(|c| ids.contains(&c.id)).cloned().collect::<Vec<_>>(), cancel, progress)?;
        } else {
            edited.invalidate(self.phase);
        }
        *self = edited;
        Ok(())
    }
    pub fn scene(&self) -> &str {
        match self.phase {
            0 => "scan",
            1 => "permissions",
            2 => "tree",
            3 => "planning",
            4 => "review",
            _ => "execution",
        }
    }
    pub fn prepare_cleanup(&mut self) {
        crate::cleanup::prepare(self, chrono::Utc::now().timestamp_millis().max(0) as u64);
    }
}

pub fn relative(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)?
        .components()
        .map(|part| match part {
            std::path::Component::Normal(name) => name
                .to_str()
                .map(str::to_owned)
                .context("文件名不是有效 UTF-8，不能安全表示"),
            _ => anyhow::bail!("相对路径无效"),
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}
pub fn under(path: &str, parent: &str) -> bool {
    path.starts_with(&format!("{parent}/"))
}
pub fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn valid_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 240 && name != "." && name != "..",
        "文件夹或文件名称无效"
    );
    ensure!(
        !name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
            && !name.ends_with([' ', '.']),
        "名称含 Windows / macOS 不支持的字符：{name}"
    );
    let base = name.split('.').next().unwrap_or("").to_uppercase();
    ensure!(
        ![
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"
        ]
        .contains(&base.as_str()),
        "系统保留名称：{name}"
    );
    Ok(())
}
pub fn safe_relative(value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "相对路径无效");
    for part in value.split('/') {
        #[cfg(windows)]
        valid_name(part)?;
        #[cfg(not(windows))]
        ensure!(
            !part.is_empty() && part != "." && part != ".." && !part.contains('\0'),
            "相对路径无效"
        );
    }
    Ok(())
}
pub fn collision_path(
    root: &Path,
    candidate: &str,
    reserved: &mut HashSet<String>,
) -> Result<String> {
    safe_relative(candidate)?;
    let path = Path::new(candidate);
    let parent = path.parent().unwrap_or(Path::new(""));
    let stem = path.file_stem().unwrap().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 0..100_000 {
        let rel = if i == 0 {
            candidate.to_string()
        } else {
            relative(Path::new(""), &parent.join(format!("{stem} ({i}){ext}")))?
        };
        if !root.join(&rel).try_exists()? && reserved.insert(rel.to_lowercase()) {
            return Ok(rel);
        }
    }
    bail!("同名文件过多")
}

/// Entries outside the shallow snapshot never become desktop move candidates.
pub fn desktop_retained(entry: &Entry) -> bool {
    !entry.parent.is_empty()
        || entry.name.starts_with('.')
        || entry.name.starts_with("~$")
        || matches!(
            entry.name.to_lowercase().as_str(),
            "desktop.ini" | "thumbs.db"
        )
        || [
            "lnk",
            "url",
            "webloc",
            "alias",
            "app",
            "crdownload",
            "part",
            "download",
            "tmp",
            "temp",
            "swp",
        ]
        .contains(&entry.extension.as_str())
}

fn desktop_template(entries: &[Entry]) -> Vec<Node> {
    let definitions: &[(&str, &str, &[&str])] = &[
        ("text", "文本", &["txt", "md", "rtf", "log"]),
        (
            "office",
            "文档",
            &[
                "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "csv", "pages",
                "numbers", "key",
            ],
        ),
        ("pdf", "PDF", &["pdf"]),
        (
            "images",
            "图片",
            &[
                "png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "bmp", "tif", "tiff", "svg",
            ],
        ),
        ("other", "其他", &["*"]),
    ];
    definitions
        .iter()
        .filter(|(id, _, exts)| {
            *id == "other"
                || (*id == "office" && entries.iter().any(|e| e.is_dir() && !desktop_retained(e)))
                || entries
                    .iter()
                    .any(|e| !desktop_retained(e) && exts.contains(&e.extension.as_str()))
        })
        .flat_map(|(id, name, exts)| {
            // Existing desktop folders remain untouched, including a previous output folder.
            let mut name = name.to_string();
            let mut suffix = 2;
            while entries.iter().any(|e| e.name.eq_ignore_ascii_case(&name)) {
                name = format!("{} ({suffix})", name.split(" (").next().unwrap());
                suffix += 1;
            }
            let top = Node {
                id: format!("desktop-{id}"),
                parent: Some("root".into()),
                name,
                rule_type: RuleType::Simple,
                extensions: exts.iter().map(|s| s.to_string()).collect(),
                note: String::new(),
                examples: vec![],
                mapping: None,
                position: None,
            };
            // The default desktop tree must also offer semantic destinations to the Agent.
            // Rule-only planning still stops at the top node; children require AI decisions.
            let children = match *id {
                "images" => [("照片", "人物、生活与实拍照片"), ("参考素材", "截图、图标、设计与工作参考素材")],
                "other" => [("工作资料", "工作项目所需的资料与资源"), ("个人资料", "个人生活、兴趣与学习资源")],
                _ => [("工作资料", "合同、报告、会议、业务与项目资料"), ("学习与个人资料", "课程、论文、笔记与个人生活资料")],
            };
            let mut nodes = vec![top];
            for (index, (name, note)) in children.into_iter().enumerate() {
                nodes.push(Node {
                    id: format!("desktop-{id}-semantic-{index}"),
                    parent: Some(format!("desktop-{id}")),
                    name: name.into(),
                    rule_type: RuleType::Complex,
                    extensions: vec![],
                    note: note.into(),
                    examples: vec![],
                    mapping: None,
                    position: None,
                });
            }
            nodes
        })
        .collect()
}

pub fn template(entries: &[Entry]) -> Vec<Node> {
    let definitions = [
        (
            "video",
            "视频",
            vec!["mp4", "mkv", "avi", "mov", "webm", "flv", "m4v"],
            vec![
                ("电影", "完整长片电影"),
                ("番剧", "动画剧集与动画剧场版"),
                ("剪辑素材", "短视频片段、绿幕、转场素材"),
            ],
        ),
        (
            "docs",
            "文档",
            vec![
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "csv", "json",
            ],
            vec![
                ("工作资料", "合同、报告、会议材料"),
                ("学习资料", "课程、笔记、论文与教程"),
            ],
        ),
        (
            "images",
            "图片",
            vec![
                "jpg", "jpeg", "png", "gif", "webp", "heic", "svg", "bmp", "tif", "tiff",
            ],
            vec![
                ("照片", "实拍照片"),
                ("设计素材", "插画、图标、截图、设计参考"),
            ],
        ),
        (
            "audio",
            "音频",
            vec!["mp3", "wav", "flac", "aac", "m4a", "ogg"],
            vec![],
        ),
        (
            "archives",
            "压缩包",
            vec!["zip", "rar", "7z", "tar", "gz", "iso"],
            vec![],
        ),
        (
            "apps",
            "软件",
            vec!["exe", "msi", "dmg", "pkg", "deb", "rpm"],
            vec![],
        ),
        ("folders", "完整文件夹", vec!["@folder"], vec![]),
        ("other", "其他", vec!["*"], vec![]),
    ];
    let mut nodes = vec![];
    for (id, name, exts, children) in definitions {
        if id != "other"
            && !entries.iter().any(|e| {
                if id == "folders" {
                    e.is_dir() && e.class == DirClass::Atomic
                } else {
                    !e.is_dir() && exts.contains(&e.extension.as_str())
                }
            })
        {
            continue;
        }
        let mapping = entries
            .iter()
            .find(|e| {
                e.is_dir()
                    && e.parent.is_empty()
                    && e.class == DirClass::Container
                    && e.name == name
            })
            .map(|e| e.id.clone());
        nodes.push(Node {
            id: id.into(),
            parent: Some("root".into()),
            name: name.into(),
            rule_type: RuleType::Simple,
            extensions: exts.iter().map(|s| s.to_string()).collect(),
            note: String::new(),
            examples: vec![],
            mapping,
            position: None,
        });
        for (i, (label, note)) in children.iter().enumerate() {
            nodes.push(Node {
                id: format!("{id}-{i}"),
                parent: Some(id.into()),
                name: label.to_string(),
                rule_type: RuleType::Complex,
                extensions: vec![],
                note: note.to_string(),
                examples: vec![],
                mapping: None,
                position: None,
            });
        }
    }
    nodes
}

pub fn extension_summary(entries: &[Entry]) -> BTreeMap<String, (usize, u64)> {
    let mut result = BTreeMap::new();
    for e in entries.iter().filter(|e| !e.is_dir()) {
        let s = result.entry(e.extension.clone()).or_insert((0, 0));
        s.0 += 1;
        s.1 += e.size;
    }
    result
}
