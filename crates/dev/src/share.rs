//! Offline source/index/portable credential verification. Findings never contain secret values.
use anyhow::{bail, ensure, Context, Result};
use regex::{bytes::Regex as BytesRegex, Regex};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::LazyLock,
};
static PATTERNS: LazyLock<Vec<(&str, BytesRegex)>> = LazyLock::new(|| {
    vec![
        (
            "provider API key",
            BytesRegex::new(r"\bsk-[A-Za-z0-9_-]{20,}").unwrap(),
        ),
        (
            "GitHub token",
            BytesRegex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})")
                .unwrap(),
        ),
        (
            "AWS access key",
            BytesRegex::new(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b").unwrap(),
        ),
        (
            "private key",
            BytesRegex::new(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----").unwrap(),
        ),
    ]
});
static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:api[_-]?key|token|secret|password)$").unwrap());
static ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ix)(?:["']?\b(?:api[_-]?key|[a-z][a-z0-9_]*_api_key|access_token|client_secret|password)["']?)\s*[:=]\s*(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^\s,\#}]+))"#).unwrap()
});
static LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_./+\-=]+$").unwrap());
static ENV_ASSIGN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:export\s+)?[A-Z][A-Z0-9_]*_API_KEY\s*=").unwrap());
fn safe(v: &str) -> bool {
    ["", "fixture-search-key", "YOUR_API_KEY", "your-api-key"].contains(&v.trim())
}
pub fn private_path(path: &Path) -> bool {
    let raw = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let parts: Vec<_> = raw.split('/').collect();
    let last = parts.last().copied().unwrap_or("");
    parts.iter().any(|p| {
        [
            ".git",
            ".ds-data",
            "artifacts",
            "target",
            "dist",
            "__pycache__",
            "node_modules",
            ".venv",
            "venv",
            ".pytest_cache",
            ".mypy_cache",
            ".ruff_cache",
            "%systemdrive%",
        ]
        .contains(p)
            || p.starts_with(".sync-")
            || ((*p == ".env" || p.starts_with(".env.")) && *p != ".env.example")
    }) || [
        "config.toml",
        "dir_class_overrides.json",
        "classification_cache.json",
        "trajectory.jsonl",
        ".ds_store",
        "thumbs.db",
        "desktop.ini",
        ".ds-workspace.json",
    ]
    .contains(&last)
        || parts.windows(2).any(|p| p == ["src-tauri", "gen"])
        || (last.starts_with("session-") && last.ends_with(".json"))
}
pub fn linked(path: &Path) -> bool {
    let Ok(m) = fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    m.file_type().is_symlink()
}
pub fn known_credentials(root: &Path, env: &HashMap<String, String>) -> Result<BTreeSet<Vec<u8>>> {
    let mut values: Vec<String> = env
        .iter()
        .filter(|(k, _)| KEY.is_match(k))
        .map(|(_, v)| v.clone())
        .collect();
    let mut custom = BTreeSet::new();
    for name in ["config.toml", "config.example.toml"] {
        let p = root.join(name);
        if p.is_file() {
            let data = fs::read_to_string(p)?;
            let c: toml::Value = toml::from_str(data.trim_start_matches('\u{feff}'))?;
            for section in ["llm", "search"] {
                if let Some(s) = c.get(section) {
                    if let Some(v) = s.get("api_key").and_then(toml::Value::as_str) {
                        values.push(v.into());
                    }
                    if let Some(n) = s.get("api_key_env").and_then(toml::Value::as_str) {
                        custom.insert(n.to_owned());
                        if let Some(v) = env.get(n) {
                            values.push(v.clone());
                        }
                    }
                }
            }
        }
    }
    for entry in fs::read_dir(root)? {
        let p = entry?.path();
        let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
        if p.is_file() && name.starts_with(".env") && name != ".env.example" {
            for line in fs::read_to_string(p)?
                .trim_start_matches('\u{feff}')
                .lines()
            {
                if line.trim_start().starts_with('#') {
                    continue;
                }
                if let Some((name, value)) = line.split_once('=') {
                    let name = name.trim();
                    if KEY.is_match(name) || name.starts_with("DS_") || custom.contains(name) {
                        values.push(value.trim().trim_matches(['\'', '"']).into());
                    }
                }
            }
        }
    }
    Ok(values
        .into_iter()
        .filter(|v| !safe(v) && v.len() >= 8)
        .map(String::into_bytes)
        .collect())
}
pub struct Checker {
    secrets: BTreeSet<Vec<u8>>,
    pub failures: Vec<String>,
    pub files: usize,
}
impl Checker {
    pub fn new(secrets: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            secrets: secrets.into_iter().collect(),
            failures: vec![],
            files: 0,
        }
    }
    pub fn fail(&mut self, location: &str, rule: &str) {
        let mut label = location.to_owned();
        for secret in &self.secrets {
            label = label.replace(String::from_utf8_lossy(secret).as_ref(), "[REDACTED]");
        }
        for (_, p) in PATTERNS.iter() {
            label =
                String::from_utf8_lossy(&p.replace_all(label.as_bytes(), b"[REDACTED]".as_slice()))
                    .into_owned();
        }
        self.failures.push(format!("{label}: {rule}"));
    }
    pub fn data(&mut self, location: &str, data: &[u8]) {
        self.bytes(location, data, 0);
    }
    fn bytes(&mut self, location: &str, data: &[u8], depth: usize) {
        self.files += 1;
        if self.secrets.iter().any(|s| {
            data.windows(s.len()).any(|w| w == s) || {
                let utf16: Vec<u8> = String::from_utf8_lossy(s)
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect();
                data.windows(utf16.len()).any(|w| w == utf16)
            }
        }) {
            self.fail(location, "known local/environment credential");
        }
        let normalized: Vec<u8> = data.iter().copied().filter(|b| *b != 0).collect();
        for (rule, p) in PATTERNS.iter() {
            if p.is_match(&normalized) {
                self.fail(location, rule);
            }
        }
        if !data.contains(&0) {
            let content = String::from_utf8_lossy(data);
            for (i, line) in content.trim_start_matches('\u{feff}').lines().enumerate() {
                if line.trim_start().starts_with(['#']) || line.trim_start().starts_with("//") {
                    continue;
                }
                for c in ASSIGN.captures_iter(line) {
                    let value = c
                        .get(1)
                        .or_else(|| c.get(2))
                        .or_else(|| c.get(3))
                        .unwrap()
                        .as_str();
                    if c.get(3).is_some() && !LITERAL.is_match(value) {
                        continue;
                    }
                    let lower = location.to_lowercase();
                    let config = [".toml", ".yaml", ".yml", ".env", ".env.example"]
                        .iter()
                        .any(|e| lower.ends_with(e));
                    if !safe(value)
                        && (c.get(3).is_none() || config || ENV_ASSIGN.is_match(line.trim_start()))
                    {
                        self.fail(
                            &format!("{location}:{}", i + 1),
                            "nonempty credential assignment",
                        );
                    }
                }
            }
        }
        if location.to_lowercase().ends_with(".zip") {
            if depth >= 4 {
                self.fail(location, "archive nesting exceeds verification limit");
                return;
            }
            let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(data)) else {
                self.fail(location, "cannot inspect ZIP");
                return;
            };
            for i in 0..archive.len() {
                let Ok(mut entry) = archive.by_index(i) else {
                    self.fail(location, "cannot inspect ZIP entry");
                    continue;
                };
                if entry.is_dir() {
                    continue;
                }
                let label = format!("{location}!{}", entry.name());
                if private_path(Path::new(entry.name())) {
                    self.fail(&label, "private/runtime file in portable archive");
                }
                if entry.is_symlink() {
                    self.fail(&label, "link in portable archive");
                }
                // Bound expansion before allocating: malicious archives must not exhaust memory.
                if entry.size() > 256 * 1024 * 1024 {
                    self.fail(&label, "archive entry exceeds verification limit");
                    continue;
                }
                let mut buf = Vec::new();
                if (&mut entry)
                    .take(256 * 1024 * 1024 + 1)
                    .read_to_end(&mut buf)
                    .is_err()
                    || buf.len() > 256 * 1024 * 1024
                {
                    self.fail(&label, "cannot inspect ZIP entry");
                    continue;
                }
                self.bytes(&label, &buf, depth + 1);
            }
        }
    }
    pub fn file(&mut self, path: &Path, location: &str) {
        if linked(path) {
            self.fail(location, "link cannot be verified as a shareable file");
            return;
        }
        match fs::read(path) {
            Ok(data) => self.data(location, &data),
            Err(_) => self.fail(location, "cannot read file"),
        }
    }
    pub fn package(&mut self, path: &Path) {
        let label = path.to_string_lossy();
        if linked(path) {
            self.fail(&label, "link cannot be verified as a portable package");
        } else if path.is_file() {
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
            {
                self.file(path, &label);
            } else {
                self.fail(&label, "expected portable directory or ZIP");
            }
        } else if path.is_dir() {
            for e in walkdir::WalkDir::new(path).follow_links(false) {
                match e {
                    Ok(e) => {
                        let p = e.path();
                        if linked(p) {
                            self.fail(&p.to_string_lossy(), "link in portable directory");
                        } else if e.file_type().is_file() {
                            if private_path(p.strip_prefix(path).unwrap()) {
                                self.fail(
                                    &p.to_string_lossy(),
                                    "private/runtime file in portable directory",
                                );
                            }
                            self.file(p, &p.to_string_lossy());
                        }
                    }
                    Err(_) => self.fail(&label, "cannot read directory"),
                }
            }
        } else {
            self.fail(&label, "package does not exist");
        }
    }
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let o = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    ensure!(o.status.success(), "Git inspection failed");
    Ok(o.stdout)
}
pub fn project_files(root: &Path) -> Result<(BTreeSet<PathBuf>, BTreeSet<PathBuf>)> {
    let tracked = if root.join(".git").exists() {
        git(root, &["ls-files", "-z"])?
            .split(|b| *b == 0)
            .filter(|n| !n.is_empty())
            .map(|n| PathBuf::from(String::from_utf8_lossy(n).as_ref()))
            .collect()
    } else {
        BTreeSet::new()
    };
    // Deleted working files are checked in the index separately, not opened as nonexistent files.
    let mut files: BTreeSet<PathBuf> = tracked
        .iter()
        .filter(|p| root.join(p).exists() || linked(&root.join(p)))
        .cloned()
        .collect();
    if root.join("config.toml").is_file() {
        files.insert(PathBuf::from("config.toml"));
    }
    let walk = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !private_path(e.path().strip_prefix(root).unwrap()));
    for e in walk {
        let e = e?;
        if e.depth() == 0 {
            continue;
        }
        let p = e.path().strip_prefix(root)?;
        if linked(e.path()) || e.file_type().is_file() {
            let name = p.to_string_lossy();
            if !name.ends_with(".pyc") && !name.ends_with(".log") {
                files.insert(p.into());
            }
        }
    }
    Ok((files, tracked))
}
pub fn check_index(root: &Path, checker: &mut Checker) -> Result<()> {
    if !root.join(".git").exists() {
        return Ok(());
    }
    let records = git(root, &["ls-files", "--stage", "-z"])?;
    let mut entries = Vec::new();
    for record in records.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let Some(tab) = record.iter().position(|b| *b == b'\t') else {
            bail!("invalid index record")
        };
        let meta = std::str::from_utf8(&record[..tab])?
            .split_whitespace()
            .collect::<Vec<_>>();
        let name = String::from_utf8_lossy(&record[tab + 1..]).into_owned();
        ensure!(meta.len() == 3, "invalid index metadata");
        if !["100644", "100755"].contains(&meta[0]) || meta[2] != "0" {
            checker.fail(&name, "unsupported or conflicted staged entry");
            continue;
        }
        if private_path(Path::new(&name)) {
            checker.fail(&name, "private/runtime file staged in Git");
        }
        entries.push((meta[1].to_owned(), name));
    }
    if entries.is_empty() {
        return Ok(());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().unwrap();
    let lines = entries
        .iter()
        .map(|(id, _)| format!("{id}\n"))
        .collect::<String>();
    let writer = std::thread::spawn(move || input.write_all(lines.as_bytes()));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("Git input failed"))??;
    ensure!(output.status.success(), "Git object inspection failed");
    let mut remaining = output.stdout.as_slice();
    for (id, path) in entries {
        let end = remaining
            .iter()
            .position(|b| *b == b'\n')
            .context("missing Git header")?;
        let parts = std::str::from_utf8(&remaining[..end])?
            .split_whitespace()
            .collect::<Vec<_>>();
        ensure!(
            parts.len() == 3 && parts[0] == id && parts[1] == "blob",
            "unexpected Git object"
        );
        let size: usize = parts[2].parse()?;
        remaining = &remaining[end + 1..];
        ensure!(
            remaining.len() > size && remaining[size] == b'\n',
            "incomplete Git object"
        );
        checker.data(&format!("Git index/{path}"), &remaining[..size]);
        remaining = &remaining[size + 1..];
    }
    Ok(())
}
pub fn verify(root: &Path, packages: &[PathBuf]) -> Result<Checker> {
    ensure!(root.is_dir(), "missing project root");
    let mut c = Checker::new(known_credentials(root, &std::env::vars().collect())?);
    let (files, tracked) = project_files(root)?;
    for p in files {
        if tracked.contains(&p) && private_path(&p) {
            c.fail(&p.to_string_lossy(), "private/runtime file tracked by Git");
        }
        c.file(&root.join(&p), &p.to_string_lossy());
    }
    check_index(root, &mut c)?;
    for p in packages {
        c.package(p);
    }
    Ok(c)
}
