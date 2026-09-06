# 开发与提交

## 首次运行

编译和测试只需 stable Rust 及平台原生编译工具，不需要 Python、Node.js 或 npm。Windows 使用 MSVC 工具链；macOS 见 [平台说明](docs/MACOS.md)。Python 3.11+ 仅用于可选的试用数据生成、演示启动和 macOS 打包工具。

在项目根目录编译、运行和测试即可。按需复制 `config.example.toml` 为 `config.toml`、`.env.example` 为 `.env`，或启动后通过网页配置。规则整理不需要 API key，配置和任务数据不提交。用户安装与操作见 [README](README.md)。

```text
cargo run --locked -p ds-web -- --open
```

## 仓库内容

| 路径 | 内容 |
| --- | --- |
| `crates/engine` | Rust 工作流、权限、模型编排、文件安全和恢复 |
| `crates/web`、`crates/cli`、`src-tauri` | Web、CLI 与桌面入口 |
| `frontend` | 界面与运行必需的本地第三方资源 |
| `crates/*/tests`、`crates/dev` | Rust 测试、凭据检查和维护者同步工具 |
| `docs`、`scripts`、`.github` | 文档、试用数据/打包工具与 CI |
| `Cargo.toml`、`Cargo.lock` | 工作区配置和应用依赖锁定文件，均需提交 |

`target/`、`dist/`、`artifacts/`、`src-tauri/gen/`、Python 缓存由工具生成，不提交。`.env`、`config.toml`、`.ds-data/` 和目录覆盖记录属于本机私有数据，不提交。配置示例可以提交，但必须保持不含密钥。发布产物通过打包脚本生成，适合放在 GitHub Release，勿放进源码提交。

核心业务逻辑保持在 Rust。文件写入必须经过计划审查、无覆盖操作和日志校验；AI 工具不能绕过读取权限或直接删除文件。

## 提交前检查

```text
cargo run --locked -p ds-dev -- verify-share --project-only
cargo fmt --all -- --check
cargo test --locked
```

`cargo test` 自动编译测试所需的 Web/CLI 可执行文件，覆盖真实 HTTP、进程重启、文件恢复和前端逻辑。测试自建临时目录和本地模型服务，不读取真实模型凭据。前端测试由 Rust 内嵌 Boa 执行实际 JavaScript，断言写在 Rust 中。原生预览按宿主平台运行；FFmpeg 后备与系统回收站测试需显式启用，详见 [测试说明](docs/TESTING.md)。

`git add` 后再次运行分享检查，再查看 `git diff --cached --stat`。检查器也检查索引中的实际文件内容，因此强制加入 `.env` 或本地配置会失败；不检查历史提交，也不联网查询密钥是否仍然有效。

## 维护者可选的编译副本

维护者若需要分开源码与运行数据，可在源码目录执行 `cargo run --locked -p ds-dev -- sync`，创建或更新同级 `downloadsweeper-build`。这只是开发安排，不是使用或贡献项目的前置要求。

同步前停止副本中的构建与运行。工具保留私有配置、任务和缓存；副本源码被手动修改时会停止，请先将需要的修改合回源码。`cargo run --locked -p ds-dev -- sync --check` 检查一致性，`cargo test --locked -p ds-dev --test sync` 验证同步保护。可用 `--root` 和 `--destination` 显式指定两个目录。

## 目录清理与本机备份

停止本项目的运行实例后，可用 `cargo clean` 删除可重建的编译缓存。`.ds-data` 含任务轨迹与恢复信息，`.env` 含私有凭据；清理源码目录时应把它们与需要保留的便携包移到项目外备份，勿当作缓存删除。若将本机配置和任务目录放在项目外，可通过 `DS_CONFIG`、`DS_DATA_DIR` 或启动参数指定原数据位置。
