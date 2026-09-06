//! 工具层：Agent 通过工具调用访问文件系统。
//!
//! 权限在工具内部依据 `PermissionConfig`（独立于 LLM）强制执行：
//! 读内容类工具先查 `tier_for(ext)`，低于所需层级则返回 `{"error":...}`，
//! LLM 根本拿不到被裁掉的数据。这是整个系统的隐私 choke point。

pub mod fs_tools;
pub mod organize;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::domain::BatchId;
use crate::llm::ToolDef;
use crate::permission::PermissionConfig;
use crate::trajectory::TrajectoryLogger;
use crate::Result;

/// 工具执行上下文，所有工具共享。线程安全。
pub struct ToolContext {
    pub permissions: PermissionConfig,
    pub scan_root: PathBuf,
    pub trajectory: Arc<TrajectoryLogger>,
}

/// 单个工具接口。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具定义（名称、描述、参数 schema），用于注册给 LLM。
    fn def(&self) -> ToolDef;
    /// 执行工具。`args` 为已解析的 JSON 参数。
    async fn call(
        &self,
        args: Value,
        ctx: &ToolContext,
        cancel: &CancellationToken,
    ) -> Result<Value>;
}

/// 工具注册表。
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: vec![] }
    }

    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(fs_tools::ScanDownloadsTool));
        r.register(Arc::new(fs_tools::ReadContentSliceTool));
        r.register(Arc::new(organize::ClassifyByRulesTool));
        r
    }

    pub fn register(&mut self, t: Arc<dyn Tool>) {
        self.tools.push(t);
    }

    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|t| t.def()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.def().name).collect()
    }

    /// 按名称调用工具；未知工具返回错误 JSON。
    pub async fn call(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolContext,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        for t in &self.tools {
            if t.def().name == name {
                return t.call(args, ctx, cancel).await;
            }
        }
        Ok(crate::domain::deny(&format!("未知工具: {name}")))
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// 为一次 Agent 任务生成批次号（贯穿扫描→计划→执行→轨迹）。
pub fn fresh_batch() -> BatchId {
    crate::trajectory::new_batch_id()
}
