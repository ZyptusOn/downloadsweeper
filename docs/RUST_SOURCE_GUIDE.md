# DownloadSweeper Rust 源码逐文件说明

以 2026-09-09 精简后的源码为准，共 **77 个 `.rs` 文件**。下面每个文件都有独立条目，覆盖正式程序、开发工具、构建脚本、示例和测试；不统计 `target` 中依赖库及自动生成的源码。

| 范围 | 文件数 | 是否属于正式程序运行逻辑 |
| --- | ---: | --- |
| 引擎、Web、CLI、Tauri 外壳 | 41 | 是；不同入口按需编译、运行 |
| `ds-dev` 开发工具 | 4 | 否，用于分享检查和开发目录同步 |
| 构建脚本 | 2 | 在编译阶段运行 |
| 指纹性能示例 | 1 | 显式运行示例时使用 |
| 测试及测试辅助模块 | 29 | `cargo test` 使用，不作为正式程序业务入口 |
| **合计** | **77** | |

## 先理解整体分工

一次整理主要经过以下调用链：

```text
浏览器界面 / CLI / Tauri 窗口
    → Web 或 CLI 接口层
    → workflow：扫描、权限、目录树、计划和审查状态
    → workflow_ai：按场景组织 AI 任务与受限工具
    → ai_runtime：并发、预算、计费记录和响应持久化
    → llm：具体厂商 HTTP 协议

用户最终确认执行
    → safe_fs：验证文件、记录操作意图、移动和恢复

配套模块
    permission：读取授权       evidence / metadata：轻量内容取样
    jobs / response_cache：暂停、恢复和已完成响应复用
    archive / chat_memory：历史归档和发送给模型的历史选择
    cleanup / recycle：清理建议、用户确认后的回收及撤销
```

核心不是给模型一个任意操作文件的终端。Rust 先决定当前允许查看的内容和可选择的分类，模型通过结构化回答或受限工具提出结果，Rust 再验证、形成计划；真正移动文件另走确认和恢复流程。

### 如何阅读 crate 一栏

- 列的是文件使用的主要**第三方 crate**，包括经父模块 `use super::*` 引入的类型、宏；不是把所属包的整个依赖表复制到每个文件。
- `std` 是 Rust 标准库，`ds-engine`、`ds-web`、`ds-dev` 是本项目自己的 crate，均不算第三方。
- `tokio-util` 等是 Cargo 包名，在 Rust 代码中通常写成 `tokio_util`。
- 标为“测试”的依赖使用，仅发生在该文件的内嵌测试中；操作系统专用依赖只在对应平台编译。
- 测试文件使用共享 `support` 时，其 HTTP 客户端、临时目录等依赖在辅助模块条目中统一解释。

## 一、核心状态、规则与配置（8 个）

| # | 文件 | 用途与实现的功能 | 第三方 crate |
| ---: | --- | --- | --- |
| 1 | [engine/src/lib.rs](../crates/engine/src/lib.rs) | **核心引擎的模块入口。** 导出工作流、AI、权限、存储、媒体等模块，统一公共 `Result` 类型。GUI 和 CLI 通过它复用同一套业务能力。 | `anyhow`：通用错误结果。 |
| 2 | [engine/src/workflow.rs](../crates/engine/src/workflow.rs) | **整个软件的业务中心。** 定义任务 `Task`、扫描条目 `Entry`、目标节点 `Node`、操作 `Operation`、AI 建议 `Proposal` 等数据结构；实现六阶段流转、扫描、桌面模式、目录类型、模板、简单规则规划、容器映射、建议合并和失效检查。还校验树的环路、重名、一级简单规则、扩展名约束与孤立节点。 | `serde`、`serde_json`：任务与建议序列化；`walkdir`：扫描；`tokio-util`：取消；`uuid`：标识；`chrono`：时间；`anyhow`：校验错误。 |
| 3 | [engine/src/domain.rs](../crates/engine/src/domain.rs) | **目录类型的共享定义。** `Normal` 表示可拆分目录，`Atomic` 表示整体处理，`Container` 表示可复用容器。文件虽小，但多个模块需要使用完全一致的类型。 | `serde`：保存、加载目录类型。 |
| 4 | [engine/src/tree.rs](../crates/engine/src/tree.rs) | **目录规则类型定义。** 保留 `Simple` / `Complex` 两种规则及序列化。实际节点结构、模板和树校验已集中在 `workflow.rs`，这里不再是一套独立的旧树引擎。 | `serde`。 |
| 5 | [engine/src/config.rs](../crates/engine/src/config.rs) | **读取、校验和保存配置。** 管理 Endpoint、Key 的环境变量来源、模型、上下文、思考及输出参数、并发、预算、价格与搜索配置；合并 TOML、`.env` 和环境变量，保存 UI 输入的 Key 到私有环境文件。提供下载目录、桌面及运行数据路径；保留旧配置摘要兼容所需的私有字段结构。 | `serde`、`toml`：配置格式；`dotenvy`：环境文件；`dirs`：系统目录；`uuid`：临时文件名；`anyhow`；Unix 下 `libc`：安全文件操作。 |
| 6 | [engine/src/permission.rs](../crates/engine/src/permission.rs) | **统一文件读取权限。** 定义不访问、只读文件名、元数据、图像及内容切片权限；按扩展名、文件大小等匹配规则。提供带类别名、颜色和可编辑扩展名的预设，供设置页和 AI 证据读取共同使用。 | 正式逻辑 `serde`；内嵌测试另用 `serde_json`。 |
| 7 | [engine/src/cost.rs](../crates/engine/src/cost.rs) | **Token 用量结构与累加。** 保存输入、输出、缓存读取和缓存写入用量，提供总量及合并操作。它不计算货币费用，也不负责拦截超预算请求。 | `serde`：保存 API 返回的用量。 |
| 8 | [engine/src/pricing.rs](../crates/engine/src/pricing.rs) | **费用换算。** 从内置价格目录匹配供应商和模型，按普通输入、缓存、输出、上下文档位或时间条件计算费用；支持手动单价和币种分类汇总，未知价格不当成免费。当前使用随项目维护的官方价格资料快照，**不是运行时自动抓取官网价格**。 | `serde`、`serde_json`：价格表和账目；`chrono`：计价时间条件；`anyhow`：无效价格或用量校验。 |

## 二、Agent 工作流与模型调用运行时（8 个）

| # | 文件 | 用途与实现的功能 | 第三方 crate |
| ---: | --- | --- | --- |
| 9 | [engine/src/workflow_ai.rs](../crates/engine/src/workflow_ai.rs) | **场景 Agent 的公共入口和证据授权层。** 构造 AI 可见的文件描述、文本及图像证据，统一权限检查、回答解析、连接测试、场景对话和清理建议复核；组织下列专用任务。发送给模型的文件引用由本地映射，不直接授予任意路径访问能力。 | `serde_json`：提示词上下文和工具数据；`tokio`、`tokio-util`：异步与取消；`anyhow`；`uuid`、`chrono`：记录标识与时间。 |
| 10 | [workflow_ai/inspection.rs](../crates/engine/src/workflow_ai/inspection.rs) | **AI 扫描分析及目标树建议。** 先取得概览，再按文件类别分批分析并保存摘要，最后结合本地模板建议目标结构；不同组可并行，同组衔接已有摘要。保存检查点，避免恢复时无条件重做全部分析。 | `tokio`、`tokio-util`、`futures`：并发与取消；`sha2`：输入摘要；`serde_json`、`anyhow`。 |
| 11 | [workflow_ai/classification.rs](../crates/engine/src/workflow_ai/classification.rs) | **生成 AI 细化分类计划的主体。** 根据规则选出需要语义分类的文件，按上下文容量组批，用短标识映射文件和目标节点；提供读取证据、提交分类的受限工具，验证结果是否完整、重复或越界。支持并行批次、检查点、按需证据和审查后的局部重新分类。产出计划，不直接移动文件。 | `tokio`、`tokio-util`、`futures`：批次并发；`serde`、`serde_json`：分类及工具协议；`sha2`：输入摘要；`uuid`：记录标识；`anyhow`。 |
| 12 | [workflow_ai/folder.rs](../crates/engine/src/workflow_ai/folder.rs) | **整体文件夹的轻量证据。** 只查看一层目录，最多考察 128 项、返回 24 项，最多尝试两份短文本或 Office 摘录；父目录与子文件权限共同约束读取。跳过快捷方式等条目，不递归解码图片，也不因此拆散文件夹。 | `serde_json`：目录摘要；经父模块使用 `anyhow`：权限及路径错误。目录枚举使用标准库。 |
| 13 | [workflow_ai/tree_proposal.rs](../crates/engine/src/workflow_ai/tree_proposal.rs) | **整理和校验 AI 目录树修改建议。** 补齐局部编辑字段、规范节点名、处理父子引用；合并可以安全复用的同名新增节点，避免 AI 再添加一个“照片”目录导致整批失败。不兼容的重复或非法结构仍拒绝。 | `serde_json`：变更数据；`uuid`：节点标识；`anyhow`：结构检查。 |
| 14 | [workflow_ai/review.rs](../crates/engine/src/workflow_ai/review.rs) | **审查页的自然语言调整。** 将用户接受的树修改和文件归属修改应用到计划草稿，只重新分类受影响的语义分支，并保留其他选择；成功后整体发布到“整理后”预览。取消或失败保留原计划；此处文件移动数为零，仍需最终执行。 | `tokio`：本地阻塞工作调度；`tokio-util`：取消；`serde_json`、`anyhow`：继承的建议处理能力；`uuid`：操作标识。 |
| 15 | [workflow_ai/rename_job.rs](../crates/engine/src/workflow_ai/rename_job.rs) | **独立的文件名重生任务。** 对用户选定范围逐文件提取授权证据，可按配置搜索文件名，再生成可读名称；保留扩展名、检查名称合法性、处理未改变名称和冲突。支持并发与逐文件恢复，最后形成重命名计划。 | `tokio`、`tokio-util`、`futures`；`reqwest`：可选搜索请求；`serde_json`；`sha2`：输入摘要；`uuid`、`chrono`；`anyhow`。 |
| 16 | [engine/src/ai_runtime.rs](../crates/engine/src/ai_runtime.rs) | **所有 AI 任务共用的请求控制层。** 验证工具调用、控制并发和上下文，在发送 HTTP 前持久化预算预留；接收已确认用量、费用和响应，交由协调者统一保存任务。区分明确失败与费用不确定的请求，支持取消及已保存响应复用，避免各 Agent 各自维护一套计费与恢复逻辑。 | `tokio`：信号量、通道和任务；`tokio-util`：取消；`serde_json`；`blake3`：请求摘要；`uuid`、`chrono`；`anyhow`。 |

## 三、模型协议适配（7 个）

| # | 文件 | 用途与实现的功能 | 第三方 crate |
| ---: | --- | --- | --- |
| 17 | [llm/mod.rs](../crates/engine/src/llm/mod.rs) | **LLM 子模块入口。** 导出客户端、模型发现、协议类型等，让上层使用统一接口。 | 无直接第三方调用；主要转导出本项目模块。 |
| 18 | [llm/client.rs](../crates/engine/src/llm/client.rs) | **统一客户端接口。** 定义异步 `LlmClient::chat` 及取消参数；用 `RejectedRequest` 表示明确被拒绝或本地构造失败，帮助运行时区分“没有正常受理”与“是否计费未知”。 | `async-trait`：异步 trait；`thiserror`：专门错误类型；`tokio-util`：取消。 |
| 19 | [llm/types.rs](../crates/engine/src/llm/types.rs) | **厂商无关的消息模型。** 定义角色、消息、图像、工具定义、工具调用、请求选项及响应。包含继续多轮工具调用所需的厂商状态字段。 | `serde`、`serde_json`：结构化协议数据。 |
| 20 | [llm/openai.rs](../crates/engine/src/llm/openai.rs) | **实际发送 HTTP 的客户端实现。** 虽名为 OpenAI，目前承载 Chat Completions、Responses 和 Anthropic Messages 三类接入；处理 Endpoint、共享连接池、请求参数、超时、取消、响应体及用量解析。模型回答完整接收后校验；界面实时进度由任务层 SSE 提供，不能把二者混为逐 Token 输出。 | `reqwest`：HTTP，TLS 使用其配置的 rustls 后端；`tokio`、`tokio-util`、`futures`：异步、分块读取与取消；`async-trait`；`serde_json`、`anyhow`。 |
| 21 | [llm/native.rs](../crates/engine/src/llm/native.rs) | **厂商原生 API 的编解码。** 将统一消息转换为 Anthropic Messages 或 OpenAI Responses 格式，再还原响应；处理图像、工具结果、思考签名/续接字段和缓存用量。这里的 native 指 API 协议，不是操作系统原生解码。 | `serde_json`、`anyhow`。 |
| 22 | [llm/providers.rs](../crates/engine/src/llm/providers.rs) | **供应商和模型差异规则。** 使用内置能力目录确定协议、模型家族、视觉能力、思考参数、采样参数及输出限制；校验 URL 并选择对应路径、认证头。统一处理不同厂商的兼容性差异。 | `serde`、`serde_json`：能力目录；`reqwest`：URL、请求构造；`anyhow`。 |
| 23 | [llm/discovery.rs](../crates/engine/src/llm/discovery.rs) | **填写 Key 后发现模型。** 调用供应商模型列表接口，整理模型 ID、上下文和视觉能力，处理列表差异及分页；接口未提供的信息可结合预设补充并标明来源。预设候选不等于该 Key 已确认有权调用。 | `reqwest`、`tokio`：模型查询；`serde`、`serde_json`：列表解析；`anyhow`。 |

## 四、文件证据、图片、视频和 PDF（4 个）

| # | 文件 | 用途与实现的功能 | 第三方 crate |
| ---: | --- | --- | --- |
| 24 | [engine/src/metadata.rs](../crates/engine/src/metadata.rs) | **图片缩略图和证据编码。** 从已经验证的文件句柄解码 PNG/JPEG，限制尺寸和内存，按比例缩小，编码成 JPEG 和 Base64。精简后它不是通用元数据提取器；Base64 编码在这里由本地函数实现。 | 正式逻辑 `image`；内嵌测试另用 `uuid`。 |
| 25 | [engine/src/evidence.rs](../crates/engine/src/evidence.rs) | **多模态和 Office 轻量预览的公共层。** 从 ZIP/XML 格式的 Office 文件提取有限文本，限制解压、XML 和输出规模；提供本地预览并发控制、视频取样时间、拼图与能力信息。调度原生解码，必要时使用可用的 FFmpeg 后备方案，并约束子进程时间与环境。 | `zip`、`quick-xml`：Office 内容；`image`：拼图；`tokio`、`tokio-util`、`futures`：进程及并发；`serde`、`serde_json`、`anyhow`。ZIP 的 DEFLATE 后端由 Cargo 中的 `flate2` 配置启用。 |
| 26 | [evidence/native.rs](../crates/engine/src/evidence/native.rs) | **系统原生视频解码和媒体辅助子进程。** Windows 调用系统媒体 API，macOS 通过 FFI 调用 Objective-C 的 AVAssetImageGenerator 桥接代码。辅助进程在加载模型配置前运行，渐进发布已取得画面，超时终止；避免卡死的解码器长期阻塞主服务。它是进程隔离和资源约束，不是完整操作系统沙箱。 | `tokio`、`tokio-util`：子进程与取消；`serde_json`：进程通信；`image`：图像；`anyhow`；Windows 用 `windows`，Unix 相关操作用 `libc`。 |
| 27 | [evidence/pdf.rs](../crates/engine/src/evidence/pdf.rs) | **PDF 页面切片。** 选取开头和中间的少量页面，去重后渲染并拼图；限制文件大小，复用媒体辅助进程。Windows 使用 `Windows.Data.Pdf`，macOS 经桥接调用 CoreGraphics；不是上传整个 PDF，也不是实现完整 PDF 阅读器或独立 OCR 引擎。 | `image`、`serde_json`、`anyhow`、`tokio-util`；Windows 下 `windows`。macOS 系统框架经 FFI 调用。 |

## 五、文件安全、恢复、历史和清理（10 个）

| # | 文件 | 用途与实现的功能 | 第三方 crate |
| ---: | --- | --- | --- |
| 28 | [engine/src/safe_fs.rs](../crates/engine/src/safe_fs.rs) | **真正接触磁盘的安全执行层。** 检查根目录、相对路径和链接，打开并验证证据句柄；提供 `TaskStore`、原子 JSON 保存和轨迹记录。执行前验证快照及冲突，先记操作意图再做不覆盖移动，支持继续执行、回滚和启动恢复。整理移动以同卷操作为前提。 | `anyhow`、`serde`、`serde_json`、`tokio-util`；`uuid`、`chrono`；`walkdir`、`blake3`：目录快照；Windows 用 `windows-sys`，Unix 用 `libc`。`std::os::windows` 属于标准库。 |
| 29 | [safe_fs/content_hash.rs](../crates/engine/src/safe_fs/content_hash.rs) | **带版本标记的恢复指纹。** 支持旧 SHA-256、新 BLAKE3 及自适应采样指纹，校验时按记录中的算法解释；当前自适应方案小文件全读，大文件取五段，并处理目录结构和空目录。支持取消、进度和历史格式兼容。大文件采样不是完整内容哈希，不能保证发现未采样区的所有修改。 | `sha2`、`blake3`：哈希；`walkdir`：目录遍历；`tokio-util`：取消；`anyhow`。内嵌测试另用 `uuid`。 |
| 30 | [engine/src/jobs.rs](../crates/engine/src/jobs.rs) | **可持久化任务控制。** 定义任务和批次进度、检查点及校验封装；保存运行参数摘要，恢复时检查原任务、配置和待确认调用是否仍匹配。支持重启后识别未完成任务；不把 API Key 明文写入检查点参数。 | `serde`、`serde_json`；`blake3`：摘要；`uuid`、`chrono`：标识与时间；`anyhow`。 |
| 31 | [engine/src/response_cache.rs](../crates/engine/src/response_cache.rs) | **已完成模型响应的持久化与重放。** 保存规范化响应、调用记录和请求标识，恢复时核对校验和并补齐用量记录；同一运行可复用已保存响应，避免重复请求和重复累计。这不是跨任务通用语义缓存。 | `serde`、`serde_json`、`blake3`、`anyhow`。 |
| 32 | [engine/src/archive.rs](../crates/engine/src/archive.rs) | **完整任务归档。** 导出任务状态与操作轨迹，保存版本和校验和；导入时验证完整性并放入只读归档区，不直接变成可执行的文件任务。使用不透明 JSON 载荷保留数值精度，支持取消导出。 | `serde`、`serde_json`；`blake3`：完整性校验；`uuid`、`chrono`；`tokio-util`、`anyhow`。 |
| 33 | [engine/src/chat_memory.rs](../crates/engine/src/chat_memory.rs) | **为当前对话选择历史。** 在上下文预算内选取同场景消息，处理过期建议及当前不可见场景信息；不再单纯固定截取最近八条，也不额外调用模型生成摘要。完整本地历史与这次实际发送的历史子集是两回事。 | `serde_json`：历史及建议数据；使用项目自己的 LLM 消息类型。 |
| 34 | [engine/src/cleanup.rs](../crates/engine/src/cleanup.rs) | **本地清理候选生成器。** 按大小、名称和时间识别七类候选：大文件、临时文件、未完成下载、旧安装包、旧压缩包、疑似副本和空文件；保护整体文件夹等对象。只是待审查建议，不读取正文判重，也不直接删除。 | `serde`、`serde_json`：选项和候选；`tokio-util`：取消；`anyhow`：配置校验。 |
| 35 | [engine/src/recycle.rs](../crates/engine/src/recycle.rs) | **用户确认后的回收及撤销事务。** 校验用户所选候选，使用完整 BLAKE3 检查内容，给每项建立独立恢复目录和持久化记录，再调用系统回收站。处理取消、回复丢失、重启和重名冲突；与普通整理移动分开管理。 | `serde`、`serde_json`；`uuid`：恢复标识；`blake3`：完整内容哈希；`tokio-util`、`anyhow`。 |
| 36 | [recycle/native.rs](../crates/engine/src/recycle/native.rs) | **系统回收站适配。** 实现回收能力检查、条目查找、送入回收站和恢复。Windows 通过 Shell 操作并使用回收站收据定位；macOS 调用 Objective-C 桥接。不支持可靠回收的平台或位置会返回错误。 | `anyhow`；Windows 用 `windows` 调 Shell，`trash` 枚举/恢复回收站条目；macOS 桥接系统 API。 |
| 37 | [recycle/windows_guard.rs](../crates/engine/src/recycle/windows_guard.rs) | **Windows 回收安全护栏。** 实现 `IFileOperationProgressSink`，检查 Shell 删除操作必须走回收站；阻止不能回收时退化为永久删除。单独保留是为了隔离 COM 实现与安全约束。 | `windows`、`windows-core`：COM 接口及实现宏。仅 Windows 编译。 |

## 六、程序入口、开发工具和构建（11 个）

| # | 文件 | 用途与实现的功能 | 第三方 crate |
| ---: | --- | --- | --- |
| 38 | [web/src/lib.rs](../crates/web/src/lib.rs) | **浏览器版后台与 HTTP 接口。** 提供页面资源、任务/配置/模型/建议/归档/操作路由；校验 Host、Origin 和会话凭据，避免网页直接随意调用本地服务。把长任务放入后台执行，通过 SSE 推送普通及并行进度，提供暂停、恢复和结果查询。调用引擎，不另写分类业务。 | `axum`：路由及 SSE；`tokio`、`tokio-util`、`tokio-stream`、`futures`：任务、取消和事件流；`serde`、`serde_json`；`uuid`、`chrono`、`anyhow`；`reqwest`：相关请求/URL 支持。 |
| 39 | [web/src/main.rs](../crates/web/src/main.rs) | **浏览器版可执行入口 `ds-web`。** 先识别媒体辅助进程模式，再解析端口、配置和数据目录等启动参数，创建异步运行时并启动 Web 服务；可打开浏览器。 | `tokio`、`anyhow`；服务和业务来自本地 `ds-web`、`ds-engine`。 |
| 40 | [cli/src/main.rs](../crates/cli/src/main.rs) | **命令行入口 `ds`。** 将扫描、阶段切换、计划、AI 建议、合并、执行、撤销、归档等命令映射到同一引擎，支持 Ctrl+C 取消。它是可脚本化命令接口；没有另维护一套旧 Agent。参数由标准库解析，未使用 `clap`。 | `tokio`、`tokio-util`、`serde_json`、`anyhow`、`uuid`。 |
| 41 | [src-tauri/src/main.rs](../src-tauri/src/main.rs) | **可选的原生桌面外壳。** 启动同一个本地 Web 服务，然后用系统 WebView 打开它，设置窗口标题、初始尺寸和最小尺寸。业务仍在 Rust 引擎和共享 Web 后台。注意“桌面整理模式”是业务模式，不要求使用这个 Tauri 外壳。 | `tauri`：桌面窗口、WebView 和运行时；另调用本地 `ds-web`。 |
| 42 | [dev/src/lib.rs](../crates/dev/src/lib.rs) | **开发工具模块入口。** 导出分享检查与目录同步，供命令入口及 Rust 测试共用。 | 无直接第三方调用。 |
| 43 | [dev/src/main.rs](../crates/dev/src/main.rs) | **开发工具入口 `ds-dev`。** 解析 `verify-share` 和 `sync` 及相关参数，定位项目，汇总结果并返回适当退出码，便于脚本和打包流程调用。 | `anyhow`；其余参数解析和退出码使用标准库。 |
| 44 | [dev/src/share.rs](../crates/dev/src/share.rs) | **分享前的凭据检查。** 检查项目文件、Git 暂存区以及指定便携目录/ZIP；检查私有路径、已知 Key、疑似密钥、二进制及 UTF-16 内容，受限展开压缩包。报错不回显密钥。是离线防泄漏检查，不验证 Key 是否在线有效，也不等于审计所有 Git 历史。 | `regex`：模式检查；`toml`：配置识别；`walkdir`：遍历；`zip`：包检查；`anyhow`。Cargo 的 `flate2` 为 ZIP 启用纯 Rust 解压后端。 |
| 45 | [dev/src/sync.rs](../crates/dev/src/sync.rs) | **开发时的单向源码同步。** 用管理清单和 SHA-256 判断哪些文件归同步器管理，先检查冲突，再原子复制；只移除满足条件的已管理旧文件，保护私有配置和非托管文件。普通使用者编译运行不需要维护两套目录。 | `serde`、`serde_json`：清单；`sha2`：摘要；`tempfile`：临时文件和原子替换；`anyhow`。 |
| 46 | [engine/build.rs](../crates/engine/build.rs) | **macOS 原生桥接构建。** macOS 编译时构建视频、PDF 和回收站的 `.m` 文件，链接 Foundation、AVFoundation、CoreMedia、CoreGraphics、ImageIO 等系统框架。主控逻辑仍在 Rust；其他平台不执行这套 Objective-C 构建。 | 构建依赖 `cc`。Apple 框架和 Objective-C 运行库不是 Rust crate。 |
| 47 | [src-tauri/build.rs](../src-tauri/build.rs) | **Tauri 构建入口。** 调用 Tauri 的构建支持，处理配置与平台资源等构建集成。 | 构建依赖 `tauri-build`。 |
| 48 | [engine/examples/fingerprint_bench.rs](../crates/engine/examples/fingerprint_bench.rs) | **可单独运行的指纹性能示例。** 创建自身的临时测试文件，比较完整 SHA-256、完整 BLAKE3 与采样指纹，输出 JSON 计时；不读取用户真实下载目录。用来观察本机性能，不是正式整理入口，也不是固定性能承诺。 | `sha2`、`blake3`：被测算法；`serde_json`：报告；`uuid`：测试路径；`tokio-util`、`anyhow`。 |

## 七、核心引擎测试（10 个）

这些文件验证引擎行为。它们不是冗余的业务实现；相同模块名表示对应测试对象。

| # | 文件 | 主要验证什么 | 第三方 crate |
| ---: | --- | --- | --- |
| 49 | [engine/tests/workflow.rs](../crates/engine/tests/workflow.rs) | 六阶段约束、目录分类与容器、树校验、孤立节点、权限、选择与计划失效；真实临时文件的执行/回滚、先记录意图、冲突、快照变化、历史指纹兼容及存储锁。 | `tokio-util`、`serde_json`、`uuid`、`sha2`。 |
| 50 | [engine/tests/desktop.rs](../crates/engine/tests/desktop.rs) | 桌面浅层扫描、完整阶段流程、自定义树、已有文件夹整体处理、容器复用；无可细化文件时保留规则结果，以及移动、取消、内容校验与回滚。 | `tokio`、`tokio-util`、`serde_json`、`uuid`。 |
| 51 | [engine/tests/config_env.rs](../crates/engine/tests/config_env.rs) | 环境变量与配置优先级、私有 Key 保存及重载、TOML 不写入明文 Key、默认预算和不限额配置的往返一致性。 | `serde_json`、`uuid`；通过标准库子进程隔离环境。 |
| 52 | [engine/tests/evidence.rs](../crates/engine/tests/evidence.rs) | 人造 Office ZIP/XML 的有限提取、共享字符串、按编号的幻灯片部分取样、损坏/加密/旧格式/XML 实体处理；视频取样时刻及不同 API 的图像格式。 | `zip`、`uuid`、`tokio-util`。 |
| 53 | [engine/tests/runtime_safety.rs](../crates/engine/tests/runtime_safety.rs) | 不授权时不泄露名称、内容切片上限、路径越界拒绝、证据快照变化、禁止覆盖，以及未知工具/重复调用标识/非法参数拒绝。 | `tokio-util`、`serde_json`、`uuid`。 |
| 54 | [engine/tests/job_recovery.rs](../crates/engine/tests/job_recovery.rs) | 重启恢复与过期/损坏检查点拒绝；旧配置摘要兼容；中断后不重复移动；取消时不发布半个归档；复用持久化响应而不再次联网或重复累计用量。 | `tokio`、`tokio-util`、`serde_json`、`uuid`、`blake3`、`chrono`。 |
| 55 | [engine/tests/archive_memory_cleanup.rs](../crates/engine/tests/archive_memory_cleanup.rs) | 归档状态与轨迹完整性、导入只读、历史选择的场景和预算约束、清理候选保护与只建议不改文件。 | `tokio-util`、`serde_json`、`uuid`。 |
| 56 | [engine/tests/pricing.rs](../crates/engine/tests/pricing.rs) | 缓存计费、上下文整请求档位、时段与周末规则、缓存写入分类、手动零价格、价格历史、未知价格及币种汇总。 | `serde_json`、`chrono`。 |
| 57 | [engine/tests/unix_compat.rs](../crates/engine/tests/unix_compat.rs) | Unix 文件名身份、符号链接拒绝、不覆盖移动与私有环境文件权限。受平台条件控制，在 Windows 上不执行这些 Unix 分支。 | `uuid`；Unix 接口使用标准库及被测引擎。 |
| 58 | [engine/src/recycle/tests.rs](../crates/engine/src/recycle/tests.rs) | 回收模块的内部测试：用假回收站模拟选择注入、文件变化、进程回复丢失、重名、取消及重启撤销；另有可选的真实系统回收往返测试。虽然放在 `src` 中，但由测试条件编译，不属于正式业务执行。 | 经父模块使用 `anyhow`、`uuid`、`tokio-util` 等；文件夹和假回收站操作主要用标准库。 |

## 八、Web 与前端集成测试（16 个）

多数 HTTP 测试通过共同的 `support` 启动真实 `ds-web` 可执行文件，连接本地假 LLM。能验证 API 协议、任务流程及错误处理，不需要付费调用真实模型；这些测试不证明真实模型的分类质量。

| # | 文件 | 主要验证什么 | 第三方 crate |
| ---: | --- | --- | --- |
| 59 | [web/tests/workflow.rs](../crates/web/tests/workflow.rs) | HTTP 层完整工作流、会话与来源保护、搜索和计费、执行/回滚；重生命名保留扩展名和不变名称，失败/取消/返回时保留计划选择。 | `tokio`、`serde_json`；共享 `support`。 |
| 60 | [web/tests/desktop.rs](../crates/web/tests/desktop.rs) | 桌面六阶段、用户选择的移动、自定义目录树和 Agent、容器复用、文件示例、整体文件夹证据权限及恢复。 | `tokio`、`serde_json`；共享 `support`。 |
| 61 | [web/tests/model_connections.rs](../crates/web/tests/model_connections.rs) | 不同厂商协议的模型发现、私有 Key 持久化、价格记录不随后续设置变化而被追溯改写。 | `tokio`、`serde_json`；共享 `support`。 |
| 62 | [web/tests/runtime.rs](../crates/web/tests/runtime.rs) | 请求并发上限、大上下文组批、重生命名检查点和预算预留；防止并行请求共同突破预算约束。 | `tokio`、`serde_json`；共享 `support`。 |
| 63 | [web/tests/classification.rs](../crates/web/tests/classification.rs) | 分类组批、断点恢复、输出截断、自适应上下文、证据授权、输出上限与非法工具调用。 | `tokio`、`serde_json`；共享 `support`。 |
| 64 | [web/tests/inspection.rs](../crates/web/tests/inspection.rs) | 总览与按类别分批分析、摘要上限、恢复时复用结果、权限改变后旧分析结果失效。 | `tokio`、`serde_json`；共享 `support`。 |
| 65 | [web/tests/proposals.rs](../crates/web/tests/proposals.rs) | 无效 AI 回答仍记录实际用量但不发布修改；局部树编辑、建议依赖、一级目录新增、重复模板节点协调。 | `tokio`、`serde_json`；共享 `support`。 |
| 66 | [web/tests/review.rs](../crates/web/tests/review.rs) | 审查反馈对规则、归属和原位保留的调整；失败时原子拒绝；恢复时不重复请求已完成分类；并行进度只统计完成文件且不倒退。 | `tokio`、`serde_json`；共享 `support`。 |
| 67 | [web/tests/checkpoints.rs](../crates/web/tests/checkpoints.rs) | 杀掉并重启服务后的任务恢复、持久暂停/继续、过期状态阻断及归档任务处理。 | `tokio`、`serde_json`；共享 `support`。 |
| 68 | [web/tests/archives.rs](../crates/web/tests/archives.rs) | 完整历史归档的导出/导入、校验和、私有信息与清理批次等任务数据保存。 | `tokio`、`serde_json`；共享 `support`。 |
| 69 | [web/tests/recycle.rs](../crates/web/tests/recycle.rs) | 用户确认后的系统回收、重启后撤销，以及回收恢复与整理回滚之间的顺序。涉及真实系统能力的测试需满足相应运行条件。 | `tokio`、`serde_json`、`futures`、`tempfile`；共享 `support`。 |
| 70 | [web/tests/media.rs](../crates/web/tests/media.rs) | 原生 PDF 的渐进返回、Unicode 路径、页码去重、快照和大小限制；不依赖 FFmpeg 的原生视频路径；Office/图片/PDF/视频在三种模型协议中的表达、权限和工具映射。另测可用时的 FFmpeg 后备路径。 | `tokio`、`serde_json`、`image`、`base64`、`tempfile`；共享 `support`。 |
| 71 | [web/tests/frontend.rs](../crates/web/tests/frontend.rs) | 用 Rust 内的 JavaScript 引擎执行实际前端模块：语法、行型节点布局、折叠和孤立节点拖动、权限预设、建议依赖、并行显示数据和归档 JSON 精度。它是前端逻辑测试，不是浏览器截图/像素排版测试。 | `boa_engine`：执行 JavaScript；`serde_json`：结果断言。 |
| 72 | [web/tests/support/mod.rs](../crates/web/tests/support/mod.rs) | **HTTP 测试公共设施。** 创建隔离配置和临时目录，启动/停止/重启真实服务器，发请求、等待后台结果、推进阶段并比较文件哈希；避免继承真实模型 Key。 | `tempfile`、`reqwest`、`tokio`、`serde_json`、`sha2`、`walkdir`。 |
| 73 | [web/tests/support/mock.rs](../crates/web/tests/support/mock.rs) | **本地假 LLM 服务。** 返回可控模型列表、三种协议响应、工具调用和错误，模拟延迟、截断等情况，记录请求与最大并发，供集成测试断言。 | `axum`：假 HTTP 服务；`tokio`：异步和延迟；`serde_json`：模拟响应。 |
| 74 | [web/tests/support/media.rs](../crates/web/tests/support/media.rs) | **测试媒体生成器。** 本地构造最小 PDF、PNG 和 Office ZIP 文件，为媒体测试提供可重复、无隐私内容的输入。 | `image`：PNG；`zip`：Office 容器；PDF 测试字节由本地函数构造。 |

## 九、CLI 与开发工具测试（3 个）

| # | 文件 | 主要验证什么 | 第三方 crate |
| ---: | --- | --- | --- |
| 75 | [cli/tests/cli.rs](../crates/cli/tests/cli.rs) | 启动真实 `ds` 命令，验证不接 LLM 的规则整理、审查、执行、回滚，以及归档导入只读；确认 CLI 确实使用共享业务流程。 | `tempfile`、`serde_json`；启动子进程用标准库。 |
| 76 | [dev/tests/share.rs](../crates/dev/tests/share.rs) | 检查器能发现源码、配置、二进制 UTF-16、ZIP 和暂存区里的凭据；验证错误输出不泄露 Key、工作区删除文件后暂存区仍检查，以及畸形包处理。 | `tempfile`、`zip`；Git 测试通过标准库启动本地 Git。 |
| 77 | [dev/tests/sync.rs](../crates/dev/tests/sync.rs) | 同步前冲突预检、只清理受管理旧文件、保留私有和非托管文件、检查模式不写入、恢复中断复制，以及恶意清单和链接拒绝。 | `tempfile`、`serde_json`。 |

## 第三方 crate 的整体职责

此表方便理解选型；具体哪个文件使用它，以上面的逐文件表为准。

| crate | 在本项目中的作用 |
| --- | --- |
| `tokio` / `tokio-util` / `futures` / `tokio-stream` | 异步 HTTP、并发调度、取消通知、通道和 SSE 事件流。异步并发主要减少网络等待；阻塞工作需另行调度，不等同于每个任务都增加一个线程。 |
| `serde` / `serde_json` / `toml` | 任务、轨迹、模型消息、配置和归档的数据表达。 |
| `anyhow` / `thiserror` | 前者处理带上下文的错误；后者用于需要被明确区分的错误类型。 |
| `reqwest` / `axum` | 前者访问模型及搜索 API，后者提供本地 Web/API 服务。 |
| `uuid` / `chrono` | 唯一标识与时间记录。 |
| `walkdir` / `dirs` / `dotenvy` | 目录遍历、系统默认目录定位、环境文件加载。 |
| `blake3` / `sha2` | 文件与状态摘要、完整性检查及旧记录兼容；不是把文件加密存储。 |
| `image` / `zip` / `quick-xml` / `flate2` | 图片处理、Office 容器与 XML 摘录、DEFLATE 解压后端；`flate2` 虽无直接函数调用，仍有 Cargo feature 配置用途。 |
| `windows` / `windows-core` / `windows-sys` / `libc` | 系统原生媒体、回收站、COM、文件句柄及平台安全文件操作。各自对应不同层次的系统接口。 |
| `trash` | 回收站条目枚举和恢复等能力；Windows 回收动作还配合自有 Shell 保护逻辑。 |
| `tauri` / `tauri-build` / `cc` | 可选桌面外壳及构建；`cc` 编译 macOS 的少量 Objective-C 桥接。 |
| `regex` / `tempfile` | 开发检查器的规则匹配、可靠临时文件，以及测试隔离。 |
| `boa_engine` / `base64` | 前者让 Rust 测试执行真实前端 JavaScript，后者在媒体测试中解码图像载荷；不是核心引擎必须依赖的通用 Agent 框架。 |

`crates/web/Cargo.toml` 还声明了测试依赖 `tower`、`http-body-util`，但本次逐文件核对未发现现有 `.rs` 测试直接引用它们。因此这里不为它们虚构文件职责；“清理后保留的源码”也不意味着依赖声明已经完全没有进一步整理空间。精确依赖版本及传递依赖以 `Cargo.toml` 和 `Cargo.lock` 为准。

## 阅读顺序与当前边界

如果想先掌握业务，建议依次阅读：`workflow.rs` → `workflow_ai.rs` → `workflow_ai/classification.rs` → `ai_runtime.rs` → `safe_fs.rs` → `web/src/lib.rs`，其余模块在遇到调用时再展开。

几个容易混淆的边界：

1. **节点画布、按钮、浮窗等前端表现主要是 JavaScript/CSS/HTML。** Rust 管理节点数据、规则、权限和计划校验；本清单仅盘点 `.rs`，不代表全部 GUI 都写在 Rust 中。
2. **macOS 原生桥接还包含 `.m` 文件。** 它们调用 Apple 系统框架，Rust 继续负责流程与权限控制；这不改变核心业务用 Rust 的设计。
3. **恢复的是持久化任务、检查点、操作记录和已保存响应。** 不是冻结并还原整个进程内存，也不能保证所有中断中的远端调用都可免成本重试。
4. **测试文件不会作为普通功能编入正式程序。** 部分正式文件另有内嵌测试，因此“29 个测试及辅助文件”是文件计数，不是测试用例数；本次源码说明未重新运行测试。
5. **小文件不等于冗余，保留文件也不等于再无可拆分之处。** `domain.rs`、`tree.rs` 负责共享类型；目前较集中的 `workflow.rs`、`web/src/lib.rs`、`classification.rs` 则是主要维护入口，可按需求继续细分，而不是直接删除。
