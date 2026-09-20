# 04 · 第三方集成调研与选型

> 调研日期：2026-09-20（联网核实官方文档与定价页）。**价格与模型名变化极快**，接入前以官方页面为准；标「未核实」的条目只有单一或二手来源。
> 设计前提：所有供应商都在适配器后面（见 [03-architecture.md](03-architecture.md) §5），下表的「默认」只是出厂预设，用户可在设置里更换。

## 0. 选型总览

| 能力 | 出厂默认 | 备选 | 单次成本量级 | 调用形态 |
|------|----------|------|--------------|----------|
| 调研大脑（LLM） | DeepSeek `deepseek-flash` | 审校步骤用 `deepseek-v4-pro`；任何 OpenAI 兼容模型 | 一次调研 ≈ ¥0.3~0.5 | 同步 / 流式 |
| 联网搜索 | 智谱 Web Search `search_std` | 火山「豆包搜索」、博查 | ¥0.01~0.036 / 次 | 同步 |
| 截图识别（视觉） | `deepseek-flash`（同一把密钥） | qwen-vl-ocr、qwen3-vl-flash、GLM-4.6V-Flash（免费） | 可忽略 | 同步 |
| 建模参考图 / 场景图 | MiniMax `image-01` | Seedream 5.0 lite、qwen-image-3.0 | ¥0.025 / 张（备选 ¥0.18~0.30） | 同步，URL 24h 过期 |
| 实拍图换背景 | 抠图 + 百炼 `wanx-background-generation-v2`（**主体像素不变**） | Seedream 5.0 pro 定位编辑、qwen-image-edit-plus | ¥0.08 / 张 | 异步 |
| **看图 → 代码建模（3D 主路线）** | `deepseek-flash` 看图出规格与复核 + `deepseek-v4-pro`（思考）写 build123d 脚本；**本机引擎执行** | 任何 OpenAI 兼容的视觉 / 代码模型 | ≈ ¥0.1 / 个模型；改参数 ¥0 | 同步；本机执行约 4 s / 次 |
| 图生 3D 网格（有机造型，延后） | Tripo H3.1（只出白模） | 腾讯混元生 3D、火山方舟上的 Seed3D / Hyper3D / Hi3D | ≈ ¥1.5 / 次（白模）+ 转换 ¥0.4~0.7 | 异步，需轮询 |
| 切片软件交接 | 用本地文件直接调起已安装的切片软件 | — | 0 | 本地进程 |
| 销售渠道 | 见 §7 | | | |

**单个想法的 AI 成本估算**（走到「发布包」）：调研 ≈ ¥0.7 + 出图 ≈ ¥0.8 + 3D ≈ ¥0.1~0.5（代码建模，含几轮修复与复核；改参数不花钱）+ 文案 ≈ ¥0.1 → **约 ¥2**；在调研阶段就被淘汰的想法 < ¥1。走网格路线的有机造型另算：Tripo 白模约 ¥1.5 / 次，平均 2 次 ≈ ¥4。

## 1. LLM：DeepSeek

| 项 | 结论（官方文档） |
|----|------------------|
| 模型 | `deepseek-flash` → DeepSeek-V4.1-Flash（2026-09-10 发布）；`deepseek-v4-pro` → V4-Pro。**旧名 `deepseek-chat` / `deepseek-reasoner` 已按公告于 2026-07-24 停用，不要再用** |
| 价格（元 / 百万 token，高峰 / 空闲） | flash：缓存命中输入 0.04 / 0.02，未命中输入 2 / 1，输出 8 / 4；pro：0.30 / 0.15，9 / 4.5，27 / 13.5。高峰 = 工作日 9–12 点、14–18 点 |
| 规格 | 上下文 1M，最大输出 384K；默认思考模式，可关闭，强度 low / high / max |
| 工具调用 | 支持（思考模式下需回传 `reasoning_content`）；strict 模式为 Beta |
| 结构化输出 | 只有 `json_object`，**没有 `json_schema`** → 我们自己做 schema 校验 + 失败重试一次 |
| 视觉 | 仅 flash 支持：OpenAI 风格的内容块 `{"type":"image_url","image_url":{"url":"data:image/jpeg;base64,…","detail":"auto"}}`；每张图最多计 1024 token；请求体上限 32 MiB → 截图识别、看图建模都不用引入第二家。**v4-pro 看不了图**，所以「看图」和「写代码」是两个模型 |
| 长回答 | 非流式请求在等待期间**持续返回空行保活**；「10 分钟还没开始推理」服务端会断开 → 客户端总超时 600 s + 读超时 120 s（读超时才是「连接坏了」的判据） |
| 联网搜索 | **API 不内置**，必须自接搜索 |
| 限流 | 按账号并发：flash 2500、pro 500，超限 429 |
| 接入 | `https://api.deepseek.com`（OpenAI 格式）；另有 Anthropic 兼容端点 |

**落地要点**：调研流水线各步用 flash，最后的「审校」一步用 v4-pro；看图建模里 flash 负责看图（出规格、复核），v4-pro 开思考负责写代码与修代码（思维链计入 `max_tokens`，现给 16K）；批量任务（趋势雷达、周报）排到空闲时段，价格减半；模型名做成配置项而不是常量——DeepSeek 一年内已经换过一轮命名。

## 2. 联网搜索

| 服务 | 价格 | 说明 |
|------|------|------|
| 智谱 Web Search | `search_std` ¥0.01 / 次，`search_pro` ¥0.03，sogou / quark ¥0.05 | 最便宜；支持 MCP |
| 火山「豆包搜索」 | 订阅 ¥5.9 / 1000 次；按量 ¥0.020 / 次 | |
| 博查 | Web Search ¥0.036 / 次，AI Search ¥0.060 | `api.bocha.cn/v1/web-search` |
| 阿里百炼内置搜索 | ¥3~4 / 千次 | 与 Qwen 模型绑定使用 |
| Tavily / Jina Reader | — | 大陆直连不稳，**不作为主依赖** |

**重要限制**：小红书的 robots.txt 对除百度、必应、360、搜狗之外的爬虫全部 Disallow，上述 API 都没有声明覆盖小红书内容。**不要指望靠搜索 API 拿到笔记数据**——调研 Agent 能搜到的是媒体、论坛、电商公开页；平台内的一手观察由用户以链接、文字或截图补充（这也是 PRD 里「用户补充证据」的原因）。

## 3. 视觉模型（截图 → 指标）

先用 `deepseek-flash` 保持单一密钥；攒 20 张真实后台截图后，与 qwen-vl-ocr（¥0.3 / 0.5 每百万 token）、qwen3-vl-flash（¥0.15 / 1.5）、GLM-4.6V-Flash（免费）做一次准确率对比再定型。识别结果一律经人工核对后入库。

## 4. 图片生成与编辑

### 4.1 MiniMax `image-01`（出厂默认，用于概念图）

| 项 | 结论 |
|----|------|
| 接口 | `POST https://api.minimax.cn/v1/image_generation`（官方文档域名已迁到 platform.minimax.cn；旧域名 api.minimaxi.com 是否可用未核实 → **域名做成配置项**） |
| 参数 | prompt ≤ 1500 字符；aspect_ratio 8 种（含 3:4、1:1）；n = 1~9；seed；prompt_optimizer；response_format = url / base64；`aigc_watermark`（默认 false） |
| 形态 | 同步返回；**URL 24 小时过期** → 生成后立即落盘；**RPM 仅 10** → 任务调度器要对它限速 |
| 价格 | ¥0.025 / 张 |
| 局限 | 图生图只有人像 `subject_reference`；**没有编辑、换背景、多参考图能力**，做不了商品一致性 |
| 商用 | 协议未禁止也未明示授权；要求发布时显著标识、不得去除其水印 |

### 4.2 需要「编辑 / 一致性」时的备选

| 模型 | 能力 | 价格 |
|------|------|------|
| 火山 Seedream 5.0 lite / pro | 多图参考（最多 10 张）、组图、bbox 定位编辑、图层拆分；`watermark` 默认 **true**（调用时要显式设置） | lite ¥0.22；pro ¥0.30~0.60 |
| 阿里 qwen-image-3.0 / pro | 生成编辑一体，1~3 张输入图，主体一致性，中文小字渲染强 | ¥0.18 / ¥0.25~0.5 |
| qwen-image-edit-plus | 纯编辑 | ¥0.2 |
| 百炼 `wanx-background-generation-v2` | **商品换背景**：输入透明底主体图，主体像素保持不变 | ¥0.08 |
| 智谱 glm-image / CogView-4 | 仅文生图，汉字渲染强 | ¥0.1 / ¥0.06 |

### 4.3 对产品设计的约束（来自小红书规则）

- 商品主图：1:1 或 3:4，3~9 张，单张 ≤ 3MB；**首图必须是商品正面实物图**，不得拼接、不得带水印或文字（品牌 logo 除外）。
- 小红书《虚假失真不实细则》（2025-12-25 生效）把「**使用 AI 技术批量更换场景、主体等模板化操作痕迹**」列为违规示例，处罚从限流到冻结店铺。
- 因此：「实拍图换背景」功能**只用于笔记配图与主图的非首图**，不提供批量模板化换景；优先走「抠图 + 背景生成」这条主体像素不变的路线；生成后在界面上并排显示原图供人工比对。
- 笔记图片：推荐 3:4（1080×1440），整篇比例统一，≤ 18 张；标题 ≤ 20 字；正文 ≤ 1000 字；话题上限有 5 与 10 两种说法（未核实 → 默认按 5 个生成）。

## 5. 3D 生成

> **2026-09-20 更新**：3D 的主路线改为「看图 → build123d 代码建模」（[ADR-0003](adr/0003-code-cad-build123d.md)），用的是 §1 的 DeepSeek，不需要本节的任何一家。本节的网格生成服务留给有机造型（人物、动物、雕塑），接入时间延后。
>
> build123d：Apache-2.0，0.12.0（2026 年），Python 3.10~3.14；依赖 cadquery-ocp（OpenCascade，LGPL——**独立进程调用，不在我们的进程里链接**）、numpy、scipy、scikit-learn、lib3mf；装好约 600 MB。

| | Tripo（VAST） | 腾讯混元生 3D | Meshy | Hi3D（原 Hitem3D） |
|---|---|---|---|---|
| 最新模型 | H3.1 | 专业版 3.0 / 3.1、极速版 | Meshy 7.1 | v1.5~v3.0，最高 2048³ |
| 打印相关 | 转换可出 **STL / 3MF**；参数有 **`flatten_bottom`（切平底）、`auto_size`、`generate_parts`**；文档无水密承诺 | 直接出 STL，无 3MF | 直接出 STL / 3MF；**可打印性检测（免费）、修复、多色 3MF（≤16 色）、自动拆件加连接件**；有 Bambu Studio / Orca 插件 | 出 STL / 3MF；拆件、多色 |
| 耗时 | 白模约 40 秒，带贴图约 120 秒 | 极速版约 90 秒 | 未公布 | 未核实 |
| 价格 | 1 积分 = $0.01；图生 3D 白模 20 / 带贴图 30；高精几何 +20；转换 5~10；失败不扣 | ¥0.12 / 积分；专业版 20（白模 15）、极速版 15；赠 100 积分 | 图生 3D 20 / 30 积分；**API 需订阅 Pro（¥145 / 月）**；资产只保留 3 天 | 每模型 $0.30~0.90（v3.0 从 $2.10 起） |
| 大陆可用性 | 有中文站 developers.tripo3d.com，支持公对公充值与开票（个人充值方式未核实） | 人民币结算；能力正迁往 TokenHub | 只支持信用卡等，对大陆个人不友好 | 未核实 |
| 商用 | **付费用户享有全部权利**；免费档产出的权利归 Tripo | 生成物权属条款未找到（未核实） | 付费私有可售；免费档 CC BY 4.0 | 付费私有商用 |

另外：火山方舟价格页同时列有 doubao-seed3d-2.0（¥2.40 / 次）、Hyper3D-Gen2（¥1.80）、Hitem3d-2.0（白模 ¥5.80）——对「想只注册一家」的用户是个选项（是否与方舟同一把密钥调用，接入时确认）。拓竹 MakerLab 没有公开 API。开源自部署（Hunyuan3D-2.1 需约 29GB 显存、TRELLIS.2 需 ≥ 24GB）不符合「零依赖安装包」的产品形态，不考虑。

**结论**

1. **默认 Tripo H3.1，只生成白模**，提交时带上 `flatten_bottom` 与 `auto_size`，完成后再请求 STL / 3MF 转换；所有结果立即下载落盘。
2. **备选腾讯混元**（人民币结算、直接出 STL）；接入前确认 TokenHub 迁移与生成物权属。
3. Meshy 的打印增强接口（可打印性检测 / 多色 / 拆件）很对口，但订阅制与支付方式对目标用户不友好 → 放到 V2 评估。
4. AI 生成的网格**常常不是流形**：应用内必须自带「检测 + 基础清理」，重度问题交给切片软件自带的修复（Bambu Studio / OrcaSlicer 导入时会提示修复）。

## 6. 3D 打印工具链

### 6.1 交接给切片软件

桌面应用直接用**本地文件路径**调起用户已安装的切片软件（按文件关联，或在设置里指定可执行文件路径）。不走 URL Scheme——调研确认 Bambu Studio 的 `bambustudio://` 带域名白名单、PrusaSlicer 限 printables.com，只有 `orcaslicer://open?file=` 对第三方开放，而且它们面向的都是网页直链场景。

### 6.2 切片估算（V1，可选集成）

检测到 OrcaSlicer / Bambu Studio 时，用其命令行切片并解析输出 3MF 里的 `Metadata/slice_info.config`（`prediction` = 秒，`weight` = 克）。没装切片软件则退回体积法估算。注意单次切片会产生数百 MB 临时文件，用完要清理；预设里的 `printer_model` 必须与 3MF 一致。

```
orca-slicer model.stl --load-settings "machine.json;process.json" --load-filaments filament.json \
  --arrange 1 --orient --slice 0 --export-3mf out.3mf
```

### 6.3 打印机直连（不在 MVP）

拓竹没有公开云 API；2025-01 授权固件之后，发起打印等控制类操作需要授权，只读的 MQTT 状态推送仍可用；开发者模式只能在 LAN 模式下开启且会断开拓竹云；官方中间件 Bambu Connect 通过 `bambu-connect://import-file?path=<本地绝对路径>` 接收**已切片**文件。结论：MVP 不直连打印机；V2 再评估「Bambu Connect 交接」与「只读状态同步」。

### 6.4 趋势数据源

| 平台 | 结论 |
|------|------|
| Thingiverse | 有官方 REST API（OAuth2，300 次 / 5 分钟），含 Popular / Newest / Featured |
| Etsy | Open API v3；禁止 screen-scraping |
| MakerWorld | **无公开 API，协议明文禁止抓取与通过 AI 服务自动获取内容** |
| Printables | 无官方 API |

→ 趋势雷达（V1）只接 Thingiverse 官方 API；MakerWorld / Printables 只支持用户贴链接或上传截图。

## 7. 销售渠道

### 7.1 能力矩阵（决定适配器的 capabilities）

| 渠道 | 个人能否开店 | 官方 API 对个人 / 个体户 | 发布内容 | 同步订单 | 回收数据 | 接入结论 |
|------|--------------|--------------------------|----------|----------|----------|----------|
| **小红书** | ✅ 个人店（身份证） | ❌ 「商家自研」明文要求店铺类型**非个人 / 个体工商户**；服务商需企业成立 ≥ 1 年 + 保证金 1~2 万 / 应用。且**没有笔记 API、没有经营数据 API** | 人工（发布包） | 后台导出文件 | 截图 / 导出文件 | **首发渠道，半自动** |
| 闲鱼 | ✅ | ❌ 开放平台只面向定向邀请的服务商 | 人工 | 人工 | 人工 | 测款与清库存，人工 |
| **微信小店** | ❌ 仅企业 / 个体工商户 | ✅ 后台「自研」直接拿 AppID / Secret；覆盖商品、订单、售后、物流、资金 | API | API | 部分 | **第一个全自动渠道**（前提：用户办个体工商户执照） |
| 抖店 / 淘宝 / 拼多多 / 快手 | ✅ 个人可开 | ❌ 自研通道要求软著与主体一致 / 仅天猫 / 仅大商家 | 人工 | 人工 | 人工 | 暂不投入 |
| Etsy / Shopify / TikTok Shop | Etsy 不接受中国大陆新店，且 2025-06 起要求 3D 打印商品基于卖家原创设计；Shopify Payments 不含大陆；TikTok Shop 需营业执照 | — | — | — | — | 暂缓 |

### 7.2 小红书个人店的经营规则（要做进产品的部分）

| 规则 | 内容 | 做进哪里 |
|------|------|----------|
| 保证金 | 默认 ¥500；累计结算 < 1 万元属试运营，可免缴先上架；上月销售 > 5 万元起另收浮动保证金 | 上架清单提示 |
| 技术服务费 | 「百万免佣」已于 2026-05-15 终止。模玩、玩具模型、家居饰品、艺术品周边、文创手作 **5%**；个性定制 / DIY **2%**（部分子类 5%） | 成本定价器的渠道费率预设 |
| 结算 | 确认收货后 4 天（店铺分 ≥ 4.7），否则 7 天 | 现金流提示 |
| **发货时效** | 现货 **48 小时内**发货；预售按「付款后 N 天」。个人店在模玩 / 玩具模型 / 艺术品周边类目的预售天数与店铺分挂钩：≥ 4.8 分 30 天，≥ 4.5 分 15 天，≥ 4.2 分 7 天，< 4.2 分不能预售；预售超期发货率 > 10% 冻结商品 | 产能测算 → 建议「现货 / 预售 N 天」；打印队列按发货截止排序并预警 |
| 笔记发布 | **没有官方发布 API**；分享 SDK 只能拉起 App 发布页并带入图片 / 视频，已不支持自动填充标题、文案、话题 | 发布包（电脑端官方网页 / 手机扫码），人工点发布 |
| 自动化红线 | 2026-03-10 官方公告：通过托管工具注册、发布、互动的账号封禁，媒体称已治理 120 万+ 账号；有用户用脚本操作网页版定时发布后被封的案例 | **不做任何自动发布、定时代发、自动互动**；不集成 xiaohongshu-mcp 一类的浏览器自动化工具 |
| 数据 | 千帆后台与创作者中心可手动导出（二手信息）；无个人可用的数据 API；聚光 Marketing API 只面向重点广告客户 | 截图识别 + 导出文件导入 |

### 7.3 闲鱼要点

2026-06-01 生效的《闲鱼社区经营性行为界定与管理规范》：媒体转述的认定指标包括在售 > 30 件且品类 < 3 类、同款售出 > 5 次、年交易 > 104 笔、年销售额 > 10 万元（各指标是「同时满足」还是「满足其一」各来源说法不一）；被认定为经营性卖家后须亮照并履行七天无理由退货。**重复售卖同款打印件很容易触线** → 平台对闲鱼渠道做计数提醒。普通卖家基础软件服务费 0.6%（单笔封顶 60 元）。

### 7.4 渠道策略（结论）

1. **小红书先做，走半自动**：发布包 + 人工发布 + 导出 / 截图回收数据。
2. **内置小红书规则**：发货时效、费率、字数与图片规格、极限词、IP 名词、AI 内容声明提醒。
3. **微信小店是第一个 API 渠道**（V1）：引导用户办个体工商户执照——同时解决「个人年交易额 10 万元免登记上限」的问题。
4. **闲鱼只做文案适配与阈值提醒**。
5. 小红书官方 API 只能以企业服务商身份获得，且个人店能否订购服务商应用未核实 → 列为远期，先向官方提工单确认。

来源：https://open.xiaohongshu.com/document/developer/file/103 · https://agora.xiaohongshu.com/doc/qa · https://school.xiaohongshu.com/rule/detail/27/119797 · /rule/detail/35/1045 · /rule/detail/35/1057 · /rule/detail/39/119740 · https://www.21jingji.com/article/20260310/herald/e3509f333c191c22cc1160c8ab49f7fd.html · https://www.v2ex.com/t/1223499 · https://open.goofish.com/doc/quick-start.html · https://www.100ec.cn/detail--6659868.html · https://e.qq.com/faq/wechat-store/store-setup/wechat-store/ · https://developers.weixin.qq.com/doc/store/API/basics/UnionID.html · https://help.etsy.com/hc/en-us/articles/115015710408

## 8. 选品数据源（小红书）

| 来源 | 个人卖家可用性 |
|------|----------------|
| 千帆（店铺后台） | 个人店可用，有热搜词、商机洞察、「商品机会」；**无开放 API** → 截图 / 导出后录入 |
| 聚光关键词规划、灵犀 | 需要企业专业号（企业 / 个体工商户认证，认证费 600 元 / 年）+ 广告账户；个人不可用 |
| 千瓜、新红、灰豚等第三方 | 千瓜 1599 元 / 月起，价格页无 API；对个人卖家过贵 |
| 自行爬取 | **不做**。用户协议禁止；已有民事判赔 490 万元与刑事判例；2026-03 起平台封禁 AI 全托管账号 |

## 9. 密钥配置预设（降低首次使用门槛）

BYOK 意味着用户要自己注册多家平台，这是外部用户最大的上手障碍。设置向导提供三种预设，每把密钥都带「测试连接」：

| 预设 | 组成 | 需要注册 |
|------|------|----------|
| **性价比组合**（默认） | DeepSeek（LLM + 视觉）· 智谱搜索 · MiniMax 出图 · Tripo 3D | 4 家 |
| **一家搞定** | 火山方舟：豆包模型（LLM + 视觉）· 豆包搜索 · Seedream · Seed3D / Hyper3D | 1 家（搜索是否同一把密钥待确认） |
| 自定义 | 任意 OpenAI 兼容 LLM + 各类适配器自选 | — |

未配置的能力按「能力降级」处理（见 [02-ux-flows.md](02-ux-flows.md) §4）。长期看，可以提供官方中转额度来彻底消除这个门槛，但那会让我们成为生成式 AI 服务提供者，合规成本见 [07-risks-compliance.md](07-risks-compliance.md)。

## 10. 接入前待核实清单

- [ ] Tripo：个人充值方式；API 按量付费用户是否等同「付费档」权利（建议邮件确认）。
- [ ] 腾讯混元生 3D：TokenHub 迁移后的接口与计费；生成物权属条款。
- [ ] 火山方舟：3D 模型与豆包搜索是否与方舟同一把密钥。
- [ ] MiniMax：旧域名 api.minimaxi.com 是否仍可用；是否写入隐式 AIGC 元数据。
- [ ] 小红书笔记话题数量上限（5 或 10）。
- [ ] 用 20 张真实后台截图评测视觉模型的识别准确率。

## 来源（节选）

- DeepSeek：https://api-docs.deepseek.com/zh-cn/quick_start/pricing · /updates · /guides/tool_calls · /guides/json_mode · /guides/vision · /quick_start/rate_limit
- 搜索：https://docs.bigmodel.cn/cn/guide/tools/web-search · https://www.volcengine.com/docs/87772/2272951 · https://help.aliyun.com/zh/model-studio/web-search · 小红书 robots.txt
- MiniMax：https://platform.minimax.cn/docs/api-reference/image-generation-t2i · /guides/rate-limits · /guides/pricing-paygo · /protocol/user-agreement
- 火山方舟：https://docs.volcengine.com/docs/ark/model-pricing · /image-generation-api
- 阿里百炼：https://help.aliyun.com/zh/model-studio/model-pricing · /image-background-generation
- Tripo：https://developers.tripo3d.com/zh/pricing · /zh/terms · https://developers.tripo3d.ai/en/docs/models-convert
- 腾讯混元 3D：https://cloud.tencent.com/document/product/1804/123461 · /123447
- Meshy：https://docs.meshy.ai/en/api/pricing · /analyze-printability · /multi-color-print
- Hi3D：https://docs.hi3d.ai/en/api/getting-started/pricing
- 切片与拓竹：https://www.orcaslicer.com/wiki/cli/cli_mode · https://printago.io/blog/3mf-file-format · https://wiki.bambulab.com/en/software/third-party-integration · https://productionshaped.com/notes/2026-05-14-bambu-studio-url-schemes-what-doesnt-work-and-why/
- 小红书规则：https://school.xiaohongshu.com/rule/detail/136/1324 · /rule/detail/106/10126 · 用户协议 https://agree.xiaohongshu.com/h5/terms/ZXXY20220331001/-1
- 标识办法：https://www.cac.gov.cn/2025-03/14/c_1743654684782215.htm
- 趋势数据：https://www.thingiverse.com/developers/getting-started · https://makerworld.com/en/user-agreement
