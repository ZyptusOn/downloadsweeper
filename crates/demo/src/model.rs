//! Deterministic demonstration responses, not a real LLM or a quality benchmark.
use axum::Json;
use serde_json::{json, Value};
pub fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn content(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.into();
    }
    array(v)
        .iter()
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
fn context(v: &Value) -> Value {
    serde_json::from_str(&content(v)).unwrap_or(Value::Null)
}
fn change(target: &Value, after: Value) -> Value {
    json!({"kind":"node","target":target["id"],"after":after})
}
fn add(id: &str, parent: &str, name: &str, ext: &[&str], note: &str) -> Value {
    json!({"kind":"node","target":id,"after":{"name":name,"parent":parent,"rule_type":if parent=="root"{"simple"}else{"complex"},"extensions":ext,"note":note}})
}
pub fn proposal(ctx: &Value, request: &str) -> Value {
    let scene = text(&ctx["scene"]);
    let nodes = array(&ctx["nodes"]);
    let mut changes = vec![];
    if scene == "tree" {
        if !nodes.iter().any(|n| text(&n["name"]) == "音乐") {
            changes.push(add(
                "demo-music",
                "root",
                "音乐",
                &["wav", "mp3", "flac", "mid"],
                "独立收纳音乐、录音与编曲素材。",
            ));
            changes.push(add(
                "demo-music-assets",
                "demo-music",
                "编曲素材",
                &[],
                "歌曲工程导出、音乐练习与音频样本。",
            ));
        }
        if let Some(node) = nodes.iter().find(|n| text(&n["name"]) == "文档") {
            changes.push(change(
                node,
                json!({"note":"课程报告、开销表与考试成绩文件夹；文件夹整体归类，内部不拆散。"}),
            ));
            if !nodes.iter().any(|n| text(&n["name"]) == "课程项目") {
                changes.push(add(
                    "demo-course",
                    text(&node["id"]),
                    "课程项目",
                    &[],
                    "Rust 课程的设计、进度、开销和答辩文件。",
                ));
            }
        }
        if let Some(node) = nodes.iter().find(|n| text(&n["name"]) == "PDF") {
            if array(&ctx["directories"])
                .iter()
                .any(|d| d["name"] == "课程资料库" && d["class"] == "container")
            {
                changes.push(change(node,json!({"name":"论文资料","mapping":"课程资料库","note":"复用已有资料库；原内容保留，新论文放入其中。"})));
            }
        }
        return json!({"message":"【离线演示预设】新增独立音乐类别及编曲素材子目录；课程文档按用途细分。成绩文件夹整体归入文档，课程资料库复用为论文容器。视频暂在“其他”，下一步可用审查反馈调整。取消父目录建议会使依赖子目录失效。","changes":changes});
    }
    if scene == "review" {
        if !nodes.iter().any(|n| n["name"] == "视频") {
            changes.push(add(
                "demo-video",
                "root",
                "视频",
                &["mp4", "mov", "mkv", "webm"],
                "从其他类独立出视频，保留已有文件内容。",
            ));
            changes.push(add(
                "demo-video-assets",
                "demo-video",
                "剪辑素材",
                &[],
                "校园录像与剪辑片段，不推断未看见的完整内容。",
            ));
        }
        return json!({"message":"【离线演示预设】按你的反馈建立“视频 / 剪辑素材”。采纳后由 Rust 重新计算具体文件位置并更新“整理后”预览；此时尚不移动磁盘文件。","changes":changes});
    }
    if scene == "directories" || scene == "permissions" {
        if let Some(d) = array(&ctx["directories"])
            .iter()
            .find(|d| d["name"] == "课程资料库")
        {
            changes.push(
                json!({"kind":"directory","target":d["index"].to_string(),"after":"container"}),
            );
        }
        return json!({"message":"【离线演示预设】课程资料库可作为复用容器；考试成绩和便携工具目录应保持整体。仅提供建议，需用户采纳。","changes":changes});
    }
    json!({"message":format!("【离线演示预设】当前是 {} 场景。任务状态、工具调用、Token 和模拟费用可在历史查看。这个包使用预设响应，不代表真实模型理解任意指令。你的输入已保存：{}",scene,request.chars().take(100).collect::<String>()),"changes":[]})
}
fn choose<'a>(file: &Value, nodes: &'a [Value]) -> Option<&'a Value> {
    let choices = nodes
        .iter()
        .filter(|n| n["selectable"] == true)
        .collect::<Vec<_>>();
    let name = text(&file["name"]);
    let ext = text(&file["extension"]);
    let preferred = if file["kind"] == "directory" {
        if name.contains("成绩") {
            "文档"
        } else {
            "其他"
        }
    } else if ext == "mp4" {
        "剪辑素材"
    } else if ext == "wav" {
        "编曲素材"
    } else if ["docx", "xlsx", "pptx"].contains(&ext) {
        "课程项目"
    } else if name.starts_with("IMG_") {
        "照片"
    } else if ["jpg", "png"].contains(&ext) {
        "参考素材"
    } else {
        "学习资料"
    };
    choices
        .iter()
        .copied()
        .find(|n| text(&n["name"]).contains(preferred))
        .or_else(|| choices.first().copied())
}
pub fn answer(body: &Value) -> Value {
    let messages = array(&body["messages"]);
    let system = messages
        .first()
        .map(|m| content(&m["content"]))
        .unwrap_or_default();
    let last = messages
        .last()
        .map(|m| content(&m["content"]))
        .unwrap_or_default();
    let ctx = messages
        .get(1)
        .map(|m| context(&m["content"]))
        .unwrap_or(Value::Null);
    let has_tools = array(&body["tools"])
        .iter()
        .any(|t| t["function"]["name"] == "submit_classifications");
    let message = if has_tools {
        let readable = array(&ctx["files"])
            .iter()
            .chain(array(&ctx["examples"]))
            .filter(|f| text(&f["evidence"]) != "none")
            .take(3)
            .map(|f| f["id"].clone())
            .collect::<Vec<_>>();
        let already = messages.iter().any(|m| m["role"] == "tool");
        let (name, args) = if !already && !readable.is_empty() {
            ("read_file_evidence", json!({"file_ids":readable}))
        } else {
            let assignments=array(&ctx["files"]).iter().map(|f|json!({"file_id":f["id"],"node_id":choose(f,array(&ctx["nodes"])).map(|n|n["id"].clone()),"reason":if f["kind"]=="directory"{"演示规则：目录整体归类，内部不拆散"}else{"演示规则：依据预设类别和已返回的有限证据"}})).collect::<Vec<_>>();
            (
                "submit_classifications",
                json!({"batch_id":ctx["batch_id"],"assignments":assignments}),
            )
        };
        json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("demo-{}",uuid::Uuid::new_v4()),"type":"function","function":{"name":name,"arguments":args.to_string()}}]})
    } else {
        let result = if last == "Reply with OK." {
            json!("OK — offline scripted demo")
        } else if system.contains("先查看文件类型统计总览") {
            json!({"summary":"演示桌面包含课程文档、论文、图片、音乐和视频素材。已有成绩与工具文件夹整体保留，优先检查用途相近的文档。","inspect_order":["documents","tables","folders","video","audio","images","text"]})
        } else if system.contains("按类型检查当前批次文件") {
            json!({"summary":format!("【预设摘要】{}：课程文档和成绩资料保持完整；视频为剪辑素材，音频为编曲素材。已提取的内容是局部样本，未进行真实模型推理。",text(&ctx["type_name"]))})
        } else if system.contains("依据现有信息提出清晰简洁文件名") {
            json!({"name":if text(&ctx["name"])=="a8f3d92c.txt"{"Rust Agent 课程答辩提纲.txt"}else{text(&ctx["name"])},"reason":"离线预设：按演示文本中的标题命名，保持扩展名。"})
        } else if system.contains("复核清理候选") {
            json!({"suggestions":array(&context(&messages.last().unwrap()["content"])).iter().map(|c|json!({"index":c["index"],"reason":"演示缓存或空文件。请确认不再需要；仅在勾选确认后移入系统回收站，可撤销。"})).collect::<Vec<_>>()})
        } else {
            let ctx = messages
                .iter()
                .find_map(|m| {
                    content(&m["content"])
                        .strip_prefix("当前场景数据：")
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                })
                .unwrap_or(Value::Null);
            proposal(&ctx, &last)
        };
        json!({"role":"assistant","content":if result.is_string(){text(&result).to_string()}else{result.to_string()}})
    };
    // Simulated accounting makes the normal budget and billing UI demonstrable. No bill exists.
    json!({"id":format!("demo-{}",uuid::Uuid::new_v4()),"choices":[{"finish_reason":if message["tool_calls"].is_array(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":240,"completion_tokens":80}})
}
pub async fn respond(Json(body): Json<Value>) -> Json<Value> {
    tokio::time::sleep(std::time::Duration::from_millis(
        if body["tools"].is_array() { 650 } else { 180 },
    ))
    .await;
    Json(answer(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_round_and_whole_folder_target() {
        let ctx = json!({"batch_id":"b1","files":[{"id":"f1","name":"考试成绩","kind":"directory","evidence":"directory"}],"nodes":[{"id":"n1","name":"其他","selectable":true},{"id":"n2","name":"文档","selectable":true}]});
        let mut body = json!({"tools":[{"function":{"name":"submit_classifications"}}],"messages":[{"role":"system","content":"classify"},{"role":"user","content":ctx.to_string()}]});
        let first = answer(&body);
        assert_eq!(
            first["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "read_file_evidence"
        );
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"tool","content":"{}"}));
        let reply = answer(&body);
        let args: Value = serde_json::from_str(text(
            &reply["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        ))
        .unwrap();
        assert_eq!(args["assignments"][0]["node_id"], "n2");
    }
}
