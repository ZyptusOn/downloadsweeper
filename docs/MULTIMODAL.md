# 轻量内容预览与并发

先在模型设置启用视觉能力，再在任务「读取权限」中逐格式授权。默认权限不会因为升级而放宽。图片、视频和 PDF 页面需要「图像」或「内容切片」权限；Office 文本必须为「内容切片」，不依赖视觉模型。已知不支持图像的模型不会收到图片。未知自定义模型由用户确认能力。

## PDF

Windows 使用 Windows.Data.Pdf，macOS 使用 CoreGraphics，从已有的安全读取链路进入独立子进程；不需要安装 PDF 阅读器、Office、Python 或额外渲染库。最多采样首页、第二页和中间页，页码去重并按阅读顺序排列。返回 sampled_pages（从 1 开始）、page_count、partial、elapsed_ms；三页按从左至右合成一个 JPEG 图像块。每页最多 512×768，整体最多 1536×768，JPEG 最多 384 KiB；适用接口采用 high detail 以保留文字。实际视觉 token 计入 API 用量，预算估算也为文档预览预留更多输入空间。

单文件最多 64 MiB，原生进程最长 8 秒；macOS 数据提供器累计读取最多 64 MiB，只读取继承的已核验描述符。Windows 的中间位图可能受显示缩放影响，限制最大 2048×3072、解码分配 48 MiB，再缩到页面上限。逐页发布完整快照，后续页面卡住时可保留已完成页面；用户主动取消则停止返回。只渲染页面，不执行文档脚本、链接、附件或 OCR，不提取完整文本。加密、损坏、超限或不支持的平台返回不可用，沿用授权的文件信息；不自动上传原 PDF。

PDF 已接入目录建议、分类证据工具与文件名重生。工具提示会区分 pdf_pages 与 video_frames；仅在名称信息不足且权限允许时使用，不为了预览而额外增加独立模型请求。Windows 已用真实合成 PDF 和三种模拟 API 协议验收；macOS 代码加入 Apple Silicon CI，仍需实机验证。

接口依据：[Windows PDF 页渲染](https://learn.microsoft.com/en-us/uwp/api/windows.data.pdf.pdfpage.rendertostreamasync)、[CoreGraphics PDF 绘制](https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/drawingwithquartz2d/dq_pdf/dq_pdf.html)。验证：`cargo test --locked -p ds-web --test media native_pdf_progressive_ipc_unicode_dedup_snapshot_and_bounds`、`cargo test --locked -p ds-web --test media pdf_agent_tool_evidence_mapping_and_permission_gates`。

## 视频

支持 mp4/m4v/mov、mkv/webm、avi。Windows 10/11 优先通过系统 Windows.Media.Editing 接口解码；macOS 已加入 AVAssetImageGenerator 后端代码（待实机验收），打开一次视频，缩放采样开头、约 1 秒以内及中段附近的关键帧；短片自动合并太近的时间点。最多三帧拼成一张 512×512 JPEG，按左上、右上、左下排列。时间是定位目标，关键帧时间可能提前或落在同一帧。空白格不是帧。不上传视频原件，不提取音轨，不使用云端文件上传服务。

只沿用原来的图像证据和请求数量上限。与纯文件名相比，启用任何内容预览都可能增加输入 token；不承诺模型账单绝对不变。三帧拼图把视频维持为一个图像块，尽量控制传输和视觉 token。小字、快速运动和未采样内容不能据此识别。

Windows 便携包已经包含原生解码调用代码，无需额外安装 FFmpeg。具体编码支持取决于 Windows 媒体组件和已安装编解码器，不能仅凭扩展名保证可读；Windows N 缺少媒体组件或缺少相应编码扩展时可能不可用。设置页显示原生后端及 FFmpeg 后备状态，原生后端标识代表程序支持调用，并非已逐个测试系统编解码器。

原生解码失败、超时或不支持该格式时，程序自动寻找可执行文件同目录或 PATH 内的 ffmpeg/ffprobe 作为后备。macOS 原生后端使用已验证的文件描述符、自定义资源读取和外部引用禁用策略，详见 MACOS.md；Linux 沿用 FFmpeg 路径。需要后备时，可从 [FFmpeg 官方下载入口](https://ffmpeg.org/download.html) 获取构建。没有可用解码器时只使用已授权的文件信息；只有 ffmpeg 没有 ffprobe 时仅采样开头。不会自动安装工具或扩展。

处理时保留已经核对文件身份的只读句柄；Windows 阻止同时写入/替换，Unix 的 FFmpeg 从已有句柄读取。原生解码在本程序的独立子进程运行，入口在加载配置与 .env 之前；再次验证文件快照和容器头，只把本地文件交给系统 API。macOS 后端不重新打开原路径，使用继承的文件句柄读取，累计读取最多 64MiB，并记录实际采样时间。原生任务整体限 8 秒，原始视频尺寸最多 8192×8192，输出缩略帧最多 256KiB。返回管道最多 3 MiB，包含逐步生成的完整 JSON 快照；Windows 每取得一帧就发布，后续帧超时仍可保留前面成功的帧。macOS 允许快速近似定位，等待超时后保留已完成回调的帧。partial 标记不完整采样，elapsed_ms 记录实际本地耗时。子进程崩溃或卡住不会拖住服务，取消后终止并回收；同一程序同时最多 3 个本地预览工作位。

FFmpeg 后备每个进程限单解码线程、探测量和输出字节数，限定输入容器与 file/pipe 协议；探测最多 2 秒，单次抽帧最多 3 秒，所有后备步骤共享从原生尝试开始的 12 秒总等待预算，避免逐项超时累加至约 24 秒。已有部分原生帧时直接使用，不再次解码。两类媒体子进程均清空环境变量，Windows 仅保留 SystemRoot，不继承模型密钥或代理凭据。JPEG 通过有界内存管道传递，应用不保存临时帧；系统媒体组件自行管理的缓存不受应用控制。总等待预算不含排队、系统清理与最终 JPEG 编码时间。

原生接口参考：[Microsoft GetThumbnailAsync](https://learn.microsoft.com/en-us/uwp/api/windows.media.editing.mediacomposition.getthumbnailasync?view=winrt-26100)。使用 NearestKeyFrame 快速定位，单边缩放以保留宽高比；不承诺精确帧时间或必然启用硬件加速。

## Office

支持 docx/docm、pptx/pptm、xlsx/xlsm 的容器文本。无需安装 Office、LibreOffice 或 Python。doc/docx、xls/xlsx 的格式不可仅通过改后缀转换；旧版 doc/xls/ppt、加密或损坏文件返回不可用，继续使用已获准的名称信息。

Word 采样 document.xml；幻灯片采样前三个编号正文部件；Excel 采样前两个编号工作表并解析所引用的共享字符串。编号顺序不一定等于用户重排后的显示顺序。只读 XML 与缓存单元格值，不执行宏或公式，不访问外部链接，不解包到磁盘，不发送未引用的共享字符串。图表、嵌入文件、扫描图片、格式与完整分页不在该预览范围。

先检查 ZIP 中央目录上限（最多 4096 项、2MiB 索引，不处理 ZIP64），每个被选 XML 最多解压 256KiB，总体最多四个部件；解析有时间界限。实际发出的 UTF-8 文本受用户切片上限约束，分类工具进一步限制为 1024 字节。结果注明 sampled_parts、bytes 和 truncated，不能当作完整文档。

## 接口与工具行为

统一生成 JPEG，然后分别编码为 Chat Completions 的 image_url、Responses 的 input_image，或 Anthropic 的 base64 image/source。BigModel 原生地址使用其文档示例的裸 base64；其他兼容端点使用带 MIME 的 data URI。仅向适用接口添加低细节参数，不向其他厂商混入专属字段。参考：[OpenAI](https://developers.openai.com/api/docs/guides/images-vision)、[Claude](https://platform.claude.com/docs/en/build-with-claude/vision)、[DeepSeek](https://api-docs.deepseek.com/guides/vision/)、[GLM](https://docs.bigmodel.cn/cn/guide/models/vlm/glm-4.6v)、[MiMo](https://mimo.mi.com/docs/en-US/quick-start/usage-guide/multimodal-understanding/image-understanding)。协议编码已用本地模拟服务验证；不代表每个账号的模型均有视觉权限。

分类工具合并读取多个文件，只接受本批 ID；并行读结果保持文件与图像顺序。工具返回明确的 ok / unavailable / already_read / error 状态、原因及后续动作。重复证据不再读取；内容中的指令不改变任务授权。所有实际文件操作仍在用户审查后的 Rust 执行器中完成。

## 提速与费用

本地有 3 个受限预览工作位，文本和 Office 读取、图片缩放放在阻塞工作线程，避免占用异步运行时。一个证据工具调用中的独立文件可并行读取。API 请求继续由统一并发上限（1–8，默认 3）管理，现有分类批次、类型摘要顺序及检查点保留，不为了填满并发拆小任务，不重复付费请求，不自动重试。

验证：`cargo test -p ds-engine --test evidence --locked`、`cargo test -p ds-engine --lib --locked`、`cargo test --locked -p ds-web --test media`、`cargo test --locked -p ds-web --test runtime`。原生测试直接使用仓库内合成 H.264 视频，从服务器 PATH 排除外部解码工具，并断言实际后端为 windows_media 或 avfoundation；Mac 的 Homebrew 后备仍可能被发现，实际后端断言确保没有使用它。可选后备测试将 `DS_TEST_MEDIA_BIN` 指向 FFmpeg 目录，再运行 `cargo test --locked -p ds-web --test media ffmpeg_fallback_protocols -- --ignored`，以系统不支持的 FFV1 合成视频验证后备。所有测试由 Rust 执行，均不调用真实 LLM。
