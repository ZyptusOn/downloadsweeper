use anyhow::{bail, Context, Result};
use std::path::PathBuf;
fn run() -> Result<bool> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut destination = None;
    let mut project_only = false;
    let mut check = false;
    let mut packages = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().context("--root needs a directory")?),
            "--destination" => {
                destination = Some(PathBuf::from(
                    args.next().context("--destination needs a directory")?,
                ))
            }
            "--package" => packages.push(PathBuf::from(
                args.next().context("--package needs a path")?,
            )),
            "--project-only" => project_only = true,
            "--check" => check = true,
            _ => bail!("Unknown option"),
        }
    }
    root = std::path::absolute(root)?;
    match command.as_str(){
        "verify-share"=>{if project_only&&!packages.is_empty(){bail!("--project-only and --package are mutually exclusive");}if !project_only&&packages.is_empty()&&root.join("dist").is_dir(){for e in std::fs::read_dir(root.join("dist"))?{let p=e?.path();if p.is_dir()||p.extension().is_some_and(|e|e.eq_ignore_ascii_case("zip")){packages.push(p);}}}
            let result=ds_dev::share::verify(&root,&packages);let checker=match result{Ok(c)=>c,Err(_)=>{eprintln!("FAIL: cannot enumerate project or read credential sources (details suppressed).");return Ok(false);}};
            for failure in &checker.failures{eprintln!("FAIL: {failure}");}if checker.failures.is_empty(){println!("PASS: {} files inspected; {} portable directories/ZIPs; credential values suppressed.",checker.files,packages.len());println!("Scope: current source + Git index + selected packages; private runtime data and Git history are not shareable.");Ok(true)}else{eprintln!("FAILED: {} findings; credential values suppressed.",checker.failures.len());Ok(false)}
        },
        "sync"=>{let dest=destination.unwrap_or_else(||root.with_file_name(format!("{}-build",root.file_name().unwrap().to_string_lossy())));let(count,copied,removed)=ds_dev::sync::sync(&root,&dest,check)?;println!("{}: {count} source files; {copied} copies, {removed} removals. Private data preserved.",if check{"CHECK"}else{"SYNC"});Ok(!check||(copied==0&&removed==0))},
        _=>bail!("Usage: cargo run -p ds-dev -- verify-share [--root DIR] [--project-only | --package PATH]\n       cargo run -p ds-dev -- sync [--root DIR] [--destination DIR] [--check]")
    }
}
fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}
