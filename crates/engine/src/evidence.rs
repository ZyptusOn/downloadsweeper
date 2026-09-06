//! Bounded local previews. No document automation, macro execution or full-video upload.
use crate::llm::ImageData;
use anyhow::{ensure, Context, Result};
use quick_xml::{events::Event, Reader};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tokio_util::sync::CancellationToken;

pub const VERSION: u32 = 5;
mod native;
pub mod pdf;
pub use native::dispatch_helper;
const XML_LIMIT: usize = 256 * 1024;
pub async fn local_slot(cancel: &CancellationToken) -> Result<OwnedSemaphorePermit> {
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let slots = SLOTS.get_or_init(|| Arc::new(Semaphore::new(3))).clone();
    tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("内容读取已取消"), p = slots.acquire_owned() => Ok(p?) }
}
pub fn office_supported(ext: &str) -> bool {
    matches!(ext, "docx" | "docm" | "pptx" | "pptm" | "xlsx" | "xlsm")
}
pub fn office_family(ext: &str) -> bool {
    office_supported(ext) || matches!(ext, "doc" | "ppt" | "xls")
}
pub fn video_supported(ext: &str) -> bool {
    matches!(ext, "mp4" | "m4v" | "mkv" | "mov" | "webm" | "avi")
}

fn append(out: &mut String, text: &str, cap: usize) {
    for ch in text.chars() {
        if out.len() + ch.len_utf8() > cap {
            break;
        }
        out.push(ch);
    }
}
fn separator(out: &mut String, cap: usize) {
    if !out.ends_with([' ', '\n', '\t']) {
        append(out, " ", cap);
    }
}
// Check the central-directory budget BEFORE the ZIP library allocates its index.
fn zip_budget(file: &mut File) -> Result<()> {
    let size = file.metadata()?.len();
    ensure!(size >= 22, "invalid_zip");
    let count = size.min(65577) as usize;
    file.seek(SeekFrom::End(-(count as i64)))?;
    let mut tail = vec![0; count];
    file.read_exact(&mut tail)?;
    let pos = (0..=count - 22)
        .rev()
        .find(|&i| {
            tail[i..i + 4] == *b"PK\x05\x06"
                && i + 22 + u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize == count
        })
        .context("invalid_zip")?;
    let u16at = |i| u16::from_le_bytes([tail[pos + i], tail[pos + i + 1]]);
    ensure!(
        pos < 20 || tail[pos - 20..pos - 16] != *b"PK\x06\x07",
        "zip64_not_sampled"
    );
    let u32at = |i| u32::from_le_bytes(tail[pos + i..pos + i + 4].try_into().unwrap());
    ensure!(
        u16at(4) == 0
            && u16at(6) == 0
            && u16at(8) == u16at(10)
            && u16at(10) <= 4096
            && u32at(12) <= 2 * 1024 * 1024
            && u32at(16) != u32::MAX,
        "zip_index_limit"
    );
    ensure!(
        u32at(16) as u64 + u32at(12) as u64 <= size - count as u64 + pos as u64,
        "invalid_zip"
    );
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}
fn member(zip: &mut zip::ZipArchive<&mut File>, name: &str) -> Result<Vec<u8>> {
    let entry = zip.by_name(name)?;
    let mut bytes = Vec::new();
    entry.take(XML_LIMIT as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn xml_text(
    bytes: &[u8],
    cap: usize,
    shared: &[String],
    spreadsheet: bool,
    started: Instant,
) -> Result<Vec<String>> {
    let mut reader = Reader::from_reader(bytes);
    let mut out = String::new();
    let mut strings = vec![];
    let mut text_tag = false;
    let mut shared_cell = false;
    let mut shared_item = false;
    loop {
        ensure!(started.elapsed() < Duration::from_secs(2), "office_timeout");
        match reader.read_event() {
            Ok(Event::DocType(_)) => anyhow::bail!("xml_doctype_not_allowed"),
            Ok(Event::Start(e)) => {
                let local = e.local_name();
                let name = local.as_ref();
                text_tag = matches!(name, b"t" | b"v");
                if name == b"si" {
                    shared_item = true;
                    out.clear();
                }
                if name == b"c" {
                    shared_cell = e
                        .attributes()
                        .flatten()
                        .any(|a| a.key.as_ref() == b"t" && a.value.as_ref() == b"s");
                }
            }
            Ok(Event::Text(e)) if text_tag => {
                let decoded = e.decode()?;
                let decoded = quick_xml::escape::unescape(&decoded)?;
                if spreadsheet && shared_cell {
                    if let Some(value) = decoded.parse::<usize>().ok().and_then(|i| shared.get(i)) {
                        append(&mut out, value, cap);
                    } else {
                        append(&mut out, "[共享文本未采样]", cap);
                    }
                } else {
                    append(&mut out, &decoded, cap);
                }
            }
            Ok(Event::GeneralRef(e)) if text_tag => {
                let reference = format!("&{};", e.decode()?);
                append(&mut out, &quick_xml::escape::unescape(&reference)?, cap);
            }
            Ok(Event::End(e)) => {
                let local = e.local_name();
                let name = local.as_ref();
                if matches!(name, b"t" | b"v") {
                    text_tag = false;
                }
                if matches!(name, b"p" | b"c" | b"row") {
                    separator(&mut out, cap);
                }
                if name == b"si" {
                    strings.push(std::mem::take(&mut out));
                    shared_item = false;
                    if strings.len() >= 8192 {
                        break;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) if bytes.len() == XML_LIMIT => break, // Bounded prefix may end mid-element.
            Err(error) => return Err(error.into()),
            _ => {}
        }
        if !shared_item && strings.is_empty() && out.len() >= cap {
            break;
        }
    }
    if strings.is_empty() {
        strings.push(out);
    }
    Ok(strings)
}
pub fn office_excerpt(file: &mut File, ext: &str, cap: usize) -> Value {
    let cap = cap.min(65536);
    if cap == 0 {
        return json!({"status":"unavailable","reason":"slice_disabled"});
    }
    if !office_supported(ext) {
        return json!({"status":"unavailable","reason":"legacy_office_unsupported","message":"旧版二进制 Office 不做自动转换；请依据名称或另存为 docx/pptx/xlsx。"});
    }
    let result = (|| -> Result<Value> {
        let started = Instant::now();
        zip_budget(file)?;
        let mut zip = zip::ZipArchive::new(file)?;
        ensure!(zip.len() <= 4096, "zip_index_limit");
        let mut names = if matches!(ext, "docx" | "docm") {
            vec!["word/document.xml".to_owned()]
        } else {
            let (prefix, suffix) = if matches!(ext, "pptx" | "pptm") {
                ("ppt/slides/slide", ".xml")
            } else {
                ("xl/worksheets/sheet", ".xml")
            };
            let mut parts = zip
                .file_names()
                .filter_map(|n| {
                    n.strip_prefix(prefix)?
                        .strip_suffix(suffix)?
                        .parse::<u32>()
                        .ok()
                        .map(|i| (i, n.to_owned()))
                })
                .collect::<Vec<_>>();
            parts.sort_by_key(|p| p.0);
            parts
                .into_iter()
                .take(if ext.starts_with("ppt") { 3 } else { 2 })
                .map(|p| p.1)
                .collect()
        };
        let shared =
            if ext.starts_with("xls") && zip.file_names().any(|n| n == "xl/sharedStrings.xml") {
                xml_text(
                    &member(&mut zip, "xl/sharedStrings.xml")?,
                    4096,
                    &[],
                    false,
                    started,
                )?
            } else {
                vec![]
            };
        let mut excerpt = String::new();
        let mut sampled = vec![];
        for name in names.drain(..) {
            if excerpt.len() >= cap {
                break;
            }
            let bytes = member(&mut zip, &name)?;
            let text = xml_text(
                &bytes,
                cap - excerpt.len(),
                &shared,
                ext.starts_with("xls"),
                started,
            )?
            .join(" ");
            append(&mut excerpt, &text, cap);
            separator(&mut excerpt, cap);
            sampled.push(name);
        }
        ensure!(!excerpt.trim().is_empty(), "no_text_in_sample");
        Ok(
            json!({"status":"ok","text_excerpt":excerpt,"bytes":excerpt.len(),"truncated":true,"sampled_parts":sampled,"message":"仅为前部编号片段；不含完整文档、图表或公式计算结果，不能推断未采样内容。"}),
        )
    })();
    result.unwrap_or_else(|_|json!({"status":"unavailable","reason":"office_preview_unavailable","message":"文档加密、损坏、无可用文本或超过轻量提取限制；请依据已有信息。"}))
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Visual {
    pub image: Option<ImageData>,
    pub info: Value,
}
impl Visual {
    pub fn attempted(&self) -> bool {
        !matches!(
            self.info["reason"].as_str(),
            Some(
                "permission_or_vision_disabled"
                    | "model_has_no_vision"
                    | "visual_format_unsupported"
            )
        )
    }
    pub fn unavailable(reason: &str) -> Self {
        Self {
            image: None,
            info: json!({"status":"unavailable","reason":reason}),
        }
    }
}
fn executable(name: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let mut dirs = vec![];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_owned());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path).filter(|p| p.is_absolute()));
    }
    // Finder does not inherit shell startup files; cover standard Homebrew locations.
    #[cfg(target_os = "macos")]
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    dirs.into_iter()
        .map(|p| p.join(&name))
        .find(|p| p.is_file())
}
pub fn capabilities() -> Value {
    json!({"office":true,"office_formats":["docx","docm","pptx","pptm","xlsx","xlsm"],"native_pdf":pdf::backend(),"pdf_pages":3,"pdf_max_bytes":pdf::MAX_FILE_BYTES,"native_video":native::backend(),"ffmpeg":executable("ffmpeg").is_some(),"ffprobe":executable("ffprobe").is_some(),"video_frames":3,"video_timeout_seconds":12,"image_max_px":512,"local_workers":3})
}
#[cfg(test)]
async fn output(
    cmd: tokio::process::Command,
    cap: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    output_timeout(cmd, cap, cancel, Duration::from_secs(4)).await
}
async fn output_timeout(
    mut cmd: tokio::process::Command,
    cap: usize,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<Vec<u8>> {
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let mut child = cmd.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .context("preview_stdout")?
        .take(cap as u64 + 1);
    let read = async {
        let mut bytes = vec![];
        stdout.read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= cap, "preview_output_limit");
        let status = child.wait().await?;
        ensure!(status.success(), "preview_failed");
        Ok::<_, anyhow::Error>(bytes)
    };
    let result = tokio::select! {biased; _ = cancel.cancelled()=>Err(anyhow::anyhow!("内容读取已取消")),r=tokio::time::timeout(timeout,read)=>r.unwrap_or_else(|_|Err(anyhow::anyhow!("preview_timeout")))};
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}
fn input(cmd: &mut tokio::process::Command, path: &Path, file: &File) -> Result<()> {
    // Windows open_evidence denies writes/deletes for the lifetime of this handle.
    #[cfg(windows)]
    {
        let _ = file;
        cmd.arg("-i").arg(path);
    }
    // Use the already verified handle on Unix, never reopen a replaceable pathname.
    #[cfg(unix)]
    {
        let _ = path;
        let mut source = file;
        source.seek(SeekFrom::Start(0))?;
        cmd.stdin(std::process::Stdio::from(file.try_clone()?));
        cmd.args(["-i", "/dev/stdin"]);
    }
    Ok(())
}
fn common(cmd: &mut tokio::process::Command) {
    clean_environment(cmd);
    cmd.args([
        "-v",
        "error",
        "-max_alloc",
        "33554432",
        "-protocol_whitelist",
        "file,pipe",
        "-format_whitelist",
        "mov,matroska,webm,avi",
        "-probesize",
        "1048576",
        "-analyzeduration",
        "500000",
    ]);
}
fn clean_environment(cmd: &mut tokio::process::Command) {
    // Media subprocesses receive no model credentials or proxy authentication.
    cmd.env_clear();
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
}
pub fn sample_times(duration: Option<f64>) -> Vec<f64> {
    if let Some(d) = duration.filter(|d| d.is_finite() && *d > 0.0 && *d < 31_536_000.0) {
        let mut times = vec![0.0, (d * 0.1).min(1.0), d * 0.5];
        times.sort_by(f64::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < 0.25);
        times
    } else {
        vec![0.0]
    }
}
fn contact_sheet(frames: Vec<Vec<u8>>) -> Option<ImageData> {
    let mut sheet = image::RgbImage::from_pixel(512, 512, image::Rgb([245, 245, 245]));
    for (i, bytes) in frames.into_iter().take(3).enumerate() {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(512);
        limits.max_image_height = Some(512);
        limits.max_alloc = Some(16 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().ok()?.thumbnail(256, 256).to_rgb8();
        image::imageops::replace(
            &mut sheet,
            &image,
            ((i % 2) * 256) as i64 + (256 - image.width()) as i64 / 2,
            ((i / 2) * 256) as i64 + (256 - image.height()) as i64 / 2,
        );
    }
    crate::metadata::jpeg_data(&image::DynamicImage::ImageRgb8(sheet), 65)
}
pub async fn video(
    path: &Path,
    file: File,
    size: u64,
    modified: u64,
    cancel: &CancellationToken,
) -> Result<Visual> {
    let overall = Instant::now();
    let remaining = || Duration::from_secs(12).saturating_sub(overall.elapsed());
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    crate::safe_fs::verify_evidence(&file, size, modified)?;
    let native_result = native::preview(path, &file, size, modified, cancel).await;
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    crate::safe_fs::verify_evidence(&file, size, modified)?;
    if let Ok(mut preview) = native_result {
        if preview.image.is_some() {
            preview.info["elapsed_ms"] = json!(overall.elapsed().as_millis());
            return Ok(preview);
        }
    }
    let Some(ffmpeg) = executable("ffmpeg") else {
        return Ok(Visual {image:None,info:json!({"status":"unavailable","reason":"video_native_failed_no_fallback","elapsed_ms":overall.elapsed().as_millis(),"message":"系统解码未返回画面且未安装 FFmpeg 后备；可能是格式、编码器或超时限制。请使用已有信息，不要反复读取。"})});
    };
    let mut duration = None;
    if let Some(ffprobe) = executable("ffprobe") {
        let mut cmd = tokio::process::Command::new(ffprobe);
        common(&mut cmd);
        cmd.args([
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=duration:stream=width,height",
            "-of",
            "json",
        ]);
        input(&mut cmd, path, &file)?;
        if let Ok(bytes) = output_timeout(cmd, 8192, cancel,remaining().min(Duration::from_secs(2))).await {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                if value["streams"].as_array().is_some_and(|s| {
                    s.iter().any(|s| {
                        s["width"].as_u64().unwrap_or(0) > 8192
                            || s["height"].as_u64().unwrap_or(0) > 8192
                    })
                }) {
                    return Ok(Visual::unavailable("video_dimensions_limit"));
                }
                duration = value["format"]["duration"]
                    .as_str()
                    .and_then(|s| s.parse().ok());
            }
        }
    }
    let mut frames = vec![];
    let mut sampled = vec![];
    for time in sample_times(duration) {
        ensure!(!cancel.is_cancelled(), "内容读取已取消");
        if remaining() < Duration::from_millis(100) {
            break;
        }
        let mut cmd = tokio::process::Command::new(&ffmpeg);
        common(&mut cmd);
        cmd.args([
            "-nostdin",
            "-threads",
            "1",
            "-ss",
            &format!("{time:.3}"),
            "-noaccurate_seek",
        ]);
        input(&mut cmd, path, &file)?;
        cmd.args([
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-frames:v",
            "1",
            "-filter_threads",
            "1",
            "-vf",
            "scale=256:256:force_original_aspect_ratio=decrease",
            "-threads",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "mjpeg",
            "-q:v",
            "5",
            "pipe:1",
        ]);
        if let Ok(bytes) = output_timeout(cmd, 256 * 1024, cancel,remaining().min(Duration::from_secs(3))).await {
            if !bytes.is_empty() {
                frames.push(bytes);
                sampled.push(time);
            }
        }
    }
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    crate::safe_fs::verify_evidence(&file, size, modified)?;
    if frames.is_empty() {
        return Ok(Visual {image:None,info:json!({"status":"unavailable","reason":"video_decode_unavailable","elapsed_ms":overall.elapsed().as_millis(),"message":"原生与后备解码均未取得可用帧；可能是损坏、编码器不支持或达到总等待上限。"})});
    }
    let image = tokio::task::spawn_blocking(move || contact_sheet(frames)).await?;
    Ok(Visual {
        image,
        info: json!({"status":"sampled","kind":"video_contact_sheet","backend":"ffmpeg","sample_targets_seconds":sampled,"duration_seconds":duration,"elapsed_ms":overall.elapsed().as_millis(),"partial":sampled.len()<sample_times(duration).len(),"layout":"row_major_2x2","message":"从左到右、从上到下对应实际返回的采样目标，空白格不是视频帧。快速定位时间为近似值；无音频，不代表完整视频。"}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "subprocess fixture only"]
    fn preview_child() {
        if std::env::var_os("DS_PREVIEW_CHILD").is_some() {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    #[tokio::test]
    async fn preview_process_is_bounded_and_cancelled() {
        let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
        cmd.arg("--help");
        assert!(output(cmd, 16, &CancellationToken::new()).await.is_err());
        let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "evidence::tests::preview_child",
            "--ignored",
            "--nocapture",
        ])
        .env("DS_PREVIEW_CHILD", "1");
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            signal.cancel();
        });
        let started = Instant::now();
        assert!(output(cmd, 8192, &cancel).await.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[tokio::test]
    async fn stalled_codec_is_terminated_on_deadline() {
        let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "evidence::tests::preview_child",
            "--ignored",
            "--nocapture",
        ])
        .env("DS_PREVIEW_CHILD", "1");
        let started = Instant::now();
        let result = output_timeout(
            cmd,
            8192,
            &CancellationToken::new(),
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(result.unwrap_err().to_string(), "preview_timeout");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn contact_sheet_rejects_excessive_dimensions() {
        let frame = image::DynamicImage::ImageRgb8(image::RgbImage::new(513, 1));
        let mut bytes = std::io::Cursor::new(Vec::new());
        frame.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        assert!(contact_sheet(vec![bytes.into_inner()]).is_none());
    }
    #[tokio::test]
    async fn local_workers_are_parallel_but_bounded() {
        use futures::{stream, StreamExt};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let jobs = stream::iter(0..9)
            .map(|_| {
                let (active, peak, cancel) = (active.clone(), peak.clone(), cancel.clone());
                async move {
                    let permit = local_slot(&cancel).await.unwrap();
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(n, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(30));
                        active.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await
                    .unwrap();
                }
            })
            .buffer_unordered(9);
        jobs.collect::<Vec<_>>().await;
        assert_eq!(peak.load(Ordering::SeqCst), 3);
        cancel.cancel();
        assert!(local_slot(&cancel).await.is_err());
    }
}
