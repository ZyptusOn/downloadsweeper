# Apple Silicon 使用与验证

当前代码以 Apple Silicon（aarch64-apple-darwin）为主要 macOS 目标，部署下限设置为 macOS 11。已加入 AVAssetImageGenerator 后端和 Mac 验证流程，但本次开发发生在 Windows；尚无 Mac 编译、实机抽帧或安装验收结果。不要将存在平台代码等同于已经通过验收。

## 运行和数据位置

源码运行需要原生 arm64 Rust 1.89+ 和 Xcode Command Line Tools。`sh scripts/run.sh` 启动浏览器开发版，明确使用项目内 `.ds-data` 和 `config.toml`。网页资源随程序内嵌，无需 Node/npm 构建。

直接运行编译后的 `ds-web --open`、`ds` 或桌面壳时，默认使用 `~/Library/Application Support/DownloadSweeper/` 下的 `.ds-data`、`config.toml` 和 `.env`。三个入口共用数据目录；同一目录一次只允许一个进程打开。不同版本不会自动迁移或覆盖旧项目数据。

浏览器版通过 `--data-dir`、`--config` 指定旧数据位置；所有入口均支持 `DS_DATA_DIR`、`DS_CONFIG` 环境变量。浏览器参数优先于环境变量。CLI 操作旧项目可这样运行：

```sh
DS_DATA_DIR="$PWD/.ds-data" DS_CONFIG="$PWD/config.toml" cargo run -p ds-cli -- list
```

可在设置页填写密钥，它保存到配置文件旁的私有 `.env`，TOML 只记录环境变量名。Unix 下写入时将 `.env` 权限设为 0600，拒绝符号链接。也可在启动进程前导出 `DS_API_KEY`。Finder 不继承终端配置中的环境变量，桌面用户优先使用设置页或应用数据目录中的 `.env`。分享时只提供 `.env.example` 和 `config.example.toml`。

macOS 会单独管理下载、文稿和桌面等目录的访问权限。遇到权限拒绝，请到系统设置的“隐私与安全性 → 文件与文件夹”检查实际启动应用；通过终端运行时可能需要允许终端访问。Tauri 包中包含读取用途说明；应用内逐格式权限仍独立生效，系统授权不会自动打开内容读取。

## 原生视频和安全边界

AVFoundation/Objective-C 桥接在构建时静态链接进程序，用户端无需 Swift CLI、Python 或 FFmpeg。使用 AVAssetImageGenerator 异步生成最多三个 256px 帧并拼成一张 512px JPEG；修正轨道旋转，允许系统快速选择附近关键帧，结果带请求时间和实际时间。源尺寸最多 8192×8192，单文件读取累计最多 64MiB，整个原生子进程最多 8 秒。回调等待超时后保留已取得的帧；取消会停止处理。

PDF 使用同样的独立进程和已核验文件描述符，由系统 CoreGraphics 数据提供器读取并渲染最多三页，不额外引入 PDFKit 或第三方 PDF 库。逐页输出可恢复的预览快照；文件、累计读取、尺寸及超时限制见 MULTIMODAL.md。代码及原生测试已加入 Apple Silicon CI，本轮未在 Mac 实机运行。

父进程将已验证的文件描述符作为标准输入传给子进程；自定义 AVAssetResourceLoader 仅用 pread 读取这个句柄，禁止外部媒体引用和其他 URL。子进程不加载配置或 .env，也不继承凭据环境变量。缩略图尺寸和读取预算不代表系统解码器的峰值内存严格等于该数值。

优先尝试 MP4/M4V/带 ftyp 的 MOV，以及 AVI；具体编码由系统支持情况决定。其他容器、资源超限或解码失败时，可选使用 FFmpeg 后备。除可执行文件同目录和 PATH 外，还检查 `/opt/homebrew/bin`、`/usr/local/bin` 和 `/usr/bin`，照顾 Finder 启动场景。全部失败时沿用已授权的文件信息，不自动安装解码器。详见 MULTIMODAL.md。

Unix 内容读取逐层使用 openat/O_NOFOLLOW，并在 Mac 用 F_GETPATH 检查已打开文件的位置。Unix 原文件名保留反斜杠、冒号等字符；新生成的名称仍使用便于跨平台分享的限制。非 UTF-8 名称会跳过并报告警告。`.app` 会被建议整体保护，但含符号链接的应用包仍拒绝整体移动；本次没有放宽这一安全限制。

## 验证和打包

```sh
cargo test --locked
cargo build --locked -p ds-web -p ds-cli
cargo check --locked -p ds-tauri
cargo test --locked -p ds-cli
cargo test --locked -p ds-web --tests
cargo test --locked -p ds-web --test runtime
cargo test --locked -p ds-web --test model_connections
```

默认 `cargo test` 直接使用仓库内的合成 H.264 视频和 Rust 生成的 PDF，验证原生抽帧及页面渲染，无需 FFmpeg，不调用真实模型。只有可选后备测试需要 FFmpeg 生成 FFV1 视频：

```sh
export DS_TEST_MEDIA_BIN="$(brew --prefix ffmpeg)/bin"
cargo test --locked -p ds-web --test media ffmpeg_fallback_protocols -- --ignored
```

`.github/workflows/apple-silicon.yml` 在 macos-15 ARM runner 上执行这些检查；上传到 GitHub 后才会运行。当前没有已通过的远端运行记录。

`python3 scripts/package_macos.py` 在 Apple Silicon Mac 本机构建 arm64 release，生成浏览器便携 ZIP、可执行 Start.command 和 SHA-256 清单；可加 `--include-cli`。脚本检查源文件、包目录和 ZIP 中的凭据，并验证 ZIP 保留启动文件执行权限。只有白名单文件进入包，当前配置、任务和 .env 不会打包。

包尚未做 Developer ID 签名和 Apple 公证，不应作为已经完成正式分发验证的安装包。Tauri `.app`/DMG 还需要在 Mac 使用 Tauri CLI 构建并完成签名、公证及 Finder 启动测试。不要通过关闭系统整体安全检查来绕过这些步骤。

参考：[Apple 图像生成](https://developer.apple.com/documentation/avfoundation/avassetimagegenerator)、[外部媒体引用限制](https://developer.apple.com/documentation/avfoundation/avurlassetreferencerestrictionskey)、[Tauri Mac 打包](https://v2.tauri.app/distribute/macos-application-bundle/)。
