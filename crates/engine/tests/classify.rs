//! 混合分类管线集成测试：规则 → 缓存 → LLM 兜底。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use ds_engine::classify::{classify_hybrid, CategoryDef, ClassifyRequest, FewShotExample};
use ds_engine::cost::CostTracker;
use ds_engine::cost::Usage;
use ds_engine::domain::FileEntry;
use ds_engine::fingerprint::{cached_entry, ClassificationCache};
use ds_engine::llm::{ChatOptions, ChatResponse, LlmClient, Message};
use ds_engine::trajectory::TrajectoryLogger;

/// Mock LLM 客户端：返回预设的 JSON 分类结果。
struct MockLlm {
    response: String,
}

#[async_trait]
impl LlmClient for MockLlm {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ds_engine::llm::ToolDef],
        _options: &ChatOptions,
        _cancel: &CancellationToken,
    ) -> anyhow::Result<ChatResponse> {
        Ok(ChatResponse {
            billing_metadata: serde_json::Value::Null,
            finish_reason: Some("stop".into()),
            message: Message::assistant(self.response.clone(), vec![]),
            usage: Usage {
                prompt_tokens: 500,
                completion_tokens: 50,
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

fn setup() -> (PathBuf, Arc<TrajectoryLogger>) {
    let root = std::env::temp_dir().join(format!("ds_cls_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let traj = Arc::new(TrajectoryLogger::open(&root.join("traj.jsonl")).unwrap());
    (root, traj)
}

#[tokio::test]
async fn hybrid_rule_cache_llm() {
    let (root, traj) = setup();

    // 文件：
    // - report.pdf → 规则命中（文档）
    // - movie.mkv → 规则未命中，缓存未命中 → LLM 分类为"电影"
    // - unknown.bin → LLM 返回"未分类"
    let f_pdf = make_file(&root.join("report.pdf").to_string_lossy(), b"pdf content");
    let f_mkvi = make_file(
        &root.join("movie.mkv").to_string_lossy(),
        b"mkv header data here",
    );
    let f_bin = make_file(&root.join("unknown.bin").to_string_lossy(), b"binary data");

    let target_dir = root.join("organized");

    // 规则：pdf → 文档
    let rules = vec![(vec!["pdf".to_string()], "文档".to_string())];

    // 类别
    let categories: Vec<CategoryDef> = serde_json::from_str(
        r#"[
            {"name":"电影","subfolder":"电影","description":"长片"},
            {"name":"未分类","subfolder":"未分类","description":"无法确定"}
        ]"#,
    )
    .unwrap();

    // 示例
    let examples = vec![FewShotExample::new("复仇者联盟.mkv", "电影")];

    let cache = ClassificationCache::load(&root.join("cache.json")).unwrap();

    // Mock LLM: movie.mkv → 电影, unknown.bin → 未分类
    let mock = MockLlm {
        response: serde_json::json!([
            {"path":f_mkvi.path,"category":"电影"},
            {"path":f_bin.path,"category":"未分类"}
        ])
        .to_string(),
    };

    let cost = CostTracker::new(ds_engine::config::Pricing::default(), None);
    let cancel = CancellationToken::new();
    let batch_id = uuid::Uuid::new_v4();

    let req = ClassifyRequest {
        files: &[f_pdf.clone(), f_mkvi.clone(), f_bin.clone()],
        rules: &rules,
        categories: &categories,
        examples: &examples,
        target_dir: &target_dir,
        permissions: &ds_engine::permission::PermissionConfig::default(),
        cache: &cache,
        client: &mock,
        cost: &cost,
        model: "test-model",
        temperature: 0.1,
        multimodal: false,
        batch_id,
        trajectory: &traj,
        cancel: &cancel,
    };

    let result = classify_hybrid(&req).await.unwrap();

    // 规则 1, LLM 1, 未分类 1
    assert_eq!(result.stats.from_rule, 1);
    assert_eq!(result.stats.from_llm, 1);
    assert_eq!(result.stats.uncategorized, 1);
    assert_eq!(result.stats.from_cache, 0);
    assert_eq!(result.moves.len(), 2); // pdf + movie.mkv

    // pdf → 文档/report.pdf
    assert!(result
        .moves
        .iter()
        .any(|(s, d)| { s == &f_pdf.path && d == &target_dir.join("文档").join("report.pdf") }));
    // movie.mkv → 电影/movie.mkv
    assert!(result
        .moves
        .iter()
        .any(|(s, d)| { s == &f_mkvi.path && d == &target_dir.join("电影").join("movie.mkv") }));
    // unknown.bin → 未分类
    assert!(result.uncategorized.contains(&f_bin.path));

    // LLM 结果应已缓存
    assert_eq!(cache.len(), 1);

    // 验证 token 统计
    assert_eq!(result.stats.usage.prompt_tokens, 500);
    assert_eq!(result.stats.usage.completion_tokens, 50);
}

#[tokio::test]
async fn cache_hit_avoids_llm() {
    let (root, traj) = setup();

    let f = make_file(
        &root.join("cached.mkv").to_string_lossy(),
        b"cached content",
    );
    let target_dir = root.join("organized");

    // 预填充缓存
    let cache = ClassificationCache::load(&root.join("cache.json")).unwrap();
    use ds_engine::fingerprint::fingerprint;
    let fp = fingerprint(&f.path).unwrap();
    cache.put(fp, cached_entry("电影", "电影", "llm"));

    // 规则不命中 mkv（没有 mkv 规则）
    let rules: Vec<(Vec<String>, String)> = vec![];
    let categories: Vec<CategoryDef> =
        serde_json::from_str(r#"[{"name":"电影","subfolder":"电影","description":"长片"}]"#)
            .unwrap();
    let examples: Vec<FewShotExample> = vec![];

    // Mock LLM — 不应被调用；如果被调用说明缓存没命中
    let mock = MockLlm {
        response: r#"[{"path":"should-not-be-called","category":"电影"}]"#.into(),
    };

    let cost = CostTracker::new(ds_engine::config::Pricing::default(), None);
    let cancel = CancellationToken::new();

    let req = ClassifyRequest {
        files: &[f.clone()],
        rules: &rules,
        categories: &categories,
        examples: &examples,
        target_dir: &target_dir,
        permissions: &ds_engine::permission::PermissionConfig::default(),
        cache: &cache,
        client: &mock,
        cost: &cost,
        model: "test",
        temperature: 0.1,
        multimodal: false,
        batch_id: uuid::Uuid::new_v4(),
        trajectory: &traj,
        cancel: &cancel,
    };

    let result = classify_hybrid(&req).await.unwrap();

    // 缓存命中，不调用 LLM
    assert_eq!(result.stats.from_cache, 1);
    assert_eq!(result.stats.from_llm, 0);
    assert_eq!(result.stats.usage.prompt_tokens, 0);
    assert!(result
        .moves
        .iter()
        .any(|(_, d)| d == &target_dir.join("电影").join("cached.mkv")));
}
