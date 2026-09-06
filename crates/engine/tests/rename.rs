//! 文件名重生集成测试：Mock LLM → 建议新名 → 生成移动对。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use ds_engine::config::Pricing;
use ds_engine::cost::{CostTracker, Usage};
use ds_engine::domain::FileEntry;
use ds_engine::llm::{ChatOptions, ChatResponse, LlmClient, Message, ToolDef};
use ds_engine::permission::{AccessTier, PermissionConfig, PermissionRule};
use ds_engine::rename::{rename_with_llm, RenameRequest};
use ds_engine::trajectory::TrajectoryLogger;

struct MockRenameLlm {
    response: String,
}

#[async_trait]
impl LlmClient for MockRenameLlm {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDef],
        _options: &ChatOptions,
        _cancel: &CancellationToken,
    ) -> anyhow::Result<ChatResponse> {
        Ok(ChatResponse {
            billing_metadata: serde_json::Value::Null,
            finish_reason: Some("stop".into()),
            message: Message::assistant(self.response.clone(), vec![]),
            usage: Usage {
                prompt_tokens: 300,
                completion_tokens: 40,
                ..Usage::default()
            },
        })
    }
}

fn make_file(path: &str, content: &[u8]) -> FileEntry {
    let p = PathBuf::from(path);
    std::fs::write(&p, content).unwrap();
    FileEntry::from_path(p).unwrap()
}

#[tokio::test]
async fn rename_produces_moves() {
    let dir = std::env::temp_dir().join(format!("ds_rename_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let traj = Arc::new(TrajectoryLogger::open(&dir.join("traj.jsonl")).unwrap());

    let f1 = make_file(
        &dir.join("qB6f3a91EE.mp4").to_string_lossy(),
        b"video data here",
    );
    let f2 = make_file(
        &dir.join("8a3f2b1c_report.pdf").to_string_lossy(),
        b"%PDF-1.4 report",
    );

    let mock = MockRenameLlm {
        response: serde_json::json!([
            {"path":f1.path,"new_name":"三体 (2023) S01E01","reason":"识别为三体动画"},
            {"path":f2.path,"new_name":"季度财务报告_2024Q3","reason":"PDF财务报告"}
        ])
        .to_string(),
    };

    let cost = CostTracker::new(Pricing::default(), None);
    let cancel = CancellationToken::new();
    let permissions = PermissionConfig::default();
    let examples = vec![("example_old.mp4".into(), "Example New".into())];

    let req = RenameRequest {
        files: &[f1.clone(), f2.clone()],
        permissions: &permissions,
        client: &mock,
        cost: &cost,
        model: "test",
        temperature: 0.1,
        batch_id: uuid::Uuid::new_v4(),
        trajectory: &traj,
        cancel: &cancel,
        examples: &examples,
    };

    let result = rename_with_llm(&req).await.unwrap();

    assert_eq!(result.stats.renamed, 2);
    assert_eq!(result.stats.total, 2);

    // 检查扩展名保留
    let r1 = &result.renames[0];
    assert!(
        r1.new_name.ends_with(".mp4"),
        "应保留 .mp4 扩展名: {}",
        r1.new_name
    );
    assert!(r1.new_name.contains("三体"));

    let r2 = &result.renames[1];
    assert!(
        r2.new_name.ends_with(".pdf"),
        "应保留 .pdf 扩展名: {}",
        r2.new_name
    );

    // 新路径应在同一目录
    assert_eq!(r1.new_path.parent(), f1.path.parent());
    assert_eq!(r2.new_path.parent(), f2.path.parent());

    // 验证 token 统计
    assert_eq!(result.stats.usage.prompt_tokens, 300);
    assert_eq!(result.stats.usage.completion_tokens, 40);

    // 验证可提取移动对
    let moves = ds_engine::rename::moves_from_rename_result(&result);
    assert_eq!(moves.len(), 2);
    assert_eq!(moves[0].0, f1.path);
}

#[tokio::test]
async fn rename_unchanged_keeps_original() {
    let dir = std::env::temp_dir().join(format!("ds_rename_uc_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let traj = Arc::new(TrajectoryLogger::open(&dir.join("traj.jsonl")).unwrap());

    // 已经是可读名 → LLM 返回原名 → 应计入 unchanged
    let f = make_file(
        &dir.join("三体 (2023) S01E01.mp4").to_string_lossy(),
        b"data",
    );

    let mock = MockRenameLlm {
        response: serde_json::json!([{"path":f.path,"new_name":"三体 (2023) S01E01","reason":"原名已可读"}]).to_string(),
    };

    let cost = CostTracker::new(Pricing::default(), None);
    let cancel = CancellationToken::new();
    let permissions = PermissionConfig::default();

    let req = RenameRequest {
        files: &[f.clone()],
        permissions: &permissions,
        client: &mock,
        cost: &cost,
        model: "test",
        temperature: 0.1,
        batch_id: uuid::Uuid::new_v4(),
        trajectory: &traj,
        cancel: &cancel,
        examples: &[],
    };

    let result = rename_with_llm(&req).await.unwrap();
    assert_eq!(result.stats.unchanged, 1);
    assert_eq!(result.stats.renamed, 0);
    assert!(result.renames.is_empty());
}

#[tokio::test]
async fn rename_respects_permission_tier() {
    let dir = std::env::temp_dir().join(format!("ds_rename_perm_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let traj = Arc::new(TrajectoryLogger::open(&dir.join("traj.jsonl")).unwrap());

    // xlsx 文件 tier=None → 文件名和内容均不得发送给 LLM
    let f = make_file(
        &dir.join("data_table.xlsx").to_string_lossy(),
        b"binary xlsx data",
    );

    let mock = MockRenameLlm {
        response: serde_json::json!([{"path":f.path,"new_name":"数据表","reason":"推测为数据表"}])
            .to_string(),
    };

    // xlsx → none
    let permissions = PermissionConfig {
        default: AccessTier::FilenameOnly,
        content_slice_bytes: 4096,
        rules: vec![PermissionRule {
            category: None,
            extensions: vec!["xlsx".into()],
            min_bytes: None,
            max_bytes: None,
            tier: AccessTier::None,
        }],
    };

    let cost = CostTracker::new(Pricing::default(), None);
    let cancel = CancellationToken::new();

    let req = RenameRequest {
        files: &[f.clone()],
        permissions: &permissions,
        client: &mock,
        cost: &cost,
        model: "test",
        temperature: 0.1,
        batch_id: uuid::Uuid::new_v4(),
        trajectory: &traj,
        cancel: &cancel,
        examples: &[],
    };

    let result = rename_with_llm(&req).await.unwrap();
    // Denied files are excluded before any API call, including their names.
    assert_eq!(result.stats.renamed, 0);
    assert_eq!(result.stats.skipped, 1);
    assert_eq!(result.stats.usage.total(), 0);
}
