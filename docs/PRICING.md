# 自动与手动计费

默认选择“自动使用官方价格”。无需填写单价，程序根据实际服务地址、模型 ID、账号结算币种、请求发送时间和服务返回的 usage 计算模型费用。设置页显示匹配结果、分档条件、价格依据和核对日期。

官方价目表核对日期：2026-09-05。价目表随程序发布，保存在 `crates/engine/src/pricing_catalog.json`；不是每次启动自动抓取网页。官方调价后应更新价目表/程序，或临时使用手动价。过期价格风险会随着核对日期变旧而增加，不能把本地估算当作实时账单。

## 支持规则与来源

| 服务 | 已接入的计费规则 | 官方来源 |
| --- | --- | --- |
| DeepSeek V4 Flash / Pro / Flash Vision Exp | 普通输入、缓存命中、输出；工作日 UTC 01–04、06–10 的高峰费率；人民币与美元各自的官方价 | [人民币](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)、[美元](https://api-docs.deepseek.com/quick_start/pricing/) |
| GLM-5 / 5.1 / 5-Turbo / 5.2 / 5.3 | 国内服务的输入长度分档与缓存；Z.ai 国际端点使用其独立美元定价 | [BigModel](https://bigmodel.cn/pricing)、[Z.ai](https://docs.z.ai/guides/overview/pricing) |
| MiMo-V2.5 / Pro | 缓存命中、未命中和输出；国内与海外分别选官方人民币或美元价 | [MiMo API 定价](https://mimo.mi.com/docs/zh-CN/price/pay-as-you-go) |
| LongCat-2.0 | 缓存命中与当前限时折扣；人民币或美元 | [LongCat 定价](https://longcat.chat/platform/docs/zh/pricing/longcat-2.0) |
| 混元 Hy3 / Hy3 Preview / Hy4 Preview | TokenHub 国内与国际端点各自的输入、输出、缓存价 | [国内](https://cloud.tencent.com/document/product/1823/130055)、[国际](https://intl.cloud.tencent.com/zh/document/product/1300/78937) |
| GPT-5、5 mini/nano、5.1、5.2、5.4及mini/nano、5.5、5.6系列、6 Astra | 标准在线请求、缓存；适用模型超过272K输入的长上下文价；5.6/6缓存写入 | [总价目表](https://developers.openai.com/api/docs/pricing)、[模型说明](https://developers.openai.com/api/docs/models/gpt-5.6-sol) |
| 较新 Claude Opus / Sonnet / Haiku / Fable | 普通输入、缓存读取、5分钟/1小时写入分别计费；已核实的新版本长上下文使用标准价 | [Claude 定价](https://platform.claude.com/docs/en/about-claude/pricing) |

不同币种分别汇总，不使用固定汇率换算。多币种服务请选择与账号结算地区一致的币种。原厂价格只匹配已核实的官方地址及模型，不因中转模型名相同就使用原厂价。订阅/Coding Plan 的额度和免费赠金无法由 token usage 推导；自动价仅提供按量计费参考。未知模型、未支持的服务档位或缓存写入价格显示“价格未知”，不等于免费，也不会阻止整理任务。

输入 token 总量包含缓存部分；计算时扣除缓存读取和写入后，再按普通输入价计算其余部分，防止重复。输出计数已经包括供应商计入的思考 token，不再重复加算。若只返回输入/输出总量而缺缓存明细，会按普通输入价暂估，并单独标注；矛盾的缓存计数不计算金额，保留原始 usage。

## 手动覆盖

选择“手动输入单价”，填写普通输入和输出价格；可选填写缓存读取、5分钟/通用缓存写入、1小时缓存写入价格。GUI 单位是所选币种 / 百万 token，配置文件兼容字段单位仍是每千 token。缓存留空沿用普通输入价，明确填写 0 表示免费。手动模式优先于官方价，不再叠加官方长上下文或峰谷倍率。

旧 `input_per_1k_usd`、`output_per_1k_usd` 字段名为兼容而保留；手动模式下数值使用 `currency` 指定的币种。旧配置中的单价数值会保留，默认新计费模式为 `auto`；需要沿用原单价时切换 `manual`。

## 历史与估算范围

每次成功取得完整 usage 后，保存当次金额、币种、应用单价、来源链接、核对日期、请求时刻与原始 usage。修改模型、币种、手动价格或更新程序，不重算历史调用。旧记录保留原美元金额并标注历史单价；旧记录的 0 无法区分免费与未配置，显示价格未知。

峰谷时段按本机请求发送时刻估算；服务端实际接收时刻可能跨越边界。GPT-5.4/5.5 官方说明按完整 session 应用长上下文加价，本程序使用独立无服务器保存请求，按本次完整输入判断；后续人为压缩上下文时不能据此精确还原供应商会话账单。金额不包含税费、充值赠金、协议折扣、订阅抵扣、独立联网搜索费用。官方限时优惠没有明确终止日的条目会保留说明，最终以平台结算为准。

验证：`cargo test -p ds-engine --test pricing --locked`、`cargo test --locked -p ds-web --test model_connections`。覆盖峰谷边界与周末、整请求分档、缓存拆分、手动零价、币种隔离、未知价格、非法参数和历史不变。
