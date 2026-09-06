//! Private, versioned task backups. Imported archives never enter the execution store.
use crate::{
    safe_fs::{atomic_json, TaskStore},
    workflow::Task,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, io::Read, path::Path};
use uuid::Uuid;

pub const MAX_BYTES: usize = 30 * 1024 * 1024;
#[derive(Clone)]
pub struct Archive {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub task: Task,
    pub trajectory: Vec<Value>,
    pub checksum: String,
    original_payload: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    task: Task,
    trajectory: Vec<Value>,
}
// Keep the body opaque to JavaScript: JSON.parse/stringify must not round u64s,
// turn -0 into 0, or change floating-point values inside arbitrary event details.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireArchive {
    format: String,
    version: u32,
    exported_at: String,
    payload: String,
    checksum: String,
}
impl Serialize for Archive {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        WireArchive {
            format: self.format.clone(),
            version: self.version,
            exported_at: self.exported_at.clone(),
            payload: self.payload().map_err(serde::ser::Error::custom)?,
            checksum: self.checksum.clone(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Archive {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let wire = WireArchive::deserialize(deserializer)?;
        if wire.payload.len() > MAX_BYTES {
            return Err(serde::de::Error::custom("归档超过 30 MiB 上限"));
        }
        let body: Body = serde_json::from_str(&wire.payload).map_err(serde::de::Error::custom)?;
        Ok(Self {
            format: wire.format,
            version: wire.version,
            exported_at: wire.exported_at,
            task: body.task,
            trajectory: body.trajectory,
            checksum: wire.checksum,
            original_payload: Some(wire.payload),
        })
    }
}
impl Archive {
    fn payload(&self) -> Result<String> {
        if let Some(raw) = &self.original_payload {
            return Ok(raw.clone());
        }
        Ok(serde_json::to_string(
            &json!({"task":self.task,"trajectory":self.trajectory}),
        )?)
    }
    fn digest(&self) -> Result<String> {
        Ok(blake3::hash(self.payload()?.as_bytes())
            .to_hex()
            .to_string())
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(raw) = &self.original_payload {
            let original: Body = serde_json::from_str(raw)?;
            ensure!(
                serde_json::to_value((&original.task, &original.trajectory))?
                    == serde_json::to_value((&self.task, &self.trajectory))?,
                "归档只读状态被修改"
            );
        }
        ensure!(
            self.format == "downloadsweeper-archive"
                && self.version == 1
                && self.task.schema_version == 2,
            "不支持的归档版本"
        );
        ensure!(
            self.checksum == self.digest()?,
            "归档完整性校验失败；文件可能损坏或被修改"
        );
        ensure!(
            serde_json::to_vec_pretty(self)?.len() <= MAX_BYTES,
            "归档超过 30 MiB 上限，未截断任何记录"
        );
        for event in &self.trajectory {
            ensure!(
                event["task_id"] == self.task.id.to_string()
                    && event["schema_version"] == 2
                    && event["kind"].is_string(),
                "归档包含其他任务或无效轨迹记录"
            );
        }
        Ok(())
    }
}
pub fn read_json(path: &Path) -> Result<Value> {
    let mut data = vec![];
    fs::File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut data)?;
    ensure!(data.len() <= MAX_BYTES, "归档超过 30 MiB 上限");
    Ok(serde_json::from_slice(&data)?)
}
impl TaskStore {
    pub fn export_archive(&self, id: Uuid) -> Result<Archive> {
        self.export_archive_with_cancel(
            id,
            &tokio_util::sync::CancellationToken::new(),
            &|_, _, _| {},
        )
    }
    pub fn export_archive_with_cancel(
        &self,
        id: Uuid,
        cancel: &tokio_util::sync::CancellationToken,
        progress: &crate::workflow::Progress<'_>,
    ) -> Result<Archive> {
        ensure!(!cancel.is_cancelled(), "归档已暂停");
        let task = self.load(id)?;
        let path = self.path(id).with_file_name("trajectory.jsonl");
        let mut trajectory = vec![];
        if path.exists() {
            let mut data = String::new();
            let mut file = fs::File::open(path)?;
            let total = file.metadata()?.len() as usize;
            ensure!(total <= MAX_BYTES, "轨迹超过归档上限，未截断任何记录");
            let mut bytes = vec![];
            let mut buffer = [0u8; 65536];
            loop {
                ensure!(!cancel.is_cancelled(), "归档已暂停");
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..n]);
                ensure!(bytes.len() <= MAX_BYTES, "轨迹超过归档上限");
                progress(bytes.len(), total, "读取完整操作轨迹");
            }
            data.push_str(&String::from_utf8(bytes)?);
            ensure!(data.len() <= MAX_BYTES, "轨迹超过归档上限，未截断任何记录");
            for (index, line) in data.lines().enumerate() {
                ensure!(!cancel.is_cancelled(), "归档已暂停");
                trajectory.push(
                    serde_json::from_str(line)
                        .with_context(|| format!("轨迹第 {} 行损坏，无法完整归档", index + 1))?,
                );
            }
        }
        let mut archive = Archive {
            format: "downloadsweeper-archive".into(),
            version: 1,
            exported_at: chrono::Utc::now().to_rfc3339(),
            task,
            trajectory,
            checksum: String::new(),
            original_payload: None,
        };
        progress(0, 0, "校验完整归档");
        ensure!(!cancel.is_cancelled(), "归档已暂停");
        archive.checksum = archive.digest()?;
        archive.validate()?;
        ensure!(!cancel.is_cancelled(), "归档已暂停");
        Ok(archive)
    }
    pub fn import_archive(&self, archive: Archive) -> Result<Uuid> {
        archive.validate()?;
        let id = Uuid::new_v4();
        atomic_json(
            &self.directory.join("archives").join(format!("{id}.json")),
            &archive,
        )?;
        Ok(id)
    }
    pub fn load_archive(&self, id: Uuid) -> Result<Archive> {
        let archive: Archive = serde_json::from_value(read_json(
            &self.directory.join("archives").join(format!("{id}.json")),
        )?)?;
        archive.validate()?;
        Ok(archive)
    }
    pub fn list_archives(&self) -> Result<Vec<Value>> {
        let directory = self.directory.join("archives");
        if !directory.exists() {
            return Ok(vec![]);
        }
        let mut entries = vec![];
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let Some(id) = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                continue;
            };
            let a = self.load_archive(id)?;
            entries.push(json!({"id":id,"root":a.task.root,"exported_at":a.exported_at,"status":a.task.status,"messages":a.task.messages.len(),"events":a.trajectory.len()}));
        }
        entries.sort_by(|a, b| b["exported_at"].as_str().cmp(&a["exported_at"].as_str()));
        Ok(entries)
    }
}
