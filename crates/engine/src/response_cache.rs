//! Responses are durable before usage is acknowledged. Replay is scoped to one run.
use crate::{
    llm::Message,
    safe_fs::{atomic_json, TaskStore},
    workflow::{CallRecord, Task},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
pub struct Response {
    pub request_key: String,
    pub record: CallRecord,
    pub message: Message,
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    payload: String,
    checksum: String,
}
pub fn save(path: &Path, response: &Response) -> Result<()> {
    let payload = serde_json::to_string(response)?;
    atomic_json(
        path,
        &Envelope {
            checksum: blake3::hash(payload.as_bytes()).to_hex().to_string(),
            payload,
        },
    )
}
pub fn load(path: &Path) -> Result<Response> {
    let envelope: Envelope = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        envelope.checksum == blake3::hash(envelope.payload.as_bytes()).to_hex().as_str(),
        "模型回答检查点校验失败"
    );
    Ok(serde_json::from_str(&envelope.payload)?)
}
pub fn directory(store: &TaskStore, task: &Task) -> Option<std::path::PathBuf> {
    task.runtime_run.map(|id| {
        store
            .path(task.id)
            .with_file_name("responses")
            .join(id.to_string())
    })
}
pub fn recover(store: &TaskStore, task: &mut Task) -> Result<()> {
    let Some(directory) = directory(store, task).filter(|d| d.is_dir()) else {
        return Ok(());
    };
    let mut changed = false;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().is_none_or(|s| s != "json") {
            continue;
        }
        let response = load(&entry.path())?;
        if task
            .pending_calls
            .iter()
            .any(|p| p["id"] == response.record.id)
        {
            task.pending_calls.retain(|p| p["id"] != response.record.id);
            if !task.calls.iter().any(|c| c.id == response.record.id) {
                task.calls.push(response.record);
            }
            changed = true;
        }
    }
    if changed {
        store.save(task)?;
        store.event(
            task,
            "llm_response_recovered",
            serde_json::json!({"note":"从已落盘的完整回答恢复 usage；未发起网络请求"}),
        )?;
    }
    Ok(())
}
