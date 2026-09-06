fn main() -> anyhow::Result<()> {
    ds_engine::evidence::dispatch_helper();
    run()
}
#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (default_data, default_config) = ds_engine::config::runtime_paths()?;
    let port = args
        .windows(2)
        .find(|w| w[0] == "--port")
        .map(|w| w[1].parse())
        .transpose()?
        .unwrap_or(3187);
    let data = args
        .windows(2)
        .find(|w| w[0] == "--data-dir")
        .map(|w| std::path::PathBuf::from(&w[1]))
        .unwrap_or(default_data);
    let config = args
        .windows(2)
        .find(|w| w[0] == "--config")
        .map(|w| std::path::PathBuf::from(&w[1]))
        .unwrap_or(default_config);
    let (url, server) = ds_web::start(port, data, config).await?;
    println!("DownloadSweeper · Rust 本地文件整理工作台\n{url}\n按 Ctrl+C 停止服务");
    if args.iter().any(|v| v == "--open") {
        ds_web::open_browser(&url);
    }
    server.await??;
    Ok(())
}
