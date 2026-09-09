//! 目标目录规则类型；节点、模板和校验统一由 workflow 实现。

use serde::{Deserialize, Serialize};

/// 目录节点的规则类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleType {
    /// 简单规则：仅按扩展名命中即归入（一级目录强制）。
    #[default]
    Simple,
    /// 复杂规则：文件名/文件内容/备注/few-shot 相关。
    Complex,
}
