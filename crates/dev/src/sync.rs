//! One-way source -> build copy; preflight conflicts before any mutation.
use crate::share::{self, linked, private_path};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};
pub const MARKER: &str = ".ds-workspace.json";
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    source: String,
    files: BTreeMap<String, String>,
}
pub fn safe_path(root: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty()
            && !name.contains(':')
            && !name
                .replace('\\', "/")
                .split('/')
                .any(|p| p == "." || p == ".." || p.is_empty()),
        "Invalid managed path"
    );
    let p = Path::new(name);
    ensure!(
        !p.is_absolute() && p.components().all(|c| matches!(c, Component::Normal(_))),
        "Invalid managed path"
    );
    let target = root.join(p);
    for part in target.ancestors() {
        ensure!(!linked(part), "Refusing a symlink or reparse point");
        if part == root {
            break;
        }
    }
    ensure!(
        target.starts_with(root),
        "Managed path escapes build directory"
    );
    Ok(target)
}
fn absolute(path: &Path) -> Result<PathBuf> {
    let p = std::path::absolute(path)?;
    // 只拒绝工作区根路径本身是链接：macOS 临时目录位于 /var/folders，
    // 其中的 /var -> /private/var 属于系统前缀符号链接，不应误拒。
    // 根目录内部的链接由 safe_path 在逐文件操作时兜底检查。
    ensure!(
        !linked(&p),
        "Workspace roots must not use links or reparse points"
    );
    let mut normalized = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                ensure!(normalized.pop(), "Invalid workspace root");
            }
            _ => normalized.push(c.as_os_str()),
        }
    }
    Ok(normalized)
}
fn digest(path: &Path) -> Result<Option<String>> {
    if path.is_file() {
        Ok(Some(format!("{:x}", Sha256::digest(fs::read(path)?))))
    } else {
        Ok(None)
    }
}
fn atomic_write(path: &Path, data: &[u8], permissions: Option<fs::Permissions>) -> Result<()> {
    fs::create_dir_all(path.parent().unwrap())?;
    let mut f = tempfile::Builder::new()
        .prefix(".sync-")
        .tempfile_in(path.parent().unwrap())?;
    f.write_all(data)?;
    f.as_file().sync_all()?;
    if let Some(p) = permissions {
        f.as_file().set_permissions(p)?;
    }
    f.persist(path)?;
    Ok(())
}
pub fn sync(source: &Path, destination: &Path, check: bool) -> Result<(usize, usize, usize)> {
    let source = absolute(source)?;
    let destination = absolute(destination)?;
    ensure!(
        !source.starts_with(&destination) && !destination.starts_with(&source),
        "Source and build directories must be separate, non-nested directories"
    );
    ensure!(
        !source.join(MARKER).exists(),
        "Run synchronization from the source repository, not the build copy"
    );
    ensure!(
        source.join("Cargo.toml").is_file(),
        "Source is not a Rust project"
    );
    let marker = safe_path(&destination, MARKER)?;
    let old = if marker.exists() {
        let m: Manifest = serde_json::from_slice(&fs::read(&marker)?)?;
        ensure!(
            m.version == 1 && Path::new(&m.source) == source,
            "Build copy belongs to another source or marker version"
        );
        m.files
    } else {
        ensure!(
            !destination.exists() || fs::read_dir(&destination)?.next().is_none(),
            "Refusing to initialize a nonempty directory without a workspace marker"
        );
        BTreeMap::new()
    };
    let (files, _) = share::project_files(&source)?;
    let mut checker = share::Checker::new(share::known_credentials(
        &source,
        &std::env::vars().collect(),
    )?);
    let mut payloads = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    for name in files {
        if private_path(&name) {
            continue;
        }
        let key = name.to_string_lossy().replace('\\', "/");
        let path = safe_path(&source, &key)?;
        checker.file(&path, &key);
        let data = fs::read(&path)?;
        hashes.insert(key.clone(), format!("{:x}", Sha256::digest(&data)));
        payloads.insert(key, (data, fs::metadata(&path)?.permissions()));
    }
    ensure!(checker.failures.is_empty(),"Source credential verification failed; run cargo run -p ds-dev -- verify-share --project-only");
    let names: BTreeSet<_> = old.keys().chain(hashes.keys()).cloned().collect();
    let (mut changed, mut removed) = (Vec::new(), Vec::new());
    for name in names {
        ensure!(
            !private_path(Path::new(&name)),
            "Private path in managed file manifest"
        );
        let target = safe_path(&destination, &name)?;
        ensure!(
            !target.exists() || target.is_file(),
            "Managed file replaced by a directory"
        );
        let current = digest(&target)?;
        if let Some(ref current) = current {
            ensure!(
                Some(current) == old.get(&name) || Some(current) == hashes.get(&name),
                "Local edit or unmanaged file conflict; copy changes to source first: {name}"
            );
        }
        if hashes.contains_key(&name) && current.as_ref() != hashes.get(&name) {
            changed.push(name);
        } else if !hashes.contains_key(&name) && current.is_some() {
            removed.push(name);
        }
    }
    if !check {
        fs::create_dir_all(&destination)?;
        if !marker.exists() {
            atomic_write(
                &marker,
                &serde_json::to_vec_pretty(&Manifest {
                    version: 1,
                    source: source.to_string_lossy().into_owned(),
                    files: BTreeMap::new(),
                })?,
                None,
            )?;
        }
        for name in &changed {
            let (data, mode) = payloads.get(name).unwrap();
            atomic_write(&safe_path(&destination, name)?, data, Some(mode.clone()))?;
        }
        for name in &removed {
            fs::remove_file(safe_path(&destination, name)?)?;
        }
        atomic_write(
            &marker,
            &serde_json::to_vec_pretty(&Manifest {
                version: 1,
                source: source.to_string_lossy().into_owned(),
                files: hashes.clone(),
            })?,
            None,
        )?;
    }
    Ok((hashes.len(), changed.len(), removed.len()))
}
