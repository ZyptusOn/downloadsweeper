//! DownloadSweeper 核心引擎。
//!
//! 设计分层：
//! - `domain`     领域类型（文件条目、路径、批次号）
//! - `permission`  分级隐私授权配置（独立于 LLM）
//! - `config`     应用配置（LLM endpoint/key/价格、扫描根、权限）
//! - `trajectory` JSONL 追加式操作轨迹日志
//! - `scan`       下载目录扫描
//! - `cost`       token 用量统计与预算熔断
//! - `llm`        LLM 客户端 trait + OpenAI 兼容实现
//! - `tools`      文件读写工具（权限在此层强制）
//! - `agent`      工具调用编排循环与会话上下文

#[cfg(feature = "legacy-agent")]
pub mod agent;
pub mod ai_runtime;
pub mod archive;
pub mod chat_memory;
pub mod classify;
pub mod cleanup;
pub mod config;
pub mod cost;
pub mod domain;
pub mod evidence;
pub mod fingerprint;
pub mod jobs;
pub mod llm;
pub mod metadata;
pub mod permission;
pub mod plan;
pub mod pricing;
pub mod recycle;
pub mod rename;
pub mod response_cache;
pub mod safe_fs;
pub mod scan;
pub mod tools;
pub mod trajectory;
pub mod tree;
pub mod workflow;
pub mod workflow_ai;

pub use anyhow::Result;
