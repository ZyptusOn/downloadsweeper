//! DownloadSweeper 核心引擎。
//!
//! GUI、CLI 与桌面外壳共用一个工作流：
//! - `workflow`：扫描、目录规则、计划、审查和任务状态。
//! - `workflow_ai` / `ai_runtime`：受限工具、模型编排、并发与预算。
//! - `permission` / `evidence`：授权检查与有界内容提取。
//! - `safe_fs` / `recycle`：持久化轨迹、安全执行与恢复。
//! - `jobs` / `response_cache` / `archive`：检查点、回答缓存与完整归档。
//! - `llm` / `pricing` / `cost`：模型协议、价格和实际用量。

pub mod ai_runtime;
pub mod archive;
pub mod chat_memory;
pub mod cleanup;
pub mod config;
pub mod cost;
pub mod domain;
pub mod evidence;
pub mod jobs;
pub mod llm;
pub mod metadata;
pub mod permission;
pub mod pricing;
pub mod recycle;
pub mod response_cache;
pub mod safe_fs;
pub mod tree;
pub mod workflow;
pub mod workflow_ai;

pub use anyhow::Result;
