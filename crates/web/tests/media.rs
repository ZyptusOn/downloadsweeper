mod support;
use base64::Engine;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    path::Path,
    process::{Output, Stdio},
    time::{Duration, UNIX_EPOCH},
};
use support::*;
fn helper(path: &Path, kind: &str, stale: bool, metadata: &fs::Metadata, source: File) -> Output {
    let mut cmd = command(env!("CARGO_BIN_EXE_ds-web"));
    cmd.env_clear();
    if let Some(v) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", v);
    }
    cmd.arg(format!("--native-{kind}-preview"))
        .arg(path)
        .arg(metadata.len().to_string())
        .arg(
            (metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
                + if stale { 1000 } else { 0 })
            .to_string(),
        )
        .stdin(source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(path.parent().unwrap());
    let child = cmd.spawn().unwrap();
    let id = child.id();
    let (send, recv) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(child.wait_with_output());
    });
    // The production helper has bounded work; enforce the former harness deadline too.
    match recv.recv_timeout(Duration::from_secs(12)) {
        Ok(result) => result.unwrap(),
        Err(e) => {
            #[cfg(windows)]
            {
                let _ = command("taskkill")
                    .args(["/PID", &id.to_string(), "/F"])
                    .output();
            }
            #[cfg(unix)]
            {
                let _ = command("kill").args(["-KILL", &id.to_string()]).output();
            }
            panic!("native helper timeout: {e}");
        }
    }
}
fn packets(out: &Output) -> Vec<Value> {
    assert!(
        out.status.success(),
        "helper failed: {:?}; {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.len() <= 3 * 1024 * 1024);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn jpeg(encoded: &str, max: usize) {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert!(bytes.starts_with(b"\xff\xd8") && bytes.len() <= max);
    assert!(image::load_from_memory(&bytes).is_ok());
}

#[test]
#[cfg(any(windows, target_os = "macos"))]
fn native_pdf_progressive_ipc_unicode_dedup_snapshot_and_bounds() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "config.toml", "invalid TOML [");
    write(dir.path(), ".env", "invalid environment fixture");
    for (pages, expected) in [(6, json!([1, 2, 4])), (1, json!([1]))] {
        let path = write(
            dir.path(),
            &format!("页面 样本-{pages}.pdf"),
            media::pdf(pages),
        );
        let before = fs::read(&path).unwrap();
        let meta = fs::metadata(&path).unwrap();
        let out = helper(&path, "pdf", false, &meta, File::open(&path).unwrap());
        let ps = packets(&out);
        let visual = ps.last().unwrap();
        assert_eq!(visual["info"]["sampled_pages"], expected);
        assert_eq!(visual["info"]["partial"], false);
        assert_eq!(
            visual["info"]["backend"],
            if cfg!(windows) {
                "windows_pdf"
            } else {
                "coregraphics_pdf"
            }
        );
        assert_eq!(visual["image"]["high_detail"], true);
        assert!(ps.len() >= array(&expected).len());
        jpeg(text(&visual["image"]["data_base64"]), 384 * 1024);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(
            !helper(&path, "pdf", true, &meta, File::open(&path).unwrap())
                .status
                .success()
        );
    }
    let broken = write(dir.path(), "broken.pdf", b"%PDF-1.4\nnot a valid document");
    assert!(!helper(
        &broken,
        "pdf",
        false,
        &fs::metadata(&broken).unwrap(),
        File::open(&broken).unwrap()
    )
    .status
    .success());
    let oversized = write(dir.path(), "oversized.pdf", b"%PDF-1.4\n");
    File::options()
        .write(true)
        .open(&oversized)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(!helper(
        &oversized,
        "pdf",
        false,
        &fs::metadata(&oversized).unwrap(),
        File::open(&oversized).unwrap()
    )
    .status
    .success());
    #[cfg(target_os = "macos")]
    {
        let path = write(dir.path(), "descriptor.pdf", media::pdf(6));
        let source = File::open(&path).unwrap();
        let meta = source.metadata().unwrap();
        fs::rename(&path, dir.path().join("original.pdf")).unwrap();
        write(dir.path(), "descriptor.pdf", "not authorized");
        assert!(helper(&path, "pdf", false, &meta, source).status.success());
    }
}

#[test]
#[cfg(any(windows, target_os = "macos"))]
fn native_video_without_ffmpeg_uses_unicode_verified_file_and_bounded_jpeg() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "片段 测试.mp4", media::VIDEO);
    write(dir.path(), "config.toml", "invalid TOML [");
    write(dir.path(), ".env", "invalid environment fixture");
    let before = fs::read(&path).unwrap();
    let meta = fs::metadata(&path).unwrap();
    let out = helper(&path, "video", false, &meta, File::open(&path).unwrap());
    let ps = packets(&out);
    let v = ps.last().unwrap();
    assert_eq!(
        v["info"]["backend"],
        if cfg!(windows) {
            "windows_media"
        } else {
            "avfoundation"
        }
    );
    assert_eq!(array(&v["info"]["sample_targets_seconds"]).len(), 3);
    assert_eq!(v["image"]["mime"], "image/jpeg");
    jpeg(text(&v["image"]["data_base64"]), 384 * 1024);
    if cfg!(windows) {
        assert!(out.stderr.is_empty());
    }
    assert!(
        !helper(&path, "video", true, &meta, File::open(&path).unwrap())
            .status
            .success()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    #[cfg(target_os = "macos")]
    {
        let source = File::open(&path).unwrap();
        let original = dir.path().join("original.mp4");
        fs::rename(&path, &original).unwrap();
        write(dir.path(), "片段 测试.mp4", "not authorized");
        assert!(helper(&path, "video", false, &meta, source)
            .status
            .success());
        assert_eq!(fs::read(original).unwrap(), before);
    }
}
async fn rename(
    s: &Server,
    root: &Path,
    protocol: &str,
    tier: &str,
    vision: bool,
) -> (Vec<Value>, Value) {
    s.configure(json!({"api_format":protocol,"multimodal":vision}))
        .await;
    let mut t = s
        .tree(
            root,
            "rename",
            json!({"default":tier,"content_slice_bytes":64,"rules":[]}),
        )
        .await;
    t = s
        .post(
            "rename_scope",
            &t,
            json!({"extensions":["docx","png","mp4","mkv","pdf"],"web_search":false}),
        )
        .await;
    t = s.step(&t).await;
    let count = s.mock.bodies().len();
    t = s.run("rename", &t, json!({})).await;
    let wire = s.mock.wire();
    assert!(wire
        .iter()
        .all(|r| r["headers"].get("authorization").is_none()
            && r["headers"].get("x-api-key").is_none()));
    let bodies = s.mock.bodies()[count..].to_vec();
    assert_eq!(bodies.len(), 4, "previews must not add model requests");
    (bodies, t)
}
async fn protocols(fallback: bool) {
    let s = Server::new("media-fixture").await;
    let root = s.root("files");
    write(&root, "budget.docx", media::office());
    write(&root, "image.png", media::png());
    write(&root, "document.pdf", media::pdf(6));
    if fallback {
        let bin = std::env::var_os("DS_TEST_MEDIA_BIN")
            .expect("Set DS_TEST_MEDIA_BIN to the FFmpeg directory for this optional test");
        let ffmpeg = Path::new(&bin).join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        let result = command(ffmpeg)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=4:duration=6",
                "-c:v",
                "ffv1",
                "-threads",
                "1",
                "-g",
                "1",
            ])
            .arg(root.join("video.mkv"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    } else if cfg!(any(windows, target_os = "macos")) {
        write(&root, "video.mp4", media::VIDEO);
    } else {
        write(&root, "video.mp4", "not a decodable video");
    }
    let before = hashes(&root);
    let native = cfg!(any(windows, target_os = "macos"));
    for protocol in ["chat_completions", "responses", "anthropic"] {
        let (calls, t) = rename(&s, &root, protocol, "content_slice", true).await;
        let mut image_count = 0;
        for b in &calls {
            let turns = if b["messages"].is_array() {
                array(&b["messages"])
            } else {
                array(&b["input"])
            };
            let content = &turns.last().unwrap()["content"];
            let ctx = mock::context(content);
            let images: Vec<_> = content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| {
                    ["image", "input_image", "image_url"]
                        .contains(&b["type"].as_str().unwrap_or(""))
                })
                .collect();
            let name = text(&ctx["name"]);
            if name.ends_with(".docx") {
                assert!(text(&ctx["text_excerpt"]).contains("OFFICE_ALLOWED"));
                assert!(text(&ctx["text_excerpt"]).len() <= 64);
                assert!(images.is_empty());
            } else {
                let expected = if name.ends_with(".png")
                    || ((native || fallback) && !name.ends_with(".pdf"))
                    || (native && name.ends_with(".pdf"))
                {
                    1
                } else {
                    0
                };
                assert_eq!(images.len(), expected, "{protocol}: {ctx}");
                if let Some(block) = images.first() {
                    image_count += 1;
                    let encoded = match protocol {
                        "anthropic" => text(&block["source"]["data"]),
                        "responses" => text(&block["image_url"]).split_once(',').unwrap().1,
                        _ => text(&block["image_url"]["url"]).split_once(',').unwrap().1,
                    };
                    jpeg(
                        encoded,
                        if name.ends_with(".pdf") {
                            384 * 1024
                        } else {
                            256 * 1024
                        },
                    );
                }
                if name.ends_with(".pdf") && native {
                    let p = &ctx["visual_preview"];
                    assert_eq!(p["sampled_pages"], json!([1, 2, 4]));
                    assert_eq!(p["page_count"], 6);
                    assert_eq!(p["partial"], false);
                }
                if (name.ends_with(".mp4") || name.ends_with(".mkv")) && (native || fallback) {
                    let p = &ctx["visual_preview"];
                    assert_eq!(array(&p["sample_targets_seconds"]).len(), 3);
                    assert_eq!(p["layout"], "row_major_2x2");
                    if fallback {
                        assert_eq!(p["backend"], "ffmpeg");
                        assert_eq!(p["sample_targets_seconds"][2].as_f64(), Some(3.));
                    } else {
                        assert_eq!(
                            p["backend"],
                            if cfg!(windows) {
                                "windows_media"
                            } else {
                                "avfoundation"
                            }
                        );
                    }
                }
            }
        }
        assert_eq!(
            image_count,
            1 + usize::from(native || fallback) + usize::from(native)
        );
        assert!(array(&t["pending_calls"]).is_empty() && array(&t["operations"]).is_empty());
        assert!(!json!(calls).to_string().contains("NEVER_EXECUTE_MACRO"));
    }
    for (tier, vision) in [("filename_only", true), ("content_slice", false)] {
        let (calls, _) = rename(&s, &root, "chat_completions", tier, vision).await;
        let wire = json!(calls).to_string();
        assert!(!wire.contains("data:image"));
        if tier == "filename_only" {
            assert!(!wire.contains("OFFICE_ALLOWED"));
        }
    }
    assert_eq!(hashes(&root), before);
    let capabilities = s.get("/api/media-capabilities").await;
    assert_eq!(capabilities["office"], true);
    if native && !fallback {
        assert_eq!(
            capabilities["native_video"],
            if cfg!(windows) {
                "windows_media"
            } else {
                "avfoundation"
            }
        );
        if cfg!(windows) && std::env::var_os("DS_TEST_MEDIA_BIN").is_none() {
            assert_eq!(capabilities["ffmpeg"], false);
            assert_eq!(capabilities["ffprobe"], false);
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn office_image_pdf_video_protocols_and_permissions() {
    protocols(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "optional FFmpeg fallback: requires DS_TEST_MEDIA_BIN; default native tests need no FFmpeg"]
async fn ffmpeg_fallback_protocols() {
    protocols(true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg(any(windows, target_os = "macos"))]
async fn pdf_agent_tool_evidence_mapping_and_permission_gates() {
    let s = Server::new("classification-evidence-fixture").await;
    let root = s.root("files");
    write(&root, "course.pdf", media::pdf(6));
    write(&root, "private.csv", "not authorized");
    for (tier, vision) in [
        ("image", true),
        ("filename_only", true),
        ("content_slice", false),
    ] {
        s.configure(json!({"multimodal":vision})).await;
        let mut t=s.tree(&root,"organize",json!({"default":"none","content_slice_bytes":64,"rules":[{"extensions":["pdf"],"tier":tier}]})).await;
        t=s.post("tree",&t,json!({"nodes":[node("docs",Some("root"),"文档",&["pdf"]),node("course",Some("docs"),"课程资料",&[])]})).await;
        t = s.step(&t).await;
        let start = s.mock.bodies().len();
        t = s.run("plan_ai", &t, json!({})).await;
        let wire = s.mock.bodies();
        let bodies = &wire[start..];
        let serialized = json!(bodies).to_string();
        assert!(!serialized.contains("private.csv"));
        let visual = vision && tier == "image";
        assert_eq!(array(&t["calls"]).len(), if visual { 2 } else { 1 });
        assert_eq!(serialized.contains("data:image/jpeg"), visual);
        if visual {
            let values: Vec<_> = bodies
                .iter()
                .flat_map(|r| array(&r["messages"]))
                .filter(|m| m["role"] == "tool")
                .map(|m| mock::context(&m["content"]))
                .collect();
            assert!(values
                .iter()
                .flat_map(|v| array(&v["files"]))
                .any(|f| f["sampled_pages"] == json!([1, 2, 4]) && f["image_index"] == 1));
            assert!(serialized.contains("pdf_pages"));
        }
        assert_eq!(t["classification"]["completed"], 1);
        assert!(array(&t["pending_calls"]).is_empty());
        assert_eq!(fs::read(root.join("course.pdf")).unwrap(), media::pdf(6));
    }
}
