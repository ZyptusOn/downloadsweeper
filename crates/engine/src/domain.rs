//! 工作流共用的目录类型。

use serde::{Deserialize, Serialize};

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
