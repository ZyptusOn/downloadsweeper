//! OS decoding runs in a short-lived copy of our executable, before loading settings.
//! A stuck or crashing codec cannot block the server; no credential environment is inherited.
use super::*;

const HELPER: &str = "--native-video-preview";
const PDF_HELPER: &str = "--native-pdf-preview";
pub(super) fn publish(visual: &Visual) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if serde_json::to_writer(&mut out, visual).is_ok() {
        let _ = writeln!(out);
        let _ = out.flush();
    }
}
pub(super) fn backend() -> Option<&'static str> {
    if cfg!(windows) {
        Some("windows_media")
    } else if cfg!(target_os = "macos") {
        Some("avfoundation")
    } else {
        None
    }
}

/// Call at every application entry point, before starting a runtime or loading .env.
pub fn dispatch_helper() {
    let args: Vec<_> = std::env::args_os().collect();
    if args
        .get(1)
        .is_none_or(|arg| arg != HELPER && arg != PDF_HELPER)
    {
        return;
    }
    let result = (|| -> Result<Visual> {
        ensure!(args.len() == 5, "preview_arguments");
        let path = Path::new(&args[2]);
        ensure!(path.is_absolute(), "preview_path");
        let size: u64 = args[3].to_str().context("size")?.parse()?;
        let pdf = args[1] == PDF_HELPER;
        ensure!(
            !pdf || size <= super::pdf::MAX_FILE_BYTES,
            "pdf_file_size_limit"
        );
        let modified: u64 = args[4].to_str().context("modified")?.parse()?;
        #[cfg(not(target_os = "macos"))]
        let mut file = {
            let root = path.parent().context("parent")?;
            let name = path.file_name().context("filename")?;
            // Recheck the snapshot in the child. The parent's verified handle stays open.
            crate::safe_fs::open_evidence(
                root,
                name.to_str().context("filename_encoding")?,
                size,
                modified,
            )?
        };
        #[cfg(target_os = "macos")]
        let mut file = {
            use std::os::fd::FromRawFd;
            // Stdin is the parent's verified regular file, never a user-supplied URL.
            let fd = unsafe { libc::dup(libc::STDIN_FILENO) };
            ensure!(fd >= 0, "preview_descriptor");
            unsafe { File::from_raw_fd(fd) }
        };
        crate::safe_fs::verify_evidence(&file, size, modified)?;
        let mut header = [0u8; 12];
        file.read_exact(&mut header)?;
        ensure!(
            (pdf && &header[..5] == b"%PDF-")
                || (!pdf
                    && (&header[4..8] == b"ftyp"
                        || &header[..4] == b"\x1a\x45\xdf\xa3"
                        || (&header[..4] == b"RIFF" && &header[8..12] == b"AVI "))),
            "preview_container"
        );
        let visual = if pdf {
            super::pdf::decode(path, &file)?
        } else {
            decode(path, &file)?
        };
        crate::safe_fs::verify_evidence(&file, size, modified)?;
        Ok(visual)
    })();
    let status = match result {
        Ok(visual) => {
            publish(&visual);
            0
        }
        // Errors deliberately contain neither paths nor OS diagnostics in IPC/logs.
        Err(error) => {
            #[cfg(windows)]
            let code = error
                .downcast_ref::<windows::core::Error>()
                .map(|e| e.code().0);
            #[cfg(not(windows))]
            let code: Option<i32> = None;
            // Numeric OS code is useful for diagnosis, without exposing a path or document content.
            let reason = error
                .chain()
                .map(|e| e.to_string())
                .find(|s| {
                    [
                        "pdf_page_image_decode",
                        "pdf_page_size",
                        "pdf_page_render",
                        "pdf_page_output_limit",
                        "pdf_page_short",
                        "pdf_image_limit",
                        "pdf_pages_unavailable",
                        "pdf_encrypted",
                        "preview_container",
                        "pdf_file_size_limit",
                    ]
                    .contains(&s.as_str())
                })
                .unwrap_or_else(|| "native_preview_failed".into());
            let image_error = error.downcast_ref::<image::ImageError>().map(|e| match e {
                image::ImageError::Limits(_) => "image_limits",
                image::ImageError::Unsupported(_) => "image_format_unsupported",
                image::ImageError::Decoding(_) => "image_decode_failed",
                _ => "image_error",
            });
            println!(
                "{}",
                json!({"status":"unavailable","reason":reason,"os_code":code,"image_error":image_error})
            );
            2
        }
    };
    std::process::exit(status);
}

pub(super) async fn preview(
    path: &Path,
    file: &File,
    size: u64,
    modified: u64,
    cancel: &CancellationToken,
) -> Result<Visual> {
    preview_kind(path, file, size, modified, cancel, false).await
}

pub(super) async fn preview_kind(
    path: &Path,
    file: &File,
    size: u64,
    modified: u64,
    cancel: &CancellationToken,
    pdf: bool,
) -> Result<Visual> {
    ensure!(backend().is_some(), "native_decoder_unsupported");
    let mut cmd = tokio::process::Command::new(std::env::current_exe()?);
    super::clean_environment(&mut cmd);
    #[cfg(target_os = "macos")]
    cmd.stdin(std::process::Stdio::from(file.try_clone()?));
    cmd.arg(if pdf { PDF_HELPER } else { HELPER })
        .arg(path)
        .arg(size.to_string())
        .arg(modified.to_string());
    let visual = collect_preview(cmd, cancel, Duration::from_secs(8)).await?;
    crate::safe_fs::verify_evidence(file, size, modified)?;
    ensure!(
        visual.image.as_ref().is_some_and(|i| i.mime == "image/jpeg"
            && i.data_base64.len() <= if pdf { 512 * 1024 } else { 256 * 1024 }),
        "native_preview_invalid"
    );
    Ok(visual)
}

async fn collect_preview(
    mut cmd: tokio::process::Command,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<Visual> {
    // Read complete snapshots as they arrive. Timeout/crash can retain useful earlier pages/frames.
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    let mut child = cmd.spawn()?;
    let cap = 3 * 1024 * 1024;
    let mut stdout = child.stdout.take().context("preview_stdout")?.take(cap + 1);
    let mut bytes = vec![];
    let result = {
        let read = async {
            stdout.read_to_end(&mut bytes).await?;
            ensure!(bytes.len() <= cap as usize, "preview_output_limit");
            ensure!(child.wait().await?.success(), "preview_failed");
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! {biased;
            _=cancel.cancelled()=>Err(anyhow::anyhow!("内容读取已取消")),
            r=tokio::time::timeout(timeout,read)=>r.unwrap_or_else(|_|Err(anyhow::anyhow!("preview_timeout")))
        }
    };
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    ensure!(bytes.len() <= cap as usize, "preview_output_limit");
    let mut visual: Visual = bytes
        .split(|b| *b == b'\n')
        .rev()
        .find_map(|line| serde_json::from_slice::<Visual>(line).ok())
        .context("native_preview_unavailable")?;
    if result.is_err() {
        visual.info["partial"] = json!(true);
        visual.info["stopped_early"] = json!(true);
    }
    Ok(visual)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn decode(_: &Path, _: &File) -> Result<Visual> {
    anyhow::bail!("native_decoder_unsupported")
}

#[cfg(windows)]
fn decode(path: &Path, _: &File) -> Result<Visual> {
    use windows::{
        core::HSTRING,
        Foundation::TimeSpan,
        Media::Editing::{MediaClip, MediaComposition, VideoFramePrecision},
        Storage::{StorageFile, Streams::DataReader},
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    };
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    // StorageFile rejects Rust's canonical \\?\ prefix. Reject components whose
    // meaning could change when interpreted without that prefix (trailing dot/space).
    let path = storage_path(path)?;
    let source = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.get()?;
    let clip = MediaClip::CreateFromFileAsync(&source)?.get()?;
    let props = clip.GetVideoEncodingProperties()?;
    let (width, height) = (props.Width()?, props.Height()?);
    ensure!(
        width > 0 && height > 0 && width <= 8192 && height <= 8192,
        "video_dimensions_limit"
    );
    let duration = clip.OriginalDuration()?.Duration as f64 / 10_000_000.0;
    ensure!(
        duration.is_finite() && duration > 0.0 && duration < 31_536_000.0,
        "video_duration_limit"
    );
    let composition = MediaComposition::new()?;
    composition.Clips()?.Append(&clip)?;
    // Set only the longer dimension so Windows preserves the display aspect ratio.
    let (w, h) = if width >= height { (256, 0) } else { (0, 256) };
    let mut frames = Vec::new();
    let mut sampled = Vec::new();
    for time in super::sample_times(Some(duration)) {
        let frame = (|| -> Result<Vec<u8>> {
            let stream = composition
                .GetThumbnailAsync(
                    TimeSpan {
                        Duration: (time * 10_000_000.0) as i64,
                    },
                    w,
                    h,
                    VideoFramePrecision::NearestKeyFrame,
                )?
                .get()?;
            let size = stream.Size()?;
            ensure!(size > 0 && size <= 256 * 1024, "video_frame_limit");
            let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
            ensure!(
                reader.LoadAsync(size as u32)?.get()? == size as u32,
                "video_frame_short"
            );
            let mut bytes = vec![0; size as usize];
            reader.ReadBytes(&mut bytes)?;
            reader.Close()?;
            stream.Close()?;
            Ok(bytes)
        })();
        if let Ok(bytes) = frame {
            frames.push(bytes);
            sampled.push(time);
            publish(&video_visual(&frames, &sampled, duration)?);
        }
    }
    ensure!(!frames.is_empty(), "video_frames_unavailable");
    video_visual(&frames, &sampled, duration)
}

#[cfg(windows)]
fn video_visual(frames: &[Vec<u8>], sampled: &[f64], duration: f64) -> Result<Visual> {
    let image = super::contact_sheet(frames.to_vec()).context("video_image_invalid")?;
    Ok(Visual {
        image: Some(image),
        info: json!({"status":"sampled","kind":"video_contact_sheet","backend":"windows_media","sample_targets_seconds":sampled,"duration_seconds":duration,"layout":"row_major_2x2","partial":sampled.len()<super::sample_times(Some(duration)).len(),"message":"系统原生采样；左上、右上、左下依次对应实际返回的采样目标。部分帧可能不可用，空白格不是帧；近似定位、无音频，不代表完整视频。"}),
    })
}

#[cfg(target_os = "macos")]
fn decode(_: &Path, file: &File) -> Result<Visual> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn ds_macos_preview(
            fd: std::ffi::c_int,
            output: *mut u8,
            capacity: usize,
            length: *mut usize,
        ) -> std::ffi::c_int;
    }
    let mut output = vec![0u8; 512 * 1024];
    let mut length = 0usize;
    // The synchronous bridge owns all async callback state until completion/timeout;
    // callbacks never access this Rust buffer. No unwinding crosses the C boundary.
    let status = unsafe {
        ds_macos_preview(
            file.as_raw_fd(),
            output.as_mut_ptr(),
            output.len(),
            &mut length,
        )
    };
    ensure!(
        status == 0 && length > 0 && length <= output.len(),
        "native_preview_unavailable"
    );
    Ok(serde_json::from_slice(&output[..length])?)
}

#[cfg(windows)]
pub(super) fn storage_path(path: &Path) -> Result<String> {
    let text = path.to_str().context("video_path_encoding")?;
    let normalized = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_owned()
    };
    ensure!(
        !normalized
            .split(['\\', '/'])
            .any(|part| part.ends_with(['.', ' '])),
        "video_path_ambiguous"
    );
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn storage_paths_keep_unicode_and_reject_ambiguous_names() {
        assert_eq!(
            storage_path(Path::new(r"\\?\D:\片段 测试\视频.mp4")).unwrap(),
            r"D:\片段 测试\视频.mp4"
        );
        assert_eq!(
            storage_path(Path::new(r"\\?\UNC\host\share\video.mp4")).unwrap(),
            r"\\host\share\video.mp4"
        );
        assert!(storage_path(Path::new(r"\\?\D:\video.mp4.")).is_err());
        assert!(storage_path(Path::new(r"\\?\D:\folder \video.mp4")).is_err());
    }
    #[test]
    #[ignore = "subprocess fixture only"]
    fn progressive_child() {
        if std::env::var_os("DS_PROGRESSIVE_CHILD").is_some() {
            println!();
            publish(&Visual {
                image: None,
                info: json!({"sampled_pages":[1]}),
            });
            std::thread::sleep(Duration::from_secs(30));
        }
    }
    fn progressive_command() -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "evidence::native::tests::progressive_child",
            "--ignored",
            "--nocapture",
        ])
        .env("DS_PROGRESSIVE_CHILD", "1");
        cmd
    }
    #[tokio::test]
    async fn completed_snapshot_survives_timeout_but_not_user_cancellation() {
        let started = Instant::now();
        let result = collect_preview(
            progressive_command(),
            &CancellationToken::new(),
            Duration::from_millis(600),
        )
        .await
        .unwrap();
        assert_eq!(result.info["sampled_pages"], json!([1]));
        assert_eq!(result.info["partial"], true);
        assert!(started.elapsed() < Duration::from_secs(3));
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            signal.cancel();
        });
        assert!(
            collect_preview(progressive_command(), &cancel, Duration::from_secs(5))
                .await
                .is_err()
        );
    }
}
