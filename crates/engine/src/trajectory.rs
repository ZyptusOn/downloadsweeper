//! JSONL 追加式操作轨迹日志。
//!
//! 每行一个事件（serde_json 单行序列化），崩溃时最多丢失最后一行。
//! 事件含 schema 版本、自增序号、时间戳、种类、批次号、详情。
//! LLM 请求/响应只记录摘要，不落盘文件内容或 API Key。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::domain::BatchId;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionStarted,
    SessionEnded,
    ScanStarted,
    ScanCompleted,
    PlanGenerated,
    PlanApproved,
    PlanExecuted,
    PlanRolledBack,
    /// 单条文件操作：移动/改名/删除进回收站。
    FileOperation,
    /// 计划阶段前进（0 扫描 → 1 权限 → ...）。
    PlanPhaseAdvanced,
    /// 目录类型标注设置（container/atomic/normal）。
    DirClassSet,
    /// 目标目录映射到已有实际目录。
    DirMapped,
    /// AI 细化草稿被接受。
    DraftAccepted,
    /// AI 细化草稿被冻结为终稿。
    DraftFrozen,
    /// LLM 请求摘要（模型名、消息数、工具数），不含内容全文。
    LlmRequest,
    /// LLM 响应摘要（token 用量、是否含 tool_calls）。
    LlmResponse,
    ToolCall,
    ToolResult,
    BudgetExceeded,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryEvent {
    pub schema_version: u32,
    pub seq: u64,
    pub timestamp: String,
    pub kind: EventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<BatchId>,
    pub detail: Value,
}

/// 线程安全的追加式日志器。
pub struct TrajectoryLogger {
    file: Mutex<File>,
    seq: AtomicU64,
}

impl TrajectoryLogger {
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;
        // 重启后续号：统计已有行数（seq 严格自增、每行恰一条事件）。
        let seq: u64 = {
            use std::io::{BufRead, BufReader};
            let mut reader = BufReader::new(&file);
            let mut line = String::new();
            let mut count: u64 = 0;
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                count += 1;
                line.clear();
            }
            count
        };
        // 复位到文件末尾（append 模式写操作始终附加到末尾，此处保险起见）
        use std::io::Seek;
        file.seek(std::io::SeekFrom::End(0))?;
        Ok(Self {
            file: Mutex::new(file),
            seq: AtomicU64::new(seq),
        })
    }

    /// 追加一条事件。`detail` 由调用方构造（注意勿放入敏感原文）。
    pub fn log(
        &self,
        kind: EventKind,
        batch_id: Option<BatchId>,
        detail: Value,
    ) -> anyhow::Result<u64> {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let event = TrajectoryEvent {
            schema_version: SCHEMA_VERSION,
            seq,
            timestamp: Utc::now().to_rfc3339(),
            kind,
            batch_id,
            detail,
        };
        let line = serde_json::to_string(&event)?;
        let mut f = self.file.lock().map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        writeln!(f, "{line}")?;
        // 操作类事件需要持久化先于文件系统动作，故立即 flush。
        f.flush()?;
        Ok(seq)
    }
}

/// 构造一个 LLM 请求摘要 detail（不含消息原文）。
pub fn llm_request_detail(model: &str, msg_count: usize, tool_count: usize) -> Value {
    serde_json::json!({
        "model": model,
        "messages": msg_count,
        "tools": tool_count,
    })
}

/// 构造一个 LLM 响应摘要 detail。
pub fn llm_response_detail(
    prompt_tokens: u64,
    completion_tokens: u64,
    tool_calls: &[String],
) -> Value {
    serde_json::json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "tool_calls": tool_calls,
    })
}

/// 生成一个新批次号。
pub fn new_batch_id() -> BatchId {
    Uuid::new_v4()
}
