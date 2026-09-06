//! 计划-预览-确认-执行-回滚 闭环。
//!
//! 流程（每一步都写轨迹事件、每一步都可持久化恢复）：
//! 1) `generate`  从一组拟移动项生成 Plan(status=Generated)，落盘 + 写 PlanGenerated
//! 2) `approve`   用户确认 → status=Approved，写 PlanApproved
//! 3) `execute`   逐条执行移动，每条写 FileOperation；完成写 PlanExecuted
//! 4) `rollback`  按 executed 的逆序把 dst→src 移回，写 PlanRolledBack
//!
//! Plan 以 JSON 文件持久化到 `plan_store_dir`，一个批次号一个文件，
//! 可在任意阶段重启进程后继续（断点恢复）。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::domain::BatchId;
use crate::tools::organize::{perform_move, resolve_collision};
use crate::trajectory::{EventKind, TrajectoryLogger};
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// 已生成，待用户确认。
    Generated,
    /// 用户已确认，待执行。
    Approved,
    /// 已全部执行成功。
    Executed,
    /// 部分执行（有失败项）。
    PartiallyExecuted,
    /// 已回滚。
    RolledBack,
    /// 已取消（未执行或中途放弃）。
    Cancelled,
}

impl PlanStatus {
    fn label(&self) -> &'static str {
        match self {
            PlanStatus::Generated => "待确认",
            PlanStatus::Approved => "待执行",
            PlanStatus::Executed => "已执行",
            PlanStatus::PartiallyExecuted => "部分执行",
            PlanStatus::RolledBack => "已回滚",
            PlanStatus::Cancelled => "已取消",
        }
    }

    /// snake_case 字符串（与 serde 序列化一致），供 UI 比较与 CSS 类名使用。
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanStatus::Generated => "generated",
            PlanStatus::Approved => "approved",
            PlanStatus::Executed => "executed",
            PlanStatus::PartiallyExecuted => "partially_executed",
            PlanStatus::RolledBack => "rolled_back",
            PlanStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    #[default]
    Move,
    /// 将文件移入系统回收站/废纸篓（可恢复）。
    Trash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedOp {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub kind: OpKind,
    /// 执行后填入：ok / failed / skipped / missing / restored。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutedOp {
    /// 原始源路径（回滚目标）。
    pub src: PathBuf,
    /// 执行后所在路径（回滚起点）；Trash 操作此字段为空。
    pub dst: PathBuf,
    pub mode: String,
    /// 操作类型，用于回滚时区分 move（可自动移回）与 trash（需手动从回收站恢复）。
    #[serde(default)]
    pub kind: OpKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: BatchId,
    pub created_at: String,
    pub status: PlanStatus,
    /// 多阶段演进的当前阶段（扫描/权限/目标树/细化/审查/实施）。
    #[serde(default)]
    pub phase: PlanPhase,
    pub operations: Vec<PlannedOp>,
    /// AI 细化中的草稿操作（未冻结，不参与执行）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draft_operations: Vec<PlannedOp>,
    /// 目录类型标注（用户覆盖后的最终值），key = 目录 rel_path。
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub dir_overrides: std::collections::HashMap<String, crate::domain::DirClass>,
    /// 一级目标目录 映射到已有实际目录（复用已有目录，减少移动）。key=目标目录名。
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub dir_mappings: std::collections::HashMap<String, String>,
    /// 执行成功并落地的移动记录（逆序用于回滚）。
    #[serde(default)]
    pub executed: Vec<ExecutedOp>,
}

/// 计划演进阶段（对应"整理流程设计"的 零~四 阶段）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanPhase {
    /// 零阶段：扫描（计划文档确立，含扫描摘要）。
    #[default]
    Scanning,
    /// 一阶段：权限设置（目录类型标注在此可用）。
    Permissions,
    /// 二阶段：目标树确定（含 AI 建议与 dir_mappings）。
    TargetTree,
    /// 三阶段：AI 细化具体计划（draft 演进、冻结前）。
    Refinement,
    /// 四阶段：审查对比（冻结终稿，等待实施）。
    Review,
    /// 五阶段：已实施（status=Executed 后）。
    Implemented,
}

impl PlanPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanPhase::Scanning => "scanning",
            PlanPhase::Permissions => "permissions",
            PlanPhase::TargetTree => "target_tree",
            PlanPhase::Refinement => "refinement",
            PlanPhase::Review => "review",
            PlanPhase::Implemented => "implemented",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            PlanPhase::Scanning => "0 扫描",
            PlanPhase::Permissions => "1 权限与目录类型",
            PlanPhase::TargetTree => "2 目标树",
            PlanPhase::Refinement => "3 AI 细化",
            PlanPhase::Review => "4 审查对比",
            PlanPhase::Implemented => "5 已实施",
        }
    }

    /// 下一阶段（用于前进校验）。
    pub fn next(&self) -> Option<PlanPhase> {
        match self {
            PlanPhase::Scanning => Some(PlanPhase::Permissions),
            PlanPhase::Permissions => Some(PlanPhase::TargetTree),
            PlanPhase::TargetTree => Some(PlanPhase::Refinement),
            PlanPhase::Refinement => Some(PlanPhase::Review),
            PlanPhase::Review => Some(PlanPhase::Implemented),
            PlanPhase::Implemented => None,
        }
    }
}

impl Plan {
    /// 前进到下一阶段；已到最后阶段则返回 Err。
    pub fn advance_phase(&mut self) -> Result<PlanPhase> {
        match self.phase.next() {
            Some(next) => {
                self.phase = next;
                Ok(next)
            }
            None => anyhow::bail!("已是最后阶段 {}", self.phase.label()),
        }
    }

    /// 设置（覆盖）一个目录的类型标注。
    pub fn set_dir_class(&mut self, rel_path: &str, c: crate::domain::DirClass) {
        self.dir_overrides.insert(rel_path.to_string(), c);
    }

    /// 设置一级目标目录映射到已有实际目录。existing_rel_path 为空表示清除映射。
    pub fn set_dir_mapping(&mut self, target_dir: &str, existing_rel_path: &str) {
        if existing_rel_path.is_empty() {
            self.dir_mappings.remove(target_dir);
        } else {
            self.dir_mappings
                .insert(target_dir.to_string(), existing_rel_path.to_string());
        }
    }

    /// 用 AI 细化草稿替换当前 draft（用户确认后 freeze）。
    pub fn replace_draft(&mut self, ops: Vec<PlannedOp>) {
        self.draft_operations = ops;
    }

    /// 冻结草稿为正式 operations（终稿）。返回新增条目数。
    pub fn freeze_draft(&mut self) -> usize {
        let n = self.draft_operations.len();
        self.operations = std::mem::take(&mut self.draft_operations);
        n
    }

    /// 目录视同类型硬校验：原子目录内的文件不得单独进入计划（Move 源）。
    /// `dir_classes`: 绝对目录路径 → 视同类型（扫描结果 + 用户覆盖合并后的有效值）。
    /// 返回违反校验的操作（src 路径 + 所属原子目录）列表；为空表示通过。
    pub fn validate_dir_classes(
        &self,
        dir_classes: &[(std::path::PathBuf, crate::domain::DirClass)],
    ) -> Vec<(String, String)> {
        let mut violations = Vec::new();
        let ops = self.operations.iter().chain(self.draft_operations.iter());
        for op in ops {
            if let Some((dir, err)) = check_op_dir_class(&op.src, dir_classes) {
                violations.push((dir, err));
            }
        }
        violations
    }

    /// 对外部待接受的草稿操作做校验（accept_draft 前置拦截用）。
    pub fn validate_ops_against(
        ops: &[PlannedOp],
        dir_classes: &[(std::path::PathBuf, crate::domain::DirClass)],
    ) -> Vec<(String, String)> {
        let mut violations = Vec::new();
        for op in ops {
            if let Some((dir, err)) = check_op_dir_class(&op.src, dir_classes) {
                violations.push((dir, err));
            }
        }
        violations
    }

    pub fn summary(&self) -> String {
        let total = self.operations.len();
        let ok = self
            .operations
            .iter()
            .filter(|o| o.result.as_deref() == Some("ok"))
            .count();
        let failed = self
            .operations
            .iter()
            .filter(|o| o.result.as_deref() == Some("failed"))
            .count();
        let trashed = self
            .operations
            .iter()
            .filter(|o| o.kind == OpKind::Trash)
            .count();
        let moved = total - trashed;
        format!(
            "计划 {}  [{}]  共 {total} 项 ({moved} 移动 / {trashed} 回收站)  ok={ok} failed={failed} executed={}",
            self.id,
            self.status.label(),
            self.executed.len(),
        )
    }
}

/// 计划持久化：每份计划一个 JSON 文件。
pub struct PlanStore {
    dir: PathBuf,
}

impl PlanStore {
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self, id: BatchId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn save(&self, plan: &Plan) -> Result<()> {
        let s = serde_json::to_string_pretty(plan)?;
        std::fs::write(self.path(plan.id), s)?;
        Ok(())
    }

    pub fn load(&self, id: BatchId) -> Result<Plan> {
        let s = std::fs::read_to_string(self.path(id))?;
        Ok(serde_json::from_str(&s)?)
    }

    pub fn list(&self) -> Result<Vec<Plan>> {
        let mut plans = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    if let Ok(plan) = serde_json::from_str::<Plan>(&s) {
                        plans.push(plan);
                    }
                }
            }
        }
        plans.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(plans)
    }
}

/// 编排器：串联 生成→确认→执行→回滚 与轨迹日志。
pub struct PlanOrchestrator {
    store: PlanStore,
    trajectory: Arc<TrajectoryLogger>,
}

impl PlanOrchestrator {
    pub fn new(store: PlanStore, trajectory: Arc<TrajectoryLogger>) -> Self {
        Self { store, trajectory }
    }

    /// 从 (src,dst) 列表生成一份移动计划（自动冲突避让 dst）。
    pub async fn generate(&self, moves: Vec<(PathBuf, PathBuf)>) -> Result<Plan> {
        let operations = moves
            .into_iter()
            .map(|(src, dst)| PlannedOp {
                src,
                dst: resolve_collision(&dst),
                kind: OpKind::Move,
                result: None,
                error: None,
            })
            .collect::<Vec<_>>();
        self.persist_and_log(operations).await
    }

    /// 从路径列表生成一份回收站计划（dst 为空，kind=Trash）。
    pub async fn generate_trash(&self, paths: Vec<PathBuf>) -> Result<Plan> {
        let operations = paths
            .into_iter()
            .map(|src| PlannedOp {
                src,
                dst: PathBuf::new(),
                kind: OpKind::Trash,
                result: None,
                error: None,
            })
            .collect::<Vec<_>>();
        self.persist_and_log(operations).await
    }

    /// 内部：构造 Plan 并持久化 + 写 PlanGenerated 轨迹。
    async fn persist_and_log(&self, operations: Vec<PlannedOp>) -> Result<Plan> {
        let plan = Plan {
            id: Uuid::new_v4(),
            created_at: Utc::now().to_rfc3339(),
            status: PlanStatus::Generated,
            phase: PlanPhase::Scanning,
            operations,
            draft_operations: vec![],
            dir_overrides: Default::default(),
            dir_mappings: Default::default(),
            executed: vec![],
        };
        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::PlanGenerated,
            Some(plan.id),
            json!({ "count": plan.operations.len(), "preview": preview(&plan) }),
        )?;
        Ok(plan)
    }

    /// 持久化演进后的计划（写盘 + 不重复记 PlanGenerated）。
    pub fn save_evolved(&self, plan: &Plan) -> Result<()> {
        self.store.save(plan)
    }

    /// 前进计划阶段（写盘 + 轨迹）。
    pub fn advance_phase(&self, id: BatchId) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        let prev = plan.phase;
        let next = plan.advance_phase()?;
        // 在 review 之后才允许 approve（终稿冻结后确认执行）
        if matches!(
            plan.status,
            PlanStatus::Executed | PlanStatus::RolledBack | PlanStatus::Cancelled
        ) {
            anyhow::bail!("计划已终止（{}），不可再演进", plan.status.label());
        }
        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::PlanPhaseAdvanced,
            Some(plan.id),
            json!({ "from": prev.as_str(), "to": next.as_str() }),
        )?;
        Ok(plan)
    }

    /// 更新目录类型标注（写盘 + 轨迹）。
    pub fn set_dir_class(
        &self,
        id: BatchId,
        rel_path: &str,
        c: crate::domain::DirClass,
    ) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        plan.set_dir_class(rel_path, c);
        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::DirClassSet,
            Some(plan.id),
            json!({ "dir": rel_path, "class": c.as_str() }),
        )?;
        Ok(plan)
    }

    /// 设置一级目录映射（目标目录 → 已有实际目录）。
    pub fn set_dir_mapping(&self, id: BatchId, target: &str, existing: &str) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        plan.set_dir_mapping(target, existing);
        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::DirMapped,
            Some(plan.id),
            json!({ "target": target, "existing": existing }),
        )?;
        Ok(plan)
    }

    /// 接受 AI 细化草稿（写盘 + 轨迹）。
    pub fn accept_draft(&self, id: BatchId, ops: Vec<PlannedOp>) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        if plan.phase != PlanPhase::Refinement {
            anyhow::bail!("仅“AI 细化”阶段可接受草稿，当前 {}", plan.phase.label());
        }
        plan.replace_draft(ops);
        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::DraftAccepted,
            Some(plan.id),
            json!({ "count": plan.draft_operations.len() }),
        )?;
        Ok(plan)
    }

    /// 冻结草稿为终稿（写盘 + 轨迹）。
    pub fn freeze_draft(&self, id: BatchId) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        if plan.phase != PlanPhase::Refinement {
            anyhow::bail!("仅“AI 细化”阶段可冻结草稿，当前 {}", plan.phase.label());
        }
        let n = plan.freeze_draft();
        self.store.save(&plan)?;
        self.trajectory
            .log(EventKind::DraftFrozen, Some(plan.id), json!({ "ops": n }))?;
        Ok(plan)
    }

    /// 用户确认计划。仅 Generated 状态可确认。
    pub fn approve(&self, id: BatchId) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        if plan.status != PlanStatus::Generated {
            anyhow::bail!(
                "计划当前状态为 {}，仅“待确认”状态可确认",
                plan.status.label()
            );
        }
        plan.status = PlanStatus::Approved;
        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::PlanApproved,
            Some(plan.id),
            json!({ "count": plan.operations.len() }),
        )?;
        Ok(plan)
    }

    /// 执行已确认计划：逐条执行（Move 走 perform_move，Trash 走系统回收站），
    /// 逐条记轨迹。全部成功→Executed，否则→PartiallyExecuted；
    /// 若中途取消：已完成项保留，未执行项标记 cancelled，状态置 PartiallyExecuted/Cancelled。
    pub async fn execute(&self, id: BatchId, cancel: &CancellationToken) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        if plan.status != PlanStatus::Approved {
            anyhow::bail!(
                "计划当前状态为 {}，仅“待执行”状态可执行",
                plan.status.label()
            );
        }
        let traj = self.trajectory.clone();
        let batch = plan.id;
        let cancel = cancel.clone();
        let plan = spawn_blocking(move || -> Result<Plan> {
            let mut executed = Vec::new();
            for op in plan.operations.iter_mut() {
                if cancel.is_cancelled() {
                    // 取消：标记剩余项，不再执行文件操作
                    op.result = Some("cancelled".into());
                    op.error = Some("用户取消".into());
                    continue;
                }
                match op.kind {
                    OpKind::Move => match perform_move(&op.src, &op.dst) {
                        Ok(mode) => {
                            let _ = traj.log(
                                EventKind::FileOperation,
                                Some(batch),
                                json!({
                                    "op": "move", "mode": mode,
                                    "src": op.src.to_string_lossy(),
                                    "dst": op.dst.to_string_lossy(),
                                }),
                            );
                            op.result = Some("ok".into());
                            op.error = None;
                            executed.push(ExecutedOp {
                                src: op.src.clone(),
                                dst: op.dst.clone(),
                                mode: mode.into(),
                                kind: OpKind::Move,
                            });
                        }
                        Err(e) => {
                            let _ = traj.log(
                                EventKind::Error,
                                Some(batch),
                                json!({
                                    "op": "move",
                                    "src": op.src.to_string_lossy(),
                                    "dst": op.dst.to_string_lossy(),
                                    "error": e.to_string(),
                                }),
                            );
                            op.result = Some("failed".into());
                            op.error = Some(e.to_string());
                        }
                    },
                    OpKind::Trash => match trash::delete(&op.src) {
                        Ok(()) => {
                            let _ = traj.log(
                                EventKind::FileOperation,
                                Some(batch),
                                json!({
                                    "op": "trash",
                                    "src": op.src.to_string_lossy(),
                                }),
                            );
                            op.result = Some("ok".into());
                            op.error = None;
                            executed.push(ExecutedOp {
                                src: op.src.clone(),
                                dst: PathBuf::new(),
                                mode: "trash".into(),
                                kind: OpKind::Trash,
                            });
                        }
                        Err(e) => {
                            let _ = traj.log(
                                EventKind::Error,
                                Some(batch),
                                json!({
                                    "op": "trash",
                                    "src": op.src.to_string_lossy(),
                                    "error": e.to_string(),
                                }),
                            );
                            op.result = Some("failed".into());
                            op.error = Some(e.to_string());
                        }
                    },
                }
            }
            let failed = plan
                .operations
                .iter()
                .filter(|o| o.result.as_deref() == Some("failed"))
                .count();
            let cancelled = plan
                .operations
                .iter()
                .filter(|o| o.result.as_deref() == Some("cancelled"))
                .count();
            plan.status = if cancelled > 0 {
                if plan
                    .operations
                    .iter()
                    .any(|o| o.result.as_deref() == Some("ok"))
                {
                    PlanStatus::PartiallyExecuted
                } else {
                    PlanStatus::Cancelled
                }
            } else if failed == 0 {
                PlanStatus::Executed
            } else {
                PlanStatus::PartiallyExecuted
            };
            plan.executed = executed;
            Ok(plan)
        })
        .await??;

        let ok = plan
            .operations
            .iter()
            .filter(|o| o.result.as_deref() == Some("ok"))
            .count();
        let failed = plan
            .operations
            .iter()
            .filter(|o| o.result.as_deref() == Some("failed"))
            .count();
        let cancelled = plan
            .operations
            .iter()
            .filter(|o| o.result.as_deref() == Some("cancelled"))
            .count();
        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::PlanExecuted,
            Some(plan.id),
            json!({
                "ok": ok,
                "failed": failed,
                "cancelled": cancelled,
                "status": plan.status.label(),
                "status_str": plan.status.as_str(),
            }),
        )?;
        Ok(plan)
    }

    /// 回滚已执行计划：
    /// - Move 操作：按 executed 逆序把 dst→src 移回。
    /// - Trash 操作：文件在系统回收站中，无法自动移回，仅记提示（用户可通过 Finder/资源管理器手动恢复）。
    /// 中途取消：停止移动，已完成的项保留。
    pub async fn rollback(&self, id: BatchId, cancel: &CancellationToken) -> Result<Plan> {
        let mut plan = self.store.load(id)?;
        match plan.status {
            PlanStatus::Executed | PlanStatus::PartiallyExecuted => {}
            _ => anyhow::bail!("计划当前状态为 {}，仅已执行状态可回滚", plan.status.label()),
        }
        let traj = self.trajectory.clone();
        let batch = plan.id;
        let cancel = cancel.clone();
        let plan = spawn_blocking(move || -> Result<Plan> {
            let mut executed = std::mem::take(&mut plan.executed);
            executed.reverse();
            for eop in executed.into_iter() {
                if cancel.is_cancelled() {
                    let _ = traj.log(
                        EventKind::Cancelled,
                        Some(batch),
                        json!({ "op": "rollback", "note": "回滚中途取消，剩余项未处理" }),
                    );
                    break;
                }
                match eop.kind {
                    OpKind::Trash => {
                        let _ = traj.log(
                            EventKind::Error,
                            Some(batch),
                            json!({
                                "op": "rollback",
                                "kind": "trash",
                                "src": eop.src.to_string_lossy(),
                                "note": "文件在系统回收站中，需通过 Finder/资源管理器手动恢复",
                            }),
                        );
                        continue;
                    }
                    OpKind::Move => {}
                }
                if !eop.dst.exists() {
                    let _ = traj.log(
                        EventKind::Error,
                        Some(batch),
                        json!({
                            "op": "rollback",
                            "src": eop.dst.to_string_lossy(),
                            "dst": eop.src.to_string_lossy(),
                            "error": "目标文件已不在原位，跳过"
                        }),
                    );
                    continue;
                }
                let restore_dst = if eop.src.exists() {
                    let variant = resolve_collision(&eop.src);
                    let _ = traj.log(
                        EventKind::Error,
                        Some(batch),
                        json!({
                            "op": "rollback",
                            "warning": "源路径被占用，回滚到变体名",
                            "original_src": eop.src.to_string_lossy(),
                            "restore_dst": variant.to_string_lossy(),
                        }),
                    );
                    variant
                } else {
                    eop.src.clone()
                };
                match perform_move(&eop.dst, &restore_dst) {
                    Ok(mode) => {
                        let _ = traj.log(
                            EventKind::FileOperation,
                            Some(batch),
                            json!({
                                "op": "rollback", "mode": mode,
                                "src": eop.dst.to_string_lossy(),
                                "dst": restore_dst.to_string_lossy(),
                                "original_src": eop.src.to_string_lossy(),
                            }),
                        );
                    }
                    Err(e) => {
                        let _ = traj.log(
                            EventKind::Error,
                            Some(batch),
                            json!({
                                "op": "rollback",
                                "src": eop.dst.to_string_lossy(),
                                "dst": restore_dst.to_string_lossy(),
                                "error": e.to_string(),
                            }),
                        );
                    }
                }
            }
            plan.status = PlanStatus::RolledBack;
            Ok(plan)
        })
        .await??;

        self.store.save(&plan)?;
        self.trajectory.log(
            EventKind::PlanRolledBack,
            Some(plan.id),
            json!({ "status": plan.status.label() }),
        )?;
        Ok(plan)
    }
}

/// 生成计划的预览摘要（前 8 项），避免大计划把日志撑爆。
fn preview(plan: &Plan) -> Value {
    let sample: Vec<Value> = plan
        .operations
        .iter()
        .take(8)
        .map(|o| match o.kind {
            OpKind::Move => json!({
                "op": "move",
                "src": o.src.to_string_lossy(),
                "dst": o.dst.to_string_lossy(),
            }),
            OpKind::Trash => json!({
                "op": "trash",
                "src": o.src.to_string_lossy(),
            }),
        })
        .collect();
    json!({ "shown": sample.len(), "items": sample, "total": plan.operations.len() })
}

/// 给 CLI 用的“从 classify_by_rules 工具结果构造 moves”辅助函数。
pub fn moves_from_classify_json(result: &Value) -> Vec<(PathBuf, PathBuf)> {
    let arr = match result.get("moves").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return vec![],
    };
    arr.iter()
        .filter_map(|m| {
            let src = m.get("src")?.as_str()?.to_string();
            let dst = m.get("dst")?.as_str()?.to_string();
            Some((PathBuf::from(src), PathBuf::from(dst)))
        })
        .collect()
}

/// 检查单个操作的 src 是否违反目录视同类型约束：
/// - 位于 Atomic 目录内（且 src 不是该目录本身）→ 违规（不得拆散原子目录）
/// - 返回 (所属原子目录 rel_path, 错误描述)
fn check_op_dir_class(
    src: &std::path::Path,
    dir_classes: &[(PathBuf, crate::domain::DirClass)],
) -> Option<(String, String)> {
    use crate::domain::DirClass;
    for (dir_path, class) in dir_classes {
        if *class != DirClass::Atomic {
            continue;
        }
        // src 位于该原子目录内部（且非目录本身）
        if src.starts_with(dir_path) && src != dir_path {
            if let Some(parent) = src.parent() {
                if parent == dir_path || parent.starts_with(dir_path) {
                    // src 是原子目录的直接/间接后代文件
                    let rel = dir_path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir_path.to_string_lossy().into_owned());
                    return Some((
                        rel,
                        format!(
                            "原子目录“{}”内的文件 {} 不得单独移动（该目录已被标记为不可拆散）",
                            dir_path.to_string_lossy(),
                            src.to_string_lossy()
                        ),
                    ));
                }
            }
        }
    }
    None
}

/// 供 Tauri/CLI 使用的便捷包装：把 Plan 的校验结果转成字符串摘要。
pub fn summarize_violations(v: &[(String, String)]) -> String {
    v.iter()
        .map(|(dir, err)| format!("[{dir}] {err}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 过滤掉违反目录视同类型的移动项（原子目录内文件不得单独移动）。
/// 返回 (保留项, 拒绝原因列表)。
pub fn filter_atomic_ops(
    ops: &[PlannedOp],
    dir_classes: &[(PathBuf, crate::domain::DirClass)],
) -> (Vec<PlannedOp>, Vec<String>) {
    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    for op in ops {
        if let Some((_, reason)) = check_op_dir_class(&op.src, dir_classes) {
            rejected.push(reason);
        } else {
            kept.push(op.clone());
        }
    }
    (kept, rejected)
}
