use super::{array, text};
use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Default)]
pub struct StateData {
    pub requests: Vec<Value>,
    pub active: usize,
    pub maximum: usize,
    truncated: HashSet<String>,
    failed_once: bool,
}
pub struct Mock {
    pub url: String,
    pub state: Arc<Mutex<StateData>>,
    handle: tokio::task::JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
impl Mock {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(StateData::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .fallback(any(handle))
            .with_state(state.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { url, state, handle }
    }
    pub fn wire(&self) -> Vec<Value> {
        self.state.lock().unwrap().requests.clone()
    }
    pub fn bodies(&self) -> Vec<Value> {
        self.wire()
            .into_iter()
            .filter_map(|r| (!r["body"].is_null()).then(|| r["body"].clone()))
            .collect()
    }
    pub fn maximum(&self) -> usize {
        self.state.lock().unwrap().maximum
    }
    pub fn reset_maximum(&self) {
        self.state.lock().unwrap().maximum = 0;
    }
}
struct Active(Arc<Mutex<StateData>>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.lock().unwrap().active -= 1;
    }
}
pub fn context(content: &Value) -> Value {
    let s = if content.is_array() {
        content
            .as_array()
            .unwrap()
            .iter()
            .find_map(|v| v["text"].as_str())
            .unwrap_or("")
    } else {
        content.as_str().unwrap_or("")
    };
    serde_json::from_str(s).unwrap_or(Value::Null)
}
async fn pause(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
async fn handle(State(state): State<Arc<Mutex<StateData>>>, request: Request) -> Response {
    let path = request.uri().to_string();
    let method = request.method().clone();
    let headers: serde_json::Map<String, Value> = request
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap())))
        .collect();
    let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    {
        let mut s = state.lock().unwrap();
        s.requests
            .push(json!({"path":path,"headers":headers,"body":body}));
        s.active += 1;
        s.maximum = s.maximum.max(s.active);
    }
    let _active = Active(state.clone());
    let key = headers
        .get("authorization")
        .and_then(Value::as_str)
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(Value::as_str))
        .unwrap_or("");
    if method == axum::http::Method::GET {
        if path.starts_with("/denied/") {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(json!({"error":key})),
            )
                .into_response();
        }
        if path.starts_with("/redirect/") {
            return (
                axum::http::StatusCode::FOUND,
                [("Location", "/leak/models")],
                Json(json!({})),
            )
                .into_response();
        }
        if path.starts_with("/echo/") {
            return Json(json!({"data":[{"id":key}]})).into_response();
        }
        if path.starts_with("/missing/") {
            return (axum::http::StatusCode::NOT_FOUND, Json(json!({}))).into_response();
        }
        return Json(if path.starts_with("/native/") {
            if path.contains("after_id="){json!({"data":[{"id":"claude-sonnet-4-6"}],"has_more":false})}
            else{json!({"data":[{"id":"claude-opus-4-8","max_input_tokens":1000000,"max_tokens":128000,"capabilities":{"image_input":{"supported":true}}}],"has_more":true,"last_id":"claude-opus-4-8"})}
        }else{json!({"data":[{"id":"deepseek-v4-flash","context_length":96000,"max_output_tokens":8000},{"id":"deepseek-v4-flash-vision-exp"},{"id":"glm-5.2"},{"id":"mimo-v2.5"},{"id":"LongCat-2.0"},{"id":"hy3"},{"id":"gpt-6-astra"},{"id":"future-unverified"},{"id":"text-embedding-3-large"}]})}).into_response();
    }
    if path == "/search" {
        return Json(json!({"results":[{"title":"Public fixture evidence","url":"https://example.com/fixture","content":"Local search fixture, no network request."}]})).into_response();
    }
    let model = body["model"].as_str().unwrap_or("");
    let messages = if body["messages"].is_array() {
        array(&body["messages"])
    } else {
        array(&body["input"])
    };
    let raw = serde_json::to_string(messages).unwrap();
    if model == "runtime-denied" {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({}))).into_response();
    }
    if model.starts_with("runtime-") {
        pause(220).await;
    }
    if model == "runtime-timeout" {
        pause(2000).await;
    }
    if model == "checkpoint-fixture" {
        pause(5000).await;
    }
    if raw.contains("slow-fixture") || model == "planning-pause-fixture" {
        pause(8000).await;
    }
    if raw.contains("slow-archive-check") {
        pause(1000).await;
    }
    let tools = body["tools"].as_array().is_some_and(|t| {
        t.iter()
            .any(|t| t["function"]["name"] == "submit_classifications")
    });
    let mut response = if tools {
        classify(&body, &state).await
    } else {
        let system = messages
            .first()
            .and_then(|m| m["content"].as_str())
            .unwrap_or("");
        let last = &messages.last().unwrap()["content"];
        let ctx = context(last);
        let answer = if model == "media-fixture" {
            json!({"name":ctx["name"],"reason":"fixture retains name"})
        } else if last == "Reply with OK." {
            json!("OK")
        } else if system.contains("先查看文件类型统计总览") {
            let mut groups = array(&ctx["groups"]).to_vec();
            groups.sort_by_key(|g| std::cmp::Reverse(g["eligible"].as_u64().unwrap_or(0)));
            json!({"summary":"本地测试总览","inspect_order":groups.iter().map(|g|g["id"].clone()).collect::<Vec<_>>()})
        } else if system.contains("按类型检查当前批次文件") {
            if model == "inspection-pause-fixture" && ctx["batch"] == 2 {
                pause(4000).await;
            }
            json!({"summary":format!("本地测试：{}已检查 {} 个文件，可按工作用途和学习用途进一步区分。",text(&ctx["type_name"]),ctx["already_inspected"].as_u64().unwrap()+array(&ctx["files"]).len() as u64)})
        } else if system.contains("依据现有信息提出清晰简洁文件名") {
            let fail = {
                let mut s = state.lock().unwrap();
                let f = model == "runtime-rename-fail"
                    && text(&ctx["name"]).starts_with("000")
                    && !s.failed_once;
                if f {
                    s.failed_once = true;
                }
                f
            };
            if fail {
                json!("invalid response")
            } else {
                json!({"name":format!("可读_{}",text(&ctx["name"])),"reason":"本地测试模型命名建议"})
            }
        } else if system.contains("复核清理候选") {
            json!({"suggestions":array(&ctx).iter().map(|c|json!({"index":c["index"],"reason":"请确认用途与备份后再自行决定是否清理。"})).collect::<Vec<_>>()})
        } else {
            let ctx = messages
                .get(1)
                .and_then(|m| m["content"].as_str())
                .and_then(|s| s.split_once('：'))
                .and_then(|(_, s)| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            proposal(model, &ctx)
        };
        let answer = if answer.is_string() {
            text(&answer).to_owned()
        } else {
            answer.to_string()
        };
        if path.ends_with("/messages") {
            json!({"type":"message","content":[{"type":"text","text":answer}],"usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":3},"stop_reason":"end_turn"})
        } else if path.ends_with("/responses") {
            json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":answer}]}],"usage":{"input_tokens":10,"output_tokens":2}})
        } else {
            json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":answer}}],"usage":{"prompt_tokens":321,"completion_tokens":45}})
        }
    };
    if !tools
        && [
            "truncated-reasoning-fixture",
            "truncated-json-fixture",
            "empty-answer-fixture",
            "invalid-json-fixture",
        ]
        .contains(&model)
    {
        response["choices"][0]["message"]["content"] = json!(match model {
            "truncated-json-fixture" => "{\"message\":\"unfinished",
            "invalid-json-fixture" => "not valid JSON",
            _ => "",
        });
        if model.starts_with("truncated-") {
            response["choices"][0]["finish_reason"] = json!("length");
            response["usage"]["completion_tokens"] = body["max_tokens"].clone();
        }
    }
    if model == "missing-usage-fixture" {
        response.as_object_mut().unwrap().remove("usage");
    }
    if model.starts_with("runtime-") {
        response["usage"] = json!({"prompt_tokens":100,"completion_tokens":20});
    }
    if last_connection(messages) && !model.ends_with("fixture") && !model.starts_with("runtime-") {
        response["usage"] = if path.ends_with("chat/completions") {
            json!({"prompt_tokens":10,"completion_tokens":2})
        } else {
            response["usage"].clone()
        };
    }
    Json(response).into_response()
}
fn last_connection(messages: &[Value]) -> bool {
    messages
        .last()
        .is_some_and(|m| m["content"] == "Reply with OK.")
}
async fn classify(body: &Value, state: &Arc<Mutex<StateData>>) -> Value {
    let model = text(&body["model"]);
    let messages = array(&body["messages"]);
    let ctx = context(&messages[1]["content"]);
    let results: Vec<Value> = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| context(&m["content"]))
        .collect();
    if model == "classification-pause-fixture" && ctx["batch_id"] == "b2" {
        pause(5000).await;
    }
    if model.starts_with("review-semantic-") {
        pause(350).await;
    }
    if model == "parallel-progress-fixture" {
        pause(if ctx["batch_id"] == "b1" { 700 } else { 350 }).await;
    }
    let target = array(&ctx["nodes"])
        .iter()
        .find(|n| {
            n["selectable"] == true
                && match model {
                    "directory-classification-fixture" => text(&n["name"]).contains("文档"),
                    "review-semantic-fixture" | "review-semantic-resume-fixture" => {
                        text(&n["name"]).contains("学习笔记")
                    }
                    _ => true,
                }
        })
        .unwrap();
    let assignments: Vec<Value> = array(&ctx["files"])
        .iter()
        .map(|f| json!({"file_id":f["id"],"node_id":target["id"],"reason":"本地批量分类测试"}))
        .collect();
    let mut name = "submit_classifications";
    let mut args = json!({"batch_id":ctx["batch_id"],"assignments":assignments});
    match model {
        "classification-evidence-fixture"
        | "deepseek-evidence-fixture"
        | "directory-classification-fixture" => {
            let already: HashSet<String> = results
                .iter()
                .flat_map(|r| r["files"].as_array().into_iter().flatten())
                .filter_map(|f| f["file_id"].as_str().map(str::to_owned))
                .collect();
            let mut eligible = Vec::new();
            for f in array(&ctx["files"]).iter().chain(array(&ctx["examples"])) {
                if f["evidence"] != "none"
                    && !already.contains(text(&f["id"]))
                    && !eligible.contains(&f["id"])
                {
                    eligible.push(f["id"].clone());
                }
            }
            if !eligible.is_empty() {
                name = "read_file_evidence";
                args = json!({"file_ids":eligible.into_iter().take(4).collect::<Vec<_>>()});
            }
            if model.starts_with("deepseek") {
                assert!(messages
                    .iter()
                    .filter(|m| m["role"] == "assistant")
                    .all(|m| m["reasoning_content"] == "fixture continuation"));
            }
        }
        "classification-forbidden-read-fixture" if results.is_empty() => {
            name = "read_file_evidence";
            args = json!({"file_ids":["../../private.xlsx"]});
        }
        "classification-permission-fixture" if results.is_empty() => {
            name = "read_file_evidence";
            args = json!({"file_ids":array(&ctx["files"]).iter().take(4).map(|f|f["id"].clone()).collect::<Vec<_>>()});
        }
        "classification-invalid-node-fixture" => {
            args["assignments"][0]["node_id"] = json!("n999999")
        }
        "classification-incomplete-fixture" => {
            args["assignments"].as_array_mut().unwrap().pop();
        }
        "classification-duplicate-fixture" => {
            let first = args["assignments"][0].clone();
            *args["assignments"]
                .as_array_mut()
                .unwrap()
                .last_mut()
                .unwrap() = first;
        }
        "classification-path-fixture" => {
            args["assignments"][0]["destination"] = json!("../../outside.txt")
        }
        "classification-null-fixture" => {
            for a in args["assignments"].as_array_mut().unwrap() {
                a["node_id"] = Value::Null;
            }
        }
        "classification-missing-node-fixture" => {
            args["assignments"][0]
                .as_object_mut()
                .unwrap()
                .remove("node_id");
        }
        _ => {}
    }
    let mut message = json!({"role":"assistant","content":"","tool_calls":[{"id":format!("call_fixture_{}",messages.len()),"type":"function","function":{"name":name,"arguments":args.to_string()}}]});
    if model.starts_with("deepseek") {
        message["reasoning_content"] = json!("fixture continuation");
    }
    if model == "invalid-json-fixture" {
        message = json!({"role":"assistant","content":"not valid JSON"});
    }
    let truncate = model == "classification-truncated-fixture"
        || ([
            "classification-truncated-once-fixture",
            "review-semantic-resume-fixture",
        ]
        .contains(&model)
            && ctx["batch_id"] == "b2"
            && state
                .lock()
                .unwrap()
                .truncated
                .insert(messages[1]["content"].to_string()));
    if truncate {
        message["tool_calls"][0]["function"]["arguments"] = json!(format!(
            "{{\"batch_id\":\"{}\",\"assignments\":[",
            text(&ctx["batch_id"])
        ));
    }
    json!({"choices":[{"finish_reason":if truncate{"length"}else if model=="invalid-json-fixture"{"stop"}else{"tool_calls"},"message":message}],"usage":{"prompt_tokens":321,"completion_tokens":if truncate{body["max_tokens"].clone()}else{json!(45)}}})
}
fn change(target: &Value, after: Value) -> Value {
    json!({"kind":"node","target":target,"after":after})
}
fn proposal(model: &str, ctx: &Value) -> Value {
    let mut changes = Vec::new();
    if ctx["editable"] == true && ctx["scene"] == "review" {
        changes = match model {
            "review-split-fixture" => vec![change(
                &json!("review-video"),
                json!({"name":"视频","parent":"root","rule_type":"simple","extensions":["mp4","mkv"]}),
            )],
            "review-placement-fixture" => vec![
                change(
                    &json!("review-notes"),
                    json!({"name":"笔记","parent":"docs","rule_type":"complex","note":"用户指定的笔记"}),
                ),
                json!({"kind":"placement","target":array(&ctx["review_files"]).iter().find(|f|f["name"]=="notes.txt").unwrap()["id"],"after":{"node_id":"review-notes"}}),
            ],
            "review-keep-fixture" => vec![
                json!({"kind":"placement","target":array(&ctx["review_files"]).iter().find(|f|f["name"]=="clip.mp4").unwrap()["id"],"after":{"node_id":null}}),
            ],
            "review-invalid-fixture" => {
                vec![json!({"kind":"placement","target":"f99999","after":{"node_id":"docs"}})]
            }
            "review-semantic-fixture" | "review-semantic-resume-fixture" => vec![change(
                &json!("review-semantic"),
                json!({"name":"学习笔记","parent":"docs","rule_type":"complex","note":"学习文本和课程笔记"}),
            )],
            _ => vec![],
        };
    } else if ctx["editable"] == true && ctx["scene"] == "tree" && ctx["nodes"].is_array() {
        let nodes = array(&ctx["nodes"]);
        match model {
            "duplicate-template-fixture" => {
                let photo = nodes.iter().find(|n| n["name"] == "照片").unwrap();
                changes = vec![
                    change(
                        &json!("duplicate-photo"),
                        json!({"name":"照片","parent":photo["parent"],"rule_type":"complex","note":"实拍照片，排除截图和设计素材"}),
                    ),
                    change(
                        &json!("photo-travel"),
                        json!({"name":"旅行照片","parent":"duplicate-photo","rule_type":"complex","note":"旅行实拍照片"}),
                    ),
                ];
            }
            "new-top-level-fixture" => {
                changes = vec![
                    change(
                        &json!("new-music"),
                        json!({"name":"音频","parent":"root","rule_type":"simple","extensions":["mp3","wav","flac"],"note":"音乐与录音"}),
                    ),
                    change(
                        &json!("new-recordings"),
                        json!({"name":"录音","parent":"new-music","rule_type":"complex","note":"会议与课堂录音"}),
                    ),
                    change(
                        &json!("new-meetings"),
                        json!({"name":"会议","parent":"new-recordings","rule_type":"complex","note":"会议录音"}),
                    ),
                ];
            }
            "tree-structure-fixture" | "invalid-tree-fixture" => {
                let video = nodes
                    .iter()
                    .find(|n| {
                        n["parent"] == "root" && array(&n["extensions"]).contains(&json!("mp4"))
                    })
                    .unwrap();
                let docs = nodes
                    .iter()
                    .find(|n| {
                        n["parent"] == "root" && array(&n["extensions"]).contains(&json!("pdf"))
                    })
                    .unwrap();
                let children: Vec<_> = nodes
                    .iter()
                    .filter(|n| n["parent"] == video["id"])
                    .collect();
                let mut ext = array(&docs["extensions"]).to_vec();
                ext.extend([json!("htm"), json!("xml")]);
                changes = vec![
                    change(
                        &json!("fixture-campus"),
                        json!({"name":"校园记录/学业","parent":if model=="invalid-tree-fixture"{"nonexistent-parent"}else{"fixture-topics"},"rule_type":"complex","note":"校园纪实与集体活动"}),
                    ),
                    change(
                        &json!("fixture-topics"),
                        json!({"name":"专题素材","parent":video["id"],"rule_type":"complex","note":"按用途组织短视频"}),
                    ),
                    change(
                        &children[0]["id"],
                        json!({"name":"影视长片","note":"完整电影和剧场版"}),
                    ),
                    change(&children[1]["id"], Value::Null),
                    change(
                        &children[2]["id"],
                        json!({"parent":"fixture-topics","note":"视频剪辑可复用的素材片段"}),
                    ),
                    change(
                        &docs["id"],
                        json!({"extensions":ext,"note":"文档与结构化参考资料"}),
                    ),
                ];
            }
            _ => {
                if let Some(n) = nodes.first() {
                    let mut n = n.clone();
                    n.as_object_mut().unwrap().remove("example_context");
                    n["note"] = json!("保留原有分类，减少不必要的移动。");
                    changes.push(change(&n["id"], n.clone()));
                }
            }
        }
    }
    json!({"message":"这是本地测试模型的建议，供你审查后合并。","changes":changes})
}
