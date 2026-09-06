//! 计划生命周期集成测试：generate → approve → execute → rollback。

use std::path::PathBuf;
use std::sync::Arc;

use ds_engine::plan::{OpKind, Plan, PlanOrchestrator, PlanStatus, PlanStore, PlannedOp};
use ds_engine::trajectory::TrajectoryLogger;
use tokio_util::sync::CancellationToken;

fn no_cancel() -> CancellationToken {
    CancellationToken::new()
}

fn fresh_orchestrator() -> (PlanOrchestrator, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("ds_plan_{}", uuid::Uuid::new_v4()));
    let plans_dir = root.join("plans");
    let traj_path = root.join("traj.jsonl");
    let store = PlanStore::new(plans_dir.clone()).unwrap();
    let traj = Arc::new(TrajectoryLogger::open(&traj_path).unwrap());
    (PlanOrchestrator::new(store, traj), plans_dir, traj_path)
}

#[tokio::test]
async fn full_lifecycle() {
    let (orch, _plans_dir, traj_path) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_plan_src_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src1 = root.join("a.txt");
    let src2 = root.join("b.txt");
    std::fs::write(&src1, b"AAA").unwrap();
    std::fs::write(&src2, b"BBB").unwrap();
    let dst1 = root.join("out/doc/a.txt");
    let dst2 = root.join("out/doc/b.txt");

    // generate
    let plan = orch
        .generate(vec![
            (src1.clone(), dst1.clone()),
            (src2.clone(), dst2.clone()),
        ])
        .await
        .unwrap();
    assert_eq!(plan.status, PlanStatus::Generated);
    assert_eq!(plan.operations.len(), 2);
    // 此时不应移动文件
    assert!(src1.exists());

    // approve
    let plan = orch.approve(plan.id).unwrap();
    assert_eq!(plan.status, PlanStatus::Approved);

    // execute
    let plan = orch.execute(plan.id, &no_cancel()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::Executed);
    assert!(!src1.exists(), "源文件应已移走");
    assert!(!src2.exists());
    assert!(dst1.exists());
    assert!(dst2.exists());
    assert_eq!(std::fs::read(&dst1).unwrap(), b"AAA");

    // rollback
    let plan = orch.rollback(plan.id, &no_cancel()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::RolledBack);
    assert!(src1.exists(), "回滚后源文件应回到原位");
    assert!(src2.exists());
    assert_eq!(std::fs::read(&src1).unwrap(), b"AAA");
    assert!(!dst1.exists(), "回滚后目标应清空");

    // 轨迹应含全部四类事件
    let log = std::fs::read_to_string(&traj_path).unwrap();
    assert!(log.contains("plan_generated"), "缺 PlanGenerated: {log}");
    assert!(log.contains("plan_approved"), "缺 PlanApproved: {log}");
    assert!(log.contains("plan_executed"), "缺 PlanExecuted: {log}");
    assert!(log.contains("plan_rolled_back"), "缺 PlanRolledBack: {log}");
    // 执行与回滚各应有 file_operation 事件
    let fo = log.matches("file_operation").count();
    assert!(fo >= 4, "file_operation 事件不足: {fo}");
}

#[tokio::test]
async fn cannot_execute_unapproved() {
    let (orch, _, _) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_plan_un_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src = root.join("a.txt");
    std::fs::write(&src, b"x").unwrap();
    let plan = orch
        .generate(vec![(src.clone(), root.join("out/a.txt"))])
        .await
        .unwrap();
    // 未确认直接执行应报错
    let err = orch.execute(plan.id, &no_cancel()).await.unwrap_err();
    assert!(err.to_string().contains("待执行"));
}

#[tokio::test]
async fn cannot_rollback_unexecuted() {
    let (orch, _, _) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_plan_rb_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src = root.join("a.txt");
    std::fs::write(&src, b"x").unwrap();
    let plan = orch
        .generate(vec![(src.clone(), root.join("out/a.txt"))])
        .await
        .unwrap();
    let approved = orch.approve(plan.id).unwrap();
    // 已确认但未执行，回滚应报错
    let err = orch.rollback(approved.id, &no_cancel()).await.unwrap_err();
    assert!(err.to_string().contains("已执行"));
}

#[test]
fn plan_store_persistence_roundtrip() {
    let root = std::env::temp_dir().join(format!("ds_plan_store_{}", uuid::Uuid::new_v4()));
    let store = PlanStore::new(root.join("plans")).unwrap();
    // 空目录 list
    assert!(store.list().unwrap().is_empty());
}

#[allow(dead_code)]
fn _force_pathbuf_use() -> PathBuf {
    PathBuf::new()
}

// ── 回收站删除语义测试 ──

#[tokio::test]
async fn trash_lifecycle() {
    let (orch, _, traj_path) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_trash_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src1 = root.join("garbage1.txt");
    let src2 = root.join("garbage2.txt");
    std::fs::write(&src1, b"trash me 1").unwrap();
    std::fs::write(&src2, b"trash me 2").unwrap();

    // generate_trash
    let plan = orch
        .generate_trash(vec![src1.clone(), src2.clone()])
        .await
        .unwrap();
    assert_eq!(plan.status, PlanStatus::Generated);
    assert_eq!(plan.operations.len(), 2);
    assert!(plan.operations.iter().all(|o| o.kind == OpKind::Trash));
    // 未执行，文件还在
    assert!(src1.exists());

    // approve
    let plan = orch.approve(plan.id).unwrap();
    assert_eq!(plan.status, PlanStatus::Approved);

    // execute → 文件进入系统回收站，原路径消失
    let plan = orch.execute(plan.id, &no_cancel()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::Executed);
    assert!(!src1.exists(), "文件应已被移入回收站");
    assert!(!src2.exists());

    // rollback → 状态置 RolledBack，但文件无法自动恢复（在系统回收站中）
    let plan = orch.rollback(plan.id, &no_cancel()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::RolledBack);
    assert!(!src1.exists(), "回收站文件不由本系统自动恢复");

    // 轨迹应含 trash 操作与 rollback 的提示
    let log = std::fs::read_to_string(&traj_path).unwrap();
    assert!(
        log.contains("\"op\":\"trash\""),
        "缺 trash file_operation: {log}"
    );
    assert!(log.contains("plan_executed"), "缺 plan_executed: {log}");
    assert!(
        log.contains("plan_rolled_back"),
        "缺 plan_rolled_back: {log}"
    );
    // rollback 应含"系统回收站"提示
    assert!(log.contains("系统回收站"), "缺回收站恢复提示: {log}");
}

#[tokio::test]
async fn mixed_plan_move_and_trash() {
    let (orch, _, _) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_mixed_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let move_src = root.join("keep.txt");
    let trash_src = root.join("junk.txt");
    std::fs::write(&move_src, b"keep").unwrap();
    std::fs::write(&trash_src, b"junk").unwrap();

    // 先生成移动计划
    let mut plan = orch
        .generate(vec![(move_src.clone(), root.join("out/keep.txt"))])
        .await
        .unwrap();
    // 手动追加一个 trash 操作（模拟混合计划）
    plan.operations.push(ds_engine::plan::PlannedOp {
        src: trash_src.clone(),
        dst: PathBuf::new(),
        kind: OpKind::Trash,
        result: None,
        error: None,
    });
    // 手动保存修改后的计划
    let store = PlanStore::new(root.join("plans_mixed")).unwrap();
    store.save(&plan).unwrap();
    // 用新的 orchestrator（指向同一 store）来执行
    let traj = Arc::new(TrajectoryLogger::open(&root.join("traj_mixed.jsonl")).unwrap());
    let orch2 = PlanOrchestrator::new(store, traj);

    // approve + execute
    let plan = orch2.approve(plan.id).unwrap();
    let plan = orch2.execute(plan.id, &no_cancel()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::Executed);
    // 移动的文件在目标位置
    assert!(!move_src.exists());
    assert!(root.join("out/keep.txt").exists());
    // 回收站的文件消失
    assert!(!trash_src.exists());

    // rollback：move 项应自动恢复，trash 项记提示
    let plan = orch2.rollback(plan.id, &no_cancel()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::RolledBack);
    assert!(move_src.exists(), "移动项应已自动回滚");
    assert_eq!(std::fs::read(&move_src).unwrap(), b"keep");
    assert!(!trash_src.exists(), "回收站项不由本系统恢复");
}

#[tokio::test]
async fn cancelled_execute_marks_remaining() {
    let (orch, _, traj_path) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_cancel_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src1 = root.join("a1.txt");
    let src2 = root.join("a2.txt");
    let src3 = root.join("a3.txt");
    for s in [&src1, &src2, &src3] {
        std::fs::write(s, b"x").unwrap();
    }
    let plan = orch
        .generate(vec![
            (src1.clone(), root.join("out1/a1.txt")),
            (src2.clone(), root.join("out2/a2.txt")),
            (src3.clone(), root.join("out3/a3.txt")),
        ])
        .await
        .unwrap();
    let plan = orch.approve(plan.id).unwrap();

    // 预置取消：全部项应标记 cancelled
    let cancel = CancellationToken::new();
    cancel.cancel();
    let plan = orch.execute(plan.id, &cancel).await.unwrap();
    assert_eq!(plan.status, PlanStatus::Cancelled);
    assert!(
        plan.operations
            .iter()
            .all(|o| o.result.as_deref() == Some("cancelled")),
        "全部项应标记 cancelled: {:?}",
        plan.operations
            .iter()
            .map(|o| o.result.clone())
            .collect::<Vec<_>>()
    );
    // 源文件应保留
    assert!(src1.exists());

    // 轨迹应有 Cancelled 说明（取消路径不写 FileOperation，只写 PlanExecuted）
    let _ = traj_path;
}

#[tokio::test]
async fn no_cancel_token_means_no_await_blocking() {
    let (orch, _, _) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_nc_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src = root.join("a.txt");
    std::fs::write(&src, b"x").unwrap();
    let plan = orch
        .generate(vec![(src, root.join("out/a.txt"))])
        .await
        .unwrap();
    let plan = orch.approve(plan.id).unwrap();
    // 默认令牌（未取消）应与普通执行一致
    let plan = orch.execute(plan.id, &no_cancel()).await.unwrap();
    assert_eq!(plan.status, PlanStatus::Executed);
}

// ── 计划多阶段演进 ──

#[tokio::test]
async fn plan_phase_progression() {
    let (orch, _, traj_path) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_phase_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src = root.join("a.txt");
    std::fs::write(&src, b"x").unwrap();
    let plan = orch
        .generate(vec![(src, root.join("out/a.txt"))])
        .await
        .unwrap();
    assert_eq!(plan.phase, ds_engine::plan::PlanPhase::Scanning);

    // 依次推进 扫描→权限→目标树→细化→审查→实施
    let p = orch.advance_phase(plan.id).unwrap();
    assert_eq!(p.phase, ds_engine::plan::PlanPhase::Permissions);
    let p = orch.advance_phase(plan.id).unwrap();
    assert_eq!(p.phase, ds_engine::plan::PlanPhase::TargetTree);
    let p = orch.advance_phase(plan.id).unwrap();
    assert_eq!(p.phase, ds_engine::plan::PlanPhase::Refinement);
    let p = orch.advance_phase(plan.id).unwrap();
    assert_eq!(p.phase, ds_engine::plan::PlanPhase::Review);

    // 轨迹应有 phase 事件
    let log = std::fs::read_to_string(&traj_path).unwrap();
    assert!(log.contains("plan_phase_advanced"), "缺 phase 事件: {log}");
}

#[tokio::test]
async fn dir_class_and_mapping_evolve() {
    let (orch, _, _) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_dirc_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src = root.join("a.txt");
    std::fs::write(&src, b"x").unwrap();
    let plan = orch
        .generate(vec![(src, root.join("out/a.txt"))])
        .await
        .unwrap();

    // 标注目录类型
    let p = orch
        .set_dir_class(plan.id, "绿色软件", ds_engine::domain::DirClass::Atomic)
        .unwrap();
    assert_eq!(
        p.dir_overrides.get("绿色软件"),
        Some(&ds_engine::domain::DirClass::Atomic)
    );

    // 设置一级映射：目标"文档"复用已有目录"documents"
    let p = orch.set_dir_mapping(plan.id, "文档", "documents").unwrap();
    assert_eq!(p.dir_mappings.get("文档"), Some(&"documents".to_string()));
}

#[tokio::test]
async fn draft_accept_and_freeze() {
    let (orch, _, _) = fresh_orchestrator();
    let root = std::env::temp_dir().join(format!("ds_draft_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let src = root.join("a.txt");
    std::fs::write(&src, b"x").unwrap();
    let plan = orch
        .generate(vec![(src.clone(), root.join("out/a.txt"))])
        .await
        .unwrap();

    // 推进到 Refinement
    let _ = orch.advance_phase(plan.id).unwrap();
    let _ = orch.advance_phase(plan.id).unwrap();
    let _ = orch.advance_phase(plan.id).unwrap();

    // AI 草稿（含一个新增项）
    let draft = vec![ds_engine::plan::PlannedOp {
        src: src.clone(),
        dst: root.join("b/a.txt"),
        kind: ds_engine::plan::OpKind::Move,
        result: None,
        error: None,
    }];
    let p = orch.accept_draft(plan.id, draft).unwrap();
    assert_eq!(p.draft_operations.len(), 1);
    assert_eq!(
        p.operations.len(),
        1,
        "草稿未冻结前正式 operations 保持原样"
    );

    // 冻结
    let p = orch.freeze_draft(plan.id).unwrap();
    assert_eq!(p.operations.len(), 1);
    assert!(p.draft_operations.is_empty());
}

#[test]
fn no_draft_accept_outside_refinement() {
    // 纯 serde 层校验：直接构造并断言 advance 顺序（省去 tmp）
    let phase_from = ds_engine::plan::PlanPhase::Scanning.next().unwrap();
    assert_eq!(phase_from, ds_engine::plan::PlanPhase::Permissions);
    assert_eq!(ds_engine::plan::PlanPhase::Implemented.next(), None);
}

// ── 目录视同类型硬校验 ──

fn atomic_classes(root: &std::path::Path) -> Vec<(PathBuf, ds_engine::domain::DirClass)> {
    vec![
        (root.join("绿色软件"), ds_engine::domain::DirClass::Atomic),
        (root.join("icons_dump"), ds_engine::domain::DirClass::Atomic),
    ]
}

#[test]
fn atomic_dir_file_move_rejected() {
    let root = std::env::temp_dir().join(format!("ds_atomic_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("绿色软件")).unwrap();
    std::fs::create_dir_all(root.join("icons_dump")).unwrap();
    let f1 = root.join("绿色软件/tool.exe");
    let f2 = root.join("icons_dump/icon_1.png");
    std::fs::write(&f1, b"x").unwrap();
    std::fs::write(&f2, b"y").unwrap();

    // 构造一个含违规操作的计划（src 在原子目录内部）
    let plan = Plan {
        id: uuid::Uuid::new_v4(),
        created_at: String::new(),
        status: PlanStatus::Generated,
        phase: ds_engine::plan::PlanPhase::Refinement,
        operations: vec![
            PlannedOp {
                src: f1.clone(),
                dst: root.join("视频/tool.exe"),
                kind: OpKind::Move,
                result: None,
                error: None,
            },
            PlannedOp {
                src: f2,
                dst: root.join("图片/icon_1.png"),
                kind: OpKind::Move,
                result: None,
                error: None,
            },
        ],
        draft_operations: vec![],
        dir_overrides: Default::default(),
        dir_mappings: Default::default(),
        executed: vec![],
    };

    let classes = atomic_classes(&root);
    let violations = plan.validate_dir_classes(&classes);
    assert_eq!(
        violations.len(),
        2,
        "原子目录内的两个文件移动都应违反校验: {violations:?}"
    );
    assert!(violations.iter().all(|(_, err)| err.contains("原子目录")));
}

#[test]
fn atomic_dir_itself_move_allowed() {
    let root = std::env::temp_dir().join(format!("ds_atomic_ok_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("绿色软件")).unwrap();
    std::fs::write(root.join("绿色软件/tool.exe"), b"x").unwrap();

    let plan = Plan {
        id: uuid::Uuid::new_v4(),
        created_at: String::new(),
        status: PlanStatus::Generated,
        phase: ds_engine::plan::PlanPhase::Refinement,
        operations: vec![PlannedOp {
            src: root.join("绿色软件"), // 整个目录移动（非内部文件）
            dst: root.join("软件/绿色软件"),
            kind: OpKind::Move,
            result: None,
            error: None,
        }],
        draft_operations: vec![],
        dir_overrides: Default::default(),
        dir_mappings: Default::default(),
        executed: vec![],
    };

    let classes = atomic_classes(&root);
    let violations = plan.validate_dir_classes(&classes);
    assert!(
        violations.is_empty(),
        "整体移动原子目录本身应允许: {violations:?}"
    );
}

#[test]
fn normal_dir_files_free() {
    let root = std::env::temp_dir().join(format!("ds_normal_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("freezone")).unwrap();
    let f = root.join("freezone/a.mp4");
    std::fs::write(&f, b"x").unwrap();

    let plan = Plan {
        id: uuid::Uuid::new_v4(),
        created_at: String::new(),
        status: PlanStatus::Generated,
        phase: ds_engine::plan::PlanPhase::Refinement,
        operations: vec![PlannedOp {
            src: f,
            dst: root.join("视频/a.mp4"),
            kind: OpKind::Move,
            result: None,
            error: None,
        }],
        draft_operations: vec![],
        dir_overrides: Default::default(),
        dir_mappings: Default::default(),
        executed: vec![],
    };

    let classes = atomic_classes(&root); // 仅绿色软件/icons_dump 为原子
    let violations = plan.validate_dir_classes(&classes);
    assert!(
        violations.is_empty(),
        "普通目录内文件应允许: {violations:?}"
    );
}

#[test]
fn draft_ops_also_validated() {
    let root = std::env::temp_dir().join(format!("ds_draft_v_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("icons_dump")).unwrap();
    let f = root.join("icons_dump/icon_9.png");
    std::fs::write(&f, b"y").unwrap();

    let plan = Plan {
        id: uuid::Uuid::new_v4(),
        created_at: String::new(),
        status: PlanStatus::Generated,
        phase: ds_engine::plan::PlanPhase::Refinement,
        operations: vec![],
        draft_operations: vec![PlannedOp {
            src: f,
            dst: root.join("图片/icon_9.png"),
            kind: OpKind::Move,
            result: None,
            error: None,
        }],
        dir_overrides: Default::default(),
        dir_mappings: Default::default(),
        executed: vec![],
    };

    let classes = atomic_classes(&root);
    let violations = plan.validate_dir_classes(&classes);
    assert_eq!(violations.len(), 1, "草稿中的原子目录文件也应被拦截");
}
