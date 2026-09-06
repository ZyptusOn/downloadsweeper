//! Versioned recovery fingerprints. Never change the meaning of an existing tag.
use super::is_link;
use crate::workflow::Progress;
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

// Versioned thresholds: existing v1 records must keep their original 16 MiB rule.
const FULL_LIMIT_V1: u64 = 16 * 1024 * 1024;
const FULL_LIMIT: u64 = 1024 * 1024;
const SAMPLE_BYTES: u64 = 64 * 1024;
const SAMPLE_COUNT: u64 = 5;
const ADAPTIVE_TAG_V1: &str = "blake3-adaptive-v1";
const ADAPTIVE_TAG: &str = "blake3-adaptive-v2";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Blake3,
    Sha256,
    AdaptiveV1,
    AdaptiveV2,
}
impl Algorithm {
    fn label(self) -> &'static str {
        match self {
            Self::Blake3 => "BLAKE3 全量",
            Self::Sha256 => "SHA-256 全量（兼容旧记录）",
            Self::AdaptiveV1 => "BLAKE3 分段（16 MiB 旧规则）",
            Self::AdaptiveV2 => "BLAKE3 分段（1 MiB 规则）",
        }
    }

    fn sampled(self, size: u64) -> bool {
        match self {
            Self::AdaptiveV1 => size > FULL_LIMIT_V1,
            Self::AdaptiveV2 => size > FULL_LIMIT,
            _ => false,
        }
    }

    fn adaptive_domain(self) -> Option<&'static [u8]> {
        match self {
            Self::AdaptiveV1 => Some(b"DownloadSweeper adaptive fingerprint v1\0"),
            Self::AdaptiveV2 => Some(b"DownloadSweeper adaptive fingerprint v2\0"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct HashProgress {
    pub bytes_read: u64,
    pub bytes_total: u64,
    algorithm: Algorithm,
}

enum Hasher {
    Blake3(Box<blake3::Hasher>),
    Sha256(Sha256),
}

impl Hasher {
    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Blake3(hash) => {
                hash.update(bytes);
            }
            Self::Sha256(hash) => hash.update(bytes),
        }
    }

    fn finish(self) -> String {
        match self {
            Self::Blake3(hash) => hash.finalize().to_hex().to_string(),
            Self::Sha256(hash) => format!("{:x}", hash.finalize()),
        }
    }
}

fn stored_algorithm(expected: &str) -> Result<(Algorithm, &str)> {
    let (algorithm, digest) = expected.split_once(':').unwrap_or(("sha256", expected));
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "恢复指纹格式无效，已停止操作"
    );
    let algorithm = match algorithm {
        "blake3" => Algorithm::Blake3,
        "sha256" => Algorithm::Sha256,
        ADAPTIVE_TAG_V1 => Algorithm::AdaptiveV1,
        ADAPTIVE_TAG => Algorithm::AdaptiveV2,
        _ => anyhow::bail!("不支持的恢复指纹算法：{algorithm}，请使用支持此记录的版本"),
    };
    Ok((algorithm, digest))
}

struct Input {
    path: PathBuf,
    relative: Option<String>,
    metadata: Metadata,
}

fn calculate(
    path: &Path,
    algorithm: Algorithm,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(HashProgress),
) -> Result<String> {
    ensure!(!cancel.is_cancelled(), "任务已取消");
    ensure!(!is_link(path), "不能处理符号链接");
    let mut inputs = Vec::new();
    if path.is_dir() {
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry?;
            ensure!(!cancel.is_cancelled(), "任务已取消");
            ensure!(!is_link(entry.path()), "整体目录内存在链接，不能安全移动");
            inputs.push(Input {
                relative: Some(
                    entry
                        .path()
                        .strip_prefix(path)?
                        .to_string_lossy()
                        .into_owned(),
                ),
                metadata: entry.metadata()?,
                path: entry.into_path(),
            });
        }
    } else {
        inputs.push(Input {
            path: path.into(),
            relative: None,
            metadata: path.metadata()?,
        });
    }
    let mut state = HashProgress {
        bytes_read: 0,
        bytes_total: 0,
        algorithm,
    };
    for input in &inputs {
        ensure!(
            input.metadata.is_file() || input.metadata.is_dir(),
            "只能校验普通文件或目录"
        );
        if input.metadata.is_file() {
            let size = input.metadata.len();
            let read_size = if algorithm.sampled(size) {
                SAMPLE_BYTES * SAMPLE_COUNT
            } else {
                size
            };
            state.bytes_total = state
                .bytes_total
                .checked_add(read_size)
                .context("文件总大小溢出")?;
        }
    }
    let mut hash = match algorithm {
        Algorithm::Sha256 => Hasher::Sha256(Sha256::new()),
        _ => Hasher::Blake3(Box::new(blake3::Hasher::new())),
    };
    if let Some(domain) = algorithm.adaptive_domain() {
        hash.update(domain);
        hash.update(&[u8::from(path.is_dir())]);
    }
    // Reuse one buffer; samples only read the first 64 KiB of it.
    let mut buffer = vec![0u8; 1024 * 1024];
    progress(state);
    for input in inputs {
        ensure!(!cancel.is_cancelled(), "任务已取消");
        ensure!(!is_link(&input.path), "文件已被替换为链接，已停止校验");
        if let Some(rel) = &input.relative {
            // Preserve the exact legacy encoding for SHA-256 and full BLAKE3 directories.
            hash.update(&(rel.len() as u64).to_le_bytes());
            hash.update(rel.as_bytes());
            hash.update(&[u8::from(input.metadata.is_dir())]);
            if input.metadata.is_file() {
                hash.update(&input.metadata.len().to_le_bytes());
            }
        }
        if !input.metadata.is_file() {
            continue;
        }
        let mut file = File::open(&input.path)?;
        let meta = file.metadata()?;
        let size = meta.len();
        ensure!(
            meta.is_file() && size == input.metadata.len(),
            "文件大小已变化，已停止校验"
        );
        let sampled = algorithm.sampled(size);
        if algorithm.adaptive_domain().is_some() {
            hash.update(&[u8::from(sampled)]);
            hash.update(&size.to_le_bytes());
        }
        if sampled {
            let modified = meta
                .modified()
                .context("无法获取大文件修改时间，已停止抽样校验")?;
            let (before_epoch, time) = match modified.duration_since(UNIX_EPOCH) {
                Ok(time) => (false, time),
                Err(error) => (true, error.duration()),
            };
            hash.update(&[u8::from(before_epoch)]);
            hash.update(&time.as_secs().to_le_bytes());
            hash.update(&time.subsec_nanos().to_le_bytes());
            for index in 0..SAMPLE_COUNT {
                ensure!(!cancel.is_cancelled(), "任务已取消");
                let offset = ((size - SAMPLE_BYTES) as u128 * index as u128
                    / (SAMPLE_COUNT - 1) as u128) as u64;
                hash.update(&offset.to_le_bytes());
                hash.update(&SAMPLE_BYTES.to_le_bytes());
                file.seek(SeekFrom::Start(offset))?;
                file.read_exact(&mut buffer[..SAMPLE_BYTES as usize])?;
                hash.update(&buffer[..SAMPLE_BYTES as usize]);
                state.bytes_read += SAMPLE_BYTES;
                progress(state);
            }
            let after = file.metadata()?;
            ensure!(
                after.len() == size && after.modified()? == modified,
                "文件在抽样期间发生变化，已停止校验"
            );
        } else {
            loop {
                ensure!(!cancel.is_cancelled(), "任务已取消");
                let count = match file.read(&mut buffer) {
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    result => result?,
                };
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
                state.bytes_read += count as u64;
                progress(state);
            }
        }
    }
    ensure!(!cancel.is_cancelled(), "任务已取消");
    progress(state);
    Ok(hash.finish())
}

/// Small files use full BLAKE3. Files > 1 MiB and directories use the v2 policy.
pub fn fingerprint(path: &Path, cancel: &CancellationToken) -> Result<String> {
    fingerprint_with_progress(path, cancel, &mut |_| {})
}

pub(super) fn fingerprint_with_progress(
    path: &Path,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(HashProgress),
) -> Result<String> {
    let meta = path.metadata()?;
    let (algorithm, tag) = if meta.is_dir() || meta.len() > FULL_LIMIT {
        (Algorithm::AdaptiveV2, ADAPTIVE_TAG)
    } else {
        (Algorithm::Blake3, "blake3")
    };
    let digest = calculate(path, algorithm, cancel, progress)?;
    Ok(format!("{tag}:{digest}"))
}

/// Match using the stored algorithm, never reinterpret a SHA-256 digest as BLAKE3.
pub fn fingerprint_matches(
    path: &Path,
    expected: &str,
    cancel: &CancellationToken,
) -> Result<bool> {
    matches_with_progress(path, expected, cancel, &mut |_| {})
}

pub(super) fn matches_with_progress(
    path: &Path,
    expected: &str,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(HashProgress),
) -> Result<bool> {
    let (algorithm, digest) = stored_algorithm(expected)?;
    Ok(calculate(path, algorithm, cancel, progress)?.eq_ignore_ascii_case(digest))
}

pub(super) fn reporter<'a>(
    progress: &'a Progress<'_>,
    done: usize,
    total: usize,
    name: &'a str,
) -> impl FnMut(HashProgress) + 'a {
    let mut last: Option<Instant> = None;
    move |state| {
        if last.is_none_or(|t| t.elapsed() >= Duration::from_millis(250))
            || state.bytes_read >= state.bytes_total
        {
            progress(
                done,
                total,
                &format!(
                    "正在计算 {}指纹 · {name} · 已读取 {:.2} / {:.2} MiB",
                    state.algorithm.label(),
                    state.bytes_read as f64 / 1048576.0,
                    state.bytes_total as f64 / 1048576.0
                ),
            );
            last = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Seek, SeekFrom, Write},
        path::PathBuf,
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("ds-full-hash-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn tagged_blake3_and_legacy_sha256_use_their_own_algorithms() {
        let dir = Fixture::new();
        let path = dir.0.join("abc.txt");
        fs::write(&path, b"abc").unwrap();
        let cancel = CancellationToken::new();
        let blake3 = "blake3:6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";
        let legacy = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(fingerprint(&path, &cancel).unwrap(), blake3);
        for expected in [
            blake3.to_owned(),
            legacy.to_owned(),
            format!("sha256:{legacy}"),
        ] {
            assert!(fingerprint_matches(&path, &expected, &cancel).unwrap());
        }
        assert!(!fingerprint_matches(&path, &format!("blake3:{legacy}"), &cancel).unwrap());
        for bad in [
            "md5:abc",
            "blake3:",
            "unknown:0000000000000000000000000000000000000000000000000000000000000000",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(fingerprint_matches(&path, bad, &cancel).is_err());
        }
    }

    #[test]
    fn full_fingerprint_detects_same_size_middle_edits_and_can_cancel_between_chunks() {
        let dir = Fixture::new();
        let path = dir.0.join("large.bin");
        fs::write(&path, vec![42u8; 1024 * 1024]).unwrap();
        let cancel = CancellationToken::new();
        let mut progress = Vec::new();
        let original =
            fingerprint_with_progress(&path, &cancel, &mut |p| progress.push(p.bytes_read))
                .unwrap();
        assert_eq!(progress[0], 0);
        assert_eq!(progress.last(), Some(&(1024 * 1024)));
        assert!(progress.windows(2).all(|w| w[1] >= w[0]));
        assert!(progress.contains(&(1024 * 1024)));
        let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(512 * 1024 + 123)).unwrap();
        file.write_all(&[99]).unwrap();
        drop(file);
        assert!(!fingerprint_matches(&path, &original, &cancel).unwrap());
        let mut last_read = 0;
        let result = fingerprint_with_progress(&path, &cancel, &mut |p| {
            last_read = p.bytes_read;
            if p.bytes_read > 0 {
                cancel.cancel();
            }
        });
        assert!(result.unwrap_err().to_string().contains("取消"));
        assert_eq!(last_read, 1024 * 1024);
    }

    #[test]
    fn threshold_is_strict_and_sample_io_is_bounded() {
        let dir = Fixture::new();
        let path = dir.0.join("large.bin");
        let file = File::create(&path).unwrap();
        let cancel = CancellationToken::new();
        for size in [
            FULL_LIMIT - 1,
            FULL_LIMIT,
            FULL_LIMIT + 1,
            1024 * 1024 * 1024,
        ] {
            file.set_len(size).unwrap();
            let mut reports = Vec::new();
            let digest =
                fingerprint_with_progress(&path, &cancel, &mut |p| reports.push(p)).unwrap();
            let expected = if size > FULL_LIMIT {
                SAMPLE_COUNT * SAMPLE_BYTES
            } else {
                size
            };
            assert_eq!(reports[0].bytes_read, 0);
            assert_eq!(reports.last().unwrap().bytes_read, expected);
            assert!(reports.iter().all(|p| p.bytes_total == expected));
            assert_eq!(digest.starts_with(ADAPTIVE_TAG), size > FULL_LIMIT);
            assert!(fingerprint_matches(&path, &digest, &cancel).unwrap());
        }
    }

    #[test]
    fn v1_directory_records_keep_full_checks_between_one_and_sixteen_mib() {
        let dir = Fixture::new();
        let path = dir.0.join("data.bin");
        let mut file = File::create(&path).unwrap();
        file.set_len(2 * 1024 * 1024).unwrap();
        let cancel = CancellationToken::new();
        let mut legacy_read = 0;
        let legacy = format!(
            "{ADAPTIVE_TAG_V1}:{}",
            calculate(&dir.0, Algorithm::AdaptiveV1, &cancel, &mut |p| {
                legacy_read = p.bytes_read
            })
            .unwrap()
        );
        assert_eq!(legacy_read, 2 * 1024 * 1024);
        let mut current_read = 0;
        let current =
            fingerprint_with_progress(&dir.0, &cancel, &mut |p| current_read = p.bytes_read)
                .unwrap();
        assert!(current.starts_with("blake3-adaptive-v2:"));
        assert_eq!(current_read, SAMPLE_BYTES * SAMPLE_COUNT);
        assert!(fingerprint_matches(&dir.0, &legacy, &cancel).unwrap());
        assert!(fingerprint_matches(&dir.0, &current, &cancel).unwrap());
        // Preserve the timestamp to distinguish full verification from sampled verification.
        let modified = file.metadata().unwrap().modified().unwrap();
        file.seek(SeekFrom::Start(SAMPLE_BYTES + 17)).unwrap();
        file.write_all(&[99]).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert!(!fingerprint_matches(&dir.0, &legacy, &cancel).unwrap());
        assert!(fingerprint_matches(&dir.0, &current, &cancel).unwrap());
        assert!(!Algorithm::AdaptiveV1.sampled(FULL_LIMIT_V1));
        assert!(Algorithm::AdaptiveV1.sampled(FULL_LIMIT_V1 + 1));
    }

    #[test]
    fn samples_cover_five_positions_and_metadata_but_not_untouched_gaps() {
        use std::fs::FileTimes;
        let dir = Fixture::new();
        let path = dir.0.join("large.bin");
        let mut file = File::create(&path).unwrap();
        let size = 24 * 1024 * 1024;
        file.set_len(size).unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        file.set_times(FileTimes::new().set_modified(modified))
            .unwrap();
        let cancel = CancellationToken::new();
        let original = fingerprint(&path, &cancel).unwrap();
        for index in 0..SAMPLE_COUNT {
            let offset = (size - SAMPLE_BYTES) * index / (SAMPLE_COUNT - 1);
            file.seek(SeekFrom::Start(offset + 7)).unwrap();
            file.write_all(&[99]).unwrap();
            file.set_times(FileTimes::new().set_modified(modified))
                .unwrap();
            assert!(!fingerprint_matches(&path, &original, &cancel).unwrap());
            file.seek(SeekFrom::Start(offset + 7)).unwrap();
            file.write_all(&[0]).unwrap();
            file.set_times(FileTimes::new().set_modified(modified))
                .unwrap();
            assert!(fingerprint_matches(&path, &original, &cancel).unwrap());
        }
        // A preserved timestamp plus an edit outside the windows is an explicit limitation.
        let legacy_full = format!(
            "blake3:{}",
            calculate(&path, Algorithm::Blake3, &cancel, &mut |_| {}).unwrap()
        );
        file.seek(SeekFrom::Start(SAMPLE_BYTES + 17)).unwrap();
        file.write_all(&[99]).unwrap();
        file.set_times(FileTimes::new().set_modified(modified))
            .unwrap();
        assert!(fingerprint_matches(&path, &original, &cancel).unwrap());
        assert!(!fingerprint_matches(&path, &legacy_full, &cancel).unwrap());
        file.set_times(FileTimes::new().set_modified(modified + Duration::from_secs(1)))
            .unwrap();
        assert!(!fingerprint_matches(&path, &original, &cancel).unwrap());
        file.set_times(FileTimes::new().set_modified(modified))
            .unwrap();
        file.set_len(size + 1).unwrap();
        file.set_times(FileTimes::new().set_modified(modified))
            .unwrap();
        assert!(!fingerprint_matches(&path, &original, &cancel).unwrap());
    }

    #[test]
    fn adaptive_directory_progress_and_sample_cancellation_are_accurate() {
        let dir = Fixture::new();
        let path = dir.0.join("large.bin");
        File::create(&path)
            .unwrap()
            .set_len(FULL_LIMIT + 1)
            .unwrap();
        fs::write(dir.0.join("small.txt"), b"small file").unwrap();
        let cancel = CancellationToken::new();
        let mut reports = Vec::new();
        let digest = fingerprint_with_progress(&dir.0, &cancel, &mut |p| reports.push(p)).unwrap();
        assert_eq!(
            reports.last().unwrap().bytes_read,
            SAMPLE_BYTES * SAMPLE_COUNT + 10
        );
        assert!(reports
            .iter()
            .all(|p| p.bytes_total == SAMPLE_BYTES * SAMPLE_COUNT + 10));
        let moved = dir.0.with_extension("moved");
        fs::rename(&dir.0, &moved).unwrap();
        assert!(fingerprint_matches(&moved, &digest, &cancel).unwrap());
        fs::rename(&moved, &dir.0).unwrap();
        let mut last_read = 0;
        let result = fingerprint_with_progress(&path, &cancel, &mut |p| {
            last_read = p.bytes_read;
            if p.bytes_read > 0 {
                cancel.cancel();
            }
        });
        assert!(result.unwrap_err().to_string().contains("取消"));
        assert_eq!(last_read, SAMPLE_BYTES);
    }

    #[test]
    fn legacy_directory_encoding_and_new_structure_checks_are_preserved() {
        let dir = Fixture::new();
        fs::create_dir(dir.0.join("empty")).unwrap();
        fs::create_dir(dir.0.join("nested")).unwrap();
        fs::write(dir.0.join("nested/说明.txt"), b"old task\n").unwrap();
        fs::write(dir.0.join("z.bin"), [0, 1, 255]).unwrap();
        // Fixed vectors from the original SHA-256 directory serialization.
        let legacy = if cfg!(windows) {
            "1a1ab136cc4aee850736ace2c262ec7b91957f616e08f0199d3014a268874d1f"
        } else {
            "dec87c4bf005194ede40f829f1981de04d8eb410cfdb88fbd56ca2ff5c036a47"
        };
        let cancel = CancellationToken::new();
        assert!(fingerprint_matches(&dir.0, legacy, &cancel).unwrap());
        let legacy_full = format!(
            "blake3:{}",
            calculate(&dir.0, Algorithm::Blake3, &cancel, &mut |_| {}).unwrap()
        );
        assert!(fingerprint_matches(&dir.0, &legacy_full, &cancel).unwrap());
        let original = fingerprint(&dir.0, &cancel).unwrap();
        fs::rename(dir.0.join("empty"), dir.0.join("renamed")).unwrap();
        assert!(!fingerprint_matches(&dir.0, &original, &cancel).unwrap());
        assert!(!fingerprint_matches(&dir.0, legacy, &cancel).unwrap());
        fs::rename(dir.0.join("renamed"), dir.0.join("empty")).unwrap();
        assert!(fingerprint_matches(&dir.0, &original, &cancel).unwrap());
        fs::write(dir.0.join("nested/说明.txt"), b"new task\n").unwrap();
        assert!(!fingerprint_matches(&dir.0, &original, &cancel).unwrap());
    }

    #[test]
    fn empty_files_and_directories_remain_distinct() {
        let dir = Fixture::new();
        let file = dir.0.join("empty-file");
        let folder = dir.0.join("empty-dir");
        fs::write(&file, b"").unwrap();
        fs::create_dir(&folder).unwrap();
        let cancel = CancellationToken::new();
        let digest = fingerprint(&file, &cancel).unwrap();
        assert_eq!(digest, format!("blake3:{}", blake3::hash(b"")));
        assert_ne!(digest, fingerprint(&folder, &cancel).unwrap());
    }
}
