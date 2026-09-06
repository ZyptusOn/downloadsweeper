# 模型发现与服务商适配

在模型设置中选择服务商，再输入密钥。输入停止约 700 ms 后，浏览器向本机 Rust 服务发送只读模型发现请求，Rust 使用指定地址查询 `/models`。只查询列表，不产生生成任务、不保存输入密钥。已有配置的密钥仅允许在相同 API 基础地址复用；自动查询不会尝试其他服务商或跟随重定向。

选择列表中的模型会带入已知上下文、视觉能力，并限制输出预算；也可手动输入 ID、上下文和输出长度。未知信息显示“未知”，不通过模型名猜测上下文。API 返回的上下文、输出限制优先于内置预设。列表只能说明服务端列出了模型，不保证额度、账号权限和实际生成成功。

`/models` 不存在、拒绝鉴权或网络超时时，保留可选官方预设，并明确标注未验证账号可用性。预设核对日期为 2026-09-05，数据位于 `crates/engine/src/llm/model_catalog.json`，更新预设无需修改界面。文档只给出 K/M 的条目按十进制保守换算；明确给出整数 token 的条目保留原值。不同地域、中转平台和模型快照可能采用更小上限，应优先使用服务端能力并允许手动调整。

## 协议与模型差异

| 服务商 / 模型 | 适配内容 | 官方来源 |
| --- | --- | --- |
| DeepSeek V4 Flash / Pro / Flash Vision Exp | `thinking.type` 控制思考；思考时移除采样温度、使用 `reasoning_effort=high`，工具轮次回传 `reasoning_content`；Vision Exp 支持获准的图像输入 | [模型规格](https://api-docs.deepseek.com/quick_start/pricing/)、[思考与工具调用](https://api-docs.deepseek.com/guides/thinking_mode/) |
| GLM-5 / 5.1 / 5.2 | 使用 `thinking.type`，不把通用 `reasoning_effort` 发给不支持它的旧版本；5/5.1 为 200K 预设，5.2 为 1M | [模型概览](https://docs.bigmodel.cn/cn/guide/start/model-overview)、[GLM-5.2](https://docs.bigmodel.cn/cn/guide/models/text/glm-5.2) |
| MiMo-V2.5 / Pro | 官方地址使用 `api-key` 鉴权头；`max_completion_tokens`、`thinking.type`；保留工具轮次的推理续接字段，思考时省略温度 | [接口](https://mimo.mi.com/docs/en-US/api/chat/openai-api)、[模型规格](https://mimo.mi.com/docs/en-US/quick-start/model) |
| LongCat-2.0 | 正确的 `/openai/v1` 路径；思考开关；1M 上下文、131072 最大输出 | [模型列表](https://longcat.chat/platform/docs/zh/api/models.html)、[Chat 接口](https://longcat.chat/platform/docs/api/chat.html)、[快速开始](https://longcat.chat/platform/docs/zh/) |
| 混元 Hy3 / Hy4 Preview | TokenHub `/v1/models`；思考参数；分别提供上下文和最大输入限制，避免误将全部上下文用于输入 | [模型规格](https://cloud.tencent.com/document/product/1823/130051)、[API](https://cloud.tencent.com/document/product/1823/130078)、[请求参数](https://cloud.tencent.com/document/product/1823/135872) |
| GPT-5 及以后 | 官方 OpenAI 自动走 Responses；工具定义、function_call_output、加密推理续接、图像和 usage 转换。兼容模式使用 max_completion_tokens，省略不支持的采样字段。关闭思考时 GPT-5 使用 minimal，支持 none 的已知后续型号使用 none，GPT-6 Astra 使用 low | [GPT-5](https://developers.openai.com/api/docs/models/gpt-5)、[模型对比](https://developers.openai.com/api/docs/models/compare)、[当前接入指南](https://developers.openai.com/api/docs/guides/latest-model) |
| 较新 Claude | 原生 `/v1/messages`、`x-api-key`、版本头，原生模型列表分页与能力解析；system、图像、tool_use/tool_result、签名思考块续接。新版使用 adaptive，旧版使用有界 thinking budget；始终思考的 Fable 用 effort 调整 | [模型概览](https://platform.claude.com/docs/en/models/overview)、[模型列表 API](https://platform.claude.com/docs/en/api/models/list)、[思考控制](https://platform.claude.com/docs/en/build-with-claude/thinking-steering-and-cost) |

自定义代理默认采用 Chat Completions，支持手动切换 Responses 或 Anthropic Messages。API 格式由端点/用户设置决定，鉴权头由服务商端点和协议决定；不会仅凭模型名把中转密钥发送给模型原厂。未核实的未来型号可从 API 动态出现，但不会继承一份猜测的上下文预设。

## 密钥与计费

保存时新密钥追加到与配置同目录的私有 `.env`，每次生成独立变量名；配置仅记录 `api_key_env`。已有 `.env` 内容保留，无需修改全局环境。显式变量名只读取该变量；没有指定时才兼容 `DS_API_KEY`、`OPENAI_API_KEY` 和旧内联 key。相同变量名的进程环境优先于 `.env`。手动编辑 `.env` 后需重启；通过 UI 保存立即生效。解除绑定不删除旧变量，可自行清理不用的私有凭据。

模型发现不发送生成请求；“测试已保存的连接”会发送一条简短生成请求，并记录实际 usage。工具轮次会保留供应商要求的续接数据；输入估算包含这些数据，但不会把估算当成实测 token。Claude 输入 usage 计数包含普通输入、缓存写入与缓存读取，计费时分别计算，避免重复收费。默认匹配已核实的官方价目表，也可切换手动单价与币种。详细价格依据、档位与限制见 `PRICING.md`；结果是模型费用估算，不是供应商账单。

发布前运行 `python scripts/verify_share.py`。本机 `.env` 不可分享。协议回归使用本地模拟服务：`cargo test --workspace --exclude ds-tauri --locked`、`cargo build -p ds-web --locked`、`python scripts/model_connection_regression.py`。覆盖列表元数据、未知模型、鉴权、分页、错误脱敏、重定向隔离、跨端点密钥隔离、三种协议的实际 HTTP 请求与用量解析；不代表每个账号都已通过实网生成验证。

所有工作流生成请求通过统一运行时，共用连接池、并行额度与任务预算预留。可在设置中调整并行请求数（默认 3，范围 1–8）和分类工具轮次（默认 12，范围 1–32）；独立批次利用配置的上下文扩容。未确认用量单独显示并保留预留，详细行为见 `AGENT_RUNTIME.md`；本地验证为 `python scripts/runtime_regression.py`。
