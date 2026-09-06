#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// The desktop shell hosts the same loopback service as the browser edition.
// There is one workflow backend and no duplicate IPC command implementation.
fn main() {
    ds_web::dispatch_media_helper();
    let (data, config) = ds_web::default_runtime_paths().expect("无法确定应用数据目录");
    let (url, _server) = tauri::async_runtime::block_on(ds_web::start(0, data, config))
        .expect("无法启动本地 Rust 服务");
    tauri::Builder::default()
        .setup(move |app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url.parse()?),
            )
            .title("DownloadSweeper · 下载目录整理工作台")
            .inner_size(1360.0, 900.0)
            .min_inner_size(800.0, 650.0)
            .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("无法启动 DownloadSweeper 桌面窗口");
}
