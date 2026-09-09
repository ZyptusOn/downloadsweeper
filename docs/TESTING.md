# Rust 测试

在一个源码目录中执行即可，不需要预先启动服务、准备 API key、Python、Node.js 或维护编译副本：

```sh
cargo test --locked
```

Cargo 自动构建被测试的 `ds-web` 和 `ds`。引擎单元/集成测试保持在 Rust；原 Python HTTP、CLI、安全检查及 JavaScript 测试已迁移为 Rust。测试使用独立临时目录、随机本地端口、固定模拟响应和受控子进程；不会调用付费 API 或整理真实桌面/下载目录。失败时 Cargo 输出具体测试名和断言位置。

前端通过仅用于测试的 Rust ECMAScript 引擎 Boa 解析和执行实际 `frontend/*.js`，Rust 检查返回值和几何关系；没有复制一份布局算法，也不启动 Node。React DOM/浏览器视觉效果仍需人工或浏览器验收，逻辑回归不等于完整视觉测试。

## 当前工作流覆盖

2026-09-09 清理了无产品入口的旧 Agent、分类、命名、扫描和计划系统，移除 `legacy-agent` 特性。旧引擎集成测试 `classify.rs`、`rename.rs`、`plan.rs`、`organize.rs` 随之移除，仍适用的行为由实际工作流验证：

| 行为 | 当前回归入口 |
| --- | --- |
| 规则先行、AI 语义分类、真实用量与已完成批次不重发 | Web `classification.rs`、`runtime.rs`、`checkpoints.rs` |
| 重命名保留扩展名、原名不变则不产生移动、禁止外发无权限文件 | Web `workflow.rs` 的重命名回归；`runtime.rs` 覆盖失败续接 |
| 六阶段、用户审查、计划持久化、移动与恢复、取消和冲突 | 引擎 `workflow.rs`、`job_recovery.rs`；Web `workflow.rs`、`checkpoints.rs` |
| 整体目录保护、容器复用与分类树编辑 | 引擎及 Web `desktop.rs`、`workflow.rs`；Web `proposals.rs` |
| 路径越界、权限过滤、内容切片上限和无覆盖移动 | 引擎 `runtime_safety.rs`、`evidence.rs`；Web `classification.rs` |
| 回收站确认与恢复顺序 | 引擎 `recycle` 的模拟后端测试；Web `recycle.rs` 为显式原生专项 |

新工作流仍保留已发布任务的 JSON、指纹和操作日志恢复兼容性；这些兼容路径继续测试。共用的 `DirClass`、`RuleType`、`Usage` 和句柄缩略图处理也予以保留。

旧配置的 `few_shot` 字段仅作为历史配置摘要的兼容数据保留，不再连接旧分类器；清空或删除该序列化字段会改变检查点和回答缓存的键。`job_recovery.rs` 验证清理前后的摘要及配置保存/加载保持一致，当前分类示例仍来自目标节点的具体文件引用。

## 测试分组

```sh
cargo test --locked -p ds-engine
cargo test --locked -p ds-web --tests
cargo test --locked -p ds-web --test frontend
cargo test --locked -p ds-cli
cargo test --locked -p ds-dev
cargo run --locked -p ds-dev -- verify-share --project-only
```

`verify-share --package <目录或ZIP>` 同时检查源码、Git 索引和指定包。检测规则、已知凭据匹配及输出脱敏都由 Rust 实现；该离线检查不能判断服务商是否撤销密钥，也不代替 Git 历史审计。

## 平台及显式测试

Windows/macOS 默认运行原生视频、PDF 渲染及 PDF Agent 工具测试。Linux 运行可用的格式/权限/三协议路径，原生专属测试由编译条件排除。仓库内 H.264 样本和 Rust 生成的 PDF 无需 FFmpeg 或 Office。

仅测试 FFmpeg 后备时，把 `DS_TEST_MEDIA_BIN` 设为含 `ffmpeg`、`ffprobe` 的目录，然后运行：

```sh
cargo test --locked -p ds-web --test media ffmpeg_fallback_protocols -- --ignored
```

原生回收站测试默认忽略，需要 Windows/macOS 桌面会话。只移动测试自己生成的文件，随后恢复；不会清空回收站。失败时先尝试恢复自己的批次，恢复失败会保留临时目录并报告位置：

```sh
cargo test --locked -p ds-web --test recycle -- --ignored
```

引擎原有的原生回收站测试同样保留显式启用方式。不要用一个无筛选的 `--ignored` 同时启用全部平台测试。

## 迁移覆盖对应

以下旧入口均已移除；新入口中的多项 Rust 断言保留原用例意图，部分重复流程合并为公共测试助手。

| 旧脚本 | Rust 测试文件（相对源码根目录） |
| --- | --- |
| `workflow_regression.py` | `crates/web/tests/support/`：进程、HTTP、模拟 API、等待与清理 |
| `smoke_test.py`、`planning_regression.py` | `crates/web/tests/workflow.rs` |
| `classification_regression.py` | `crates/web/tests/classification.rs` |
| `desktop_regression.py`、`desktop_agent_regression.py` | `crates/web/tests/desktop.rs` |
| `atomic_folder_regression.py` | `crates/web/tests/desktop.rs`、`proposals.rs` |
| `ai_proposal_regression.py`、`tree_structure_regression.py`、`duplicate_template_regression.py` | `crates/web/tests/proposals.rs` |
| `inspection_regression.py` | `crates/web/tests/inspection.rs` |
| `review_plan_regression.py` | `crates/web/tests/review.rs` |
| `runtime_regression.py` | `crates/web/tests/runtime.rs` |
| `checkpoint_regression.py` | `crates/web/tests/checkpoints.rs` |
| `archive_cleanup_regression.py` | `crates/web/tests/archives.rs`、`frontend.rs` 的浏览器 JSON 往返 |
| `model_connection_regression.py` | `crates/web/tests/model_connections.rs` |
| `media_regression.py`、`native_preview_regression.py`、`pdf_preview_regression.py`、`pdf_agent_regression.py` | `crates/web/tests/media.rs` |
| `recycle_regression.py` | `crates/web/tests/recycle.rs` |
| `cli_smoke.py` | `crates/cli/tests/cli.rs` |
| 四个 `test_*.mjs` | `crates/web/tests/frontend.rs` |
| `test_verify_share.py`、`test_sync_workspace.py` | `crates/dev/tests/share.rs`、`sync.rs` |

`scripts/` 保留的 Python 文件仅用于生成试用资料、启动交互式本地演示及 macOS 打包，不是测试入口。Windows 打包的凭据检查也已改用 Rust，不再依赖 Python。
