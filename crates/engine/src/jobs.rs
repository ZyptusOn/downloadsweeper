//! Durable job control. Domain checkpoints and the file WAL remain authoritative.
//! No model credentials are stored in job parameters; configuration is only hashed.
use crate::{
    config::AppConfig,
    safe_fs::{atomic_json, TaskStore},
    workflow::Task,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ParallelBatch {
    pub id: String,
    pub branch: String,
    pub files: usize,
    pub status: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ParallelProgress {
    pub limit: usize,
    pub completed_files: usize,
    pub total_files: usize,
    pub batches: Vec<ParallelBatch>,
}
impl ParallelProgress {
    pub fn classification(run: &crate::workflow::Classification, limit: usize) -> Self {
        Self { limit, completed_files: run.completed, total_files: run.total,
            batches: run.batches.iter().map(|b| ParallelBatch { id:b.id.clone(), branch:b.branch.clone(), files:b.files, status:b.status.clone() }).collect() }
    }
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Job {
    pub id: Uuid,
    pub task_id: Uuid,
    pub kind: String,
    pub status: String,
    pub current: usize,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<ParallelProgress>,
    pub message: String,
    pub error: Option<String>,
    pub started_at: String,
    pub saved_at: String,
    pub resumable: bool,
    pub recovery_note: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: u32,
    pub job: Job,
    pub args: Value,
    pub revision: u64,
    pub config_digest: String,
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    payload: String,
    checksum: String,
}

pub fn config_digest(config: &AppConfig) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(config)?)
        .to_hex()
        .to_string())
}
pub fn is_ai(kind: &str) -> bool {
    matches!(
        kind,
        "plan_ai" | "review_proposal" | "rename" | "cleanup_ai" | "suggest_tree" | "chat" | "test_connection"
    )
}
impl Checkpoint {
    pub fn new(task: &Task, kind: &str, args: Value, config: &AppConfig) -> Result<Self> {
        Self::detached(task.id, task.revision, kind, args, config)
    }
    pub fn detached(
        task_id: Uuid,
        revision: u64,
        kind: &str,
        args: Value,
        config: &AppConfig,
    ) -> Result<Self> {
        let now = chrono::Utc::now().to_rfc3339();
        Ok(Self {
            version: 1,
            job: Job {
                id: Uuid::new_v4(),
                task_id,
                kind: kind.into(),
                status: "running".into(),
                current: 0,
                total: 0,
                parallel: None,
                message: "任务正在准备".into(),
                error: None,
                started_at: now.clone(),
                saved_at: now,
                resumable: false,
                recovery_note: String::new(),
            },
            args,
            revision,
            config_digest: config_digest(config)?,
        })
    }
    pub fn stopped(&mut self, task: &Task) {
        self.revision = task.revision;
        self.job.resumable = !matches!(
            self.job.kind.as_str(),
            "chat" | "test_connection" | "cleanup_trash"
        ) && (!is_ai(&self.job.kind) || task.pending_calls.is_empty());
        self.job.recovery_note = if is_ai(&self.job.kind) && !task.pending_calls.is_empty() {
            "存在未确认的 API 调用，已保留预算预留；不能自动重发，请先核对服务商用量。".into()
        } else if self.job.kind == "cleanup_trash" {
            "回收批次已记录；请在删除建议中撤销已处理和暂存文件，再重新选择，避免重复回收。".into()
        } else if !self.job.resumable {
            "单次模型回答无法从服务端断点续传；已保存上下文，需要时手动重新发送。".into()
        } else if matches!(
            self.job.kind.as_str(),
            "scan" | "desktop_plan" | "plan_rules" | "cleanup_options" | "archive_export"
        ) {
            "保留已提交的任务快照；继续时重新计算未提交的本地阶段，不调用模型。".into()
        } else {
            "继续时校验当前任务，复用已保存的批次结果或操作日志。".into()
        };
    }
    pub fn validate_resume(&self, task: &Task, config: &AppConfig) -> Result<()> {
        ensure!(
            matches!(
                self.job.status.as_str(),
                "paused" | "interrupted" | "failed"
            ),
            "该运行记录不能继续"
        );
        ensure!(self.job.resumable, "{}", self.job.recovery_note);
        ensure!(
            task.id == self.job.task_id && task.revision == self.revision,
            "暂停后任务已改变，旧检查点失效；请从当前页面重新启动该操作"
        );
        if is_ai(&self.job.kind) {
            ensure!(
                task.runtime_run == Some(self.job.id),
                "模型运行上下文已经改变，不能继续旧检查点"
            );
            ensure!(
                task.pending_calls.is_empty(),
                "存在未确认的 API 调用，不能自动重发"
            );
            ensure!(
                config_digest(config)? == self.config_digest,
                "模型配置已改变，请从当前页面重新启动，不能套用旧运行参数"
            );
        }
        Ok(())
    }
}
impl TaskStore {
    pub fn job_path(&self, id: Uuid) -> std::path::PathBuf {
        self.directory.join("jobs").join(format!("{id}.json"))
    }
    pub fn save_job(&self, checkpoint: &mut Checkpoint) -> Result<()> {
        checkpoint.job.saved_at = chrono::Utc::now().to_rfc3339();
        let payload = serde_json::to_string(checkpoint)?;
        atomic_json(
            &self.job_path(checkpoint.job.id),
            &Envelope {
                checksum: blake3::hash(payload.as_bytes()).to_hex().to_string(),
                payload,
            },
        )
    }
    pub fn load_job(&self, id: Uuid) -> Result<Checkpoint> {
        let wire: Envelope = serde_json::from_slice(&std::fs::read(self.job_path(id))?)?;
        ensure!(
            wire.checksum == blake3::hash(wire.payload.as_bytes()).to_hex().as_str(),
            "运行检查点完整性校验失败"
        );
        let checkpoint: Checkpoint = serde_json::from_str(&wire.payload)?;
        ensure!(
            checkpoint.version == 1 && checkpoint.job.id == id,
            "运行检查点版本或 ID 不匹配"
        );
        Ok(checkpoint)
    }
    pub fn list_jobs(&self) -> Result<Vec<Checkpoint>> {
        let directory = self.directory.join("jobs");
        if !directory.exists() {
            return Ok(vec![]);
        }
        let mut jobs = vec![];
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|s| s == "json") {
                let Ok(id) = Uuid::parse_str(
                    entry
                        .path()
                        .file_stem()
                        .context("检查点缺少 ID")?
                        .to_str()
                        .context("检查点 ID 无效")?,
                ) else {
                    continue;
                };
                match self.load_job(id) {
                    Ok(checkpoint) => jobs.push(checkpoint),
                    Err(error) => {
                        // Keep the corrupt file for diagnosis; one damaged record must
                        // not hide every other task or prevent the application opening.
                        let mut checkpoint = Checkpoint::detached(
                            Uuid::nil(),
                            0,
                            "damaged",
                            Value::Null,
                            &AppConfig::default(),
                        )?;
                        checkpoint.job.id = id;
                        checkpoint.job.status = "failed".into();
                        checkpoint.job.message = "运行检查点损坏，已禁止恢复；原记录保留".into();
                        checkpoint.job.error = Some(error.to_string());
                        jobs.push(checkpoint);
                    }
                }
            }
        }
        jobs.sort_by(|a, b| b.job.saved_at.cmp(&a.job.saved_at));
        Ok(jobs)
    }
    /// Run after reconciling file intents. Never starts work or sends an API call.
    pub fn recover_jobs(&self) -> Result<Vec<Checkpoint>> {
        let mut jobs = self.list_jobs()?;
        for checkpoint in &mut jobs {
            if matches!(checkpoint.job.status.as_str(), "running" | "pausing") {
                checkpoint.job.status = "interrupted".into();
                checkpoint.job.message = "上次运行意外中断，已恢复最近的持久化状态".into();
                match self.load(checkpoint.job.task_id) {
                    Ok(task) => checkpoint.stopped(&task),
                    Err(_) if checkpoint.job.kind == "archive_import" => {
                        checkpoint.job.resumable = self.job_input_path(checkpoint.job.id).is_file();
                        checkpoint.job.recovery_note =
                            "重新校验已保存的归档输入；仅在校验完成后发布只读归档。".into();
                    }
                    Err(e) => {
                        checkpoint.job.resumable = false;
                        checkpoint.job.error = Some(e.to_string());
                    }
                }
                self.save_job(checkpoint)?;
            }
        }
        Ok(jobs)
    }
    pub fn job_input_path(&self, id: Uuid) -> std::path::PathBuf {
        self.directory
            .join("jobs")
            .join("inputs")
            .join(format!("{id}.json"))
    }
    pub fn job_result_path(&self, id: Uuid) -> std::path::PathBuf {
        self.directory
            .join("jobs")
            .join("results")
            .join(format!("{id}.json"))
    }
}
