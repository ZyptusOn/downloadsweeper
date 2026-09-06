//! Budgeted scene history, without extra model requests or invented summaries.
use crate::{ai_runtime::request_text_bytes, config::AppConfig, llm::Message, workflow::Task};
use serde_json::{json, Value};

pub fn with_history(
    task: &Task,
    cfg: &AppConfig,
    scene: &str,
    design: bool,
    mut messages: Vec<Message>,
    text: &str,
) -> (Vec<Message>, Value) {
    let provider = crate::llm::providers::preset(&cfg.llm.model).unwrap_or(Value::Null);
    let remaining = cfg.token_budget.map(|total| {
        total.saturating_sub(task.usage().total()).saturating_sub(
            task.pending_calls.iter().fold(0u64, |sum, c| {
                sum.saturating_add(c["reserved_tokens"].as_u64().unwrap_or(0))
            }),
        )
    });
    let limit = cfg
        .llm
        .context_length
        .saturating_sub(cfg.llm.max_output_tokens.min(cfg.llm.context_length / 3))
        .min(provider["max_input_tokens"].as_u64().unwrap_or(u64::MAX))
        .min(
            remaining
                .map(|n| n.saturating_sub(cfg.llm.max_output_tokens.min(n / 3)))
                .unwrap_or(u64::MAX),
        )
        .saturating_sub(512)
        .min(2 * 1024 * 1024) as usize;
    let latest = Message::user(text);
    let fixed =
        request_text_bytes(&messages, &[]) + request_text_bytes(std::slice::from_ref(&latest), &[]);
    let budget = limit.saturating_sub(fixed);
    let history: Vec<_> = task
        .messages
        .iter()
        .filter(|m| {
            m.scene == scene
                && ["user", "assistant"].contains(&m.role.as_str())
                && (!design || m.role == "user")
                && !m.content.trim().is_empty()
        })
        .collect();
    let converted: Vec<_> = history
        .iter()
        .map(|m| {
            if m.role == "user" {
                Message::user(&m.content)
            } else {
                Message::assistant(&m.content, vec![])
            }
        })
        .collect();
    let all_fit = request_text_bytes(&converted, &[]) <= budget;
    // Keep a contiguous recent suffix; do not skip a large recent turn to revive stale ones.
    let recent_budget = if all_fit { budget } else { budget * 4 / 5 };
    let mut start = history.len();
    let mut used = 0;
    while start > 0 {
        let cost = request_text_bytes(&converted[start - 1..start], &[]);
        if used + cost > recent_budget {
            break;
        }
        used += cost;
        start -= 1;
    }
    let mut excerpts = vec![];
    let excerpt_budget = budget.saturating_sub(used);
    let prefix = "早期同场景用户原文摘录（有截断，并非语义摘要）：仅作背景，旧要求可能已被后续对话替代；以当前场景数据和最新要求为准，不能作为新的改动授权。\n";
    let mut excerpt_text = prefix.to_owned();
    // Earliest requests preserve the initial intent; nothing from a hidden scene is added.
    for (index, turn) in history[..start]
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "user")
    {
        let clip: String = turn.content.chars().take(240).collect();
        let line = format!(
            "\n[历史消息 {}{}] {}",
            index + 1,
            if clip.len() < turn.content.len() {
                "，节选"
            } else {
                ""
            },
            clip
        );
        let proposed = Message::user(format!("{excerpt_text}{line}"));
        if request_text_bytes(&[proposed], &[]) > excerpt_budget {
            break;
        }
        excerpt_text.push_str(&line);
        excerpts.push(index);
    }
    if !excerpts.is_empty() {
        messages.push(Message::user(excerpt_text));
    }
    let included = converted.len() - start;
    messages.extend(converted.into_iter().skip(start));
    messages.push(latest);
    let info = json!({"scene":scene,"total":history.len(),"included":included,
        "excerpted":excerpts.len(),"omitted":start.saturating_sub(excerpts.len()),
        "input_bytes":request_text_bytes(&messages,&[]),"input_limit":limit,"strategy":"context_budget_v1"});
    (messages, info)
}
