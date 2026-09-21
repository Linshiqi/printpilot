# PrintPilot — 3D 打印项目管理平台

[English](README.md) | **简体中文**

PrintPilot 是一个桌面平台：把一件 3D 打印产品从一个想法带到上架，并且让整个过程可以度量——

**市场调研 → 产品预览图 → 参数化 3D 模型 → 打样与定价 → 上架发布 → 运营 → 复盘**

AI 负责起草，人负责拍板。每个阶段都有明确的清单，清单是从实际产出里算出来的：证据够了，产品才往下走。

## 主要功能

- **项目看板**：一张卡片 = 一个单品，一列 = 一个阶段。阶段门的清单从项目名下的产出里**算出来**（调研报告、采用的预览图、模型、打样记录、定下的售价），不是手工打勾；要跳过清单得写一句原因，留给复盘。
- **市场调研 Agent**：有界的流水线（规划 → 检索 → 评分 → 审校），接大语言模型和联网搜索。每个机会都带着证据和价格带。
- **图片工作台**：和 AI 一句一句地出图、按指令改图——建模参考图、场景图、封面。任何一张图都可以一键送去建模。
- **建模工作室**：参考图或一句话 → 给人审的设计规格 → 参数化的 [build123d](https://github.com/gumyr/build123d) 脚本 → 在沙箱里用真正的 CAD 内核（OpenCascade）执行 → 自动检查与有界的修复循环。之后继续对话着改：改参数在本机重建，约 0.1~0.3 秒、不花钱；对话修改只动相关的特征段；每次改动都是一个可以回去的版本。导出 STEP、STL、3MF，并给出可打印性提示（贴床面积、悬空平面）。
- **成本与定价**：逐行对得上账的成本拆解（材料、机时、人工、包装、渠道费；失败的打印同时摊进材料**和**机时）、三档建议价、每机时毛利、产能、打样记录。
- **像原生应用的界面**：每种对象都有自己的右键菜单，AI 的回复是流式的，任何一轮都可以中途停止。

接下来：发布包（半自动上架）、内容日历、订单与打印、数据。见[路线图](docs/06-roadmap.md)。

## 设计原则

- **一个安装包，零外部依赖。** 单个安装程序，不需要另装任何东西。建模引擎（Python + build123d + OpenCascade，压缩后约 224 MB）就在安装包里，首次使用时自动解开，不联网。数据库是内嵌的 SQLite。
- **数据在本机。** 所有数据都留在用户自己的机器上；资料库可以放在任意一块盘上。
- **用户自带接口密钥。** 密钥在「设置」里填写，只存进操作系统的凭据管理器——不进配置文件、日志和数据库。
- **不做非官方的平台自动化。** 发布刻意做成半自动：应用把一切准备好，最后那一下由人在销售平台上点（[ADR-0002](docs/adr/0002-channel-strategy.md)）。
- **不可信的代码按不可信来对待。** 模型写的建模脚本要过 AST 白名单，在独立进程里执行（清空环境变量、超时即杀），自己碰不到文件系统（[ADR-0003](docs/adr/0003-code-cad-build123d.md)）。

## 当前状态

当前版本 **0.9.5**（Windows x64、macOS Apple 芯片、macOS Intel）。调研、图片、建模、定价和项目看板已经实现，并在演示模式下端到端验证过（脚本化的假模型 + 真实的建模引擎）。真实的供应商接口（DeepSeek、MiniMax、通义千问）还没有用密钥实测过；安装包还没有代码签名，也没有自动更新。详见[实现状态](docs/w0-status.md)和[各版本说明](docs/releases/)。

## 技术栈

Rust · [Tauri 2](https://tauri.app) · [Leptos](https://leptos.dev) 0.7（CSR，编译到 WebAssembly）· Tailwind CSS v4 · 内嵌 SQLite（rusqlite）· three.js 视图 · build123d / OpenCascade。

已接入的供应商：DeepSeek 或任何 OpenAI 兼容接口（文本与视觉）、智谱联网搜索、MiniMax 与通义千问（出图与改图）。内置的**演示模式**不填任何密钥就能把每条流程走一遍。

## 文档导航

| 文档 | 回答什么问题 |
|------|--------------|
| [00 · 产品一页纸](docs/00-product-brief.md) | 这是什么、为谁、为什么、怎么衡量 |
| [01 · PRD](docs/01-prd.md) | 每个模块做什么、优先级、验收标准、待拍板的问题 |
| [02 · 用户旅程与信息架构](docs/02-ux-flows.md) | 主流程、页面结构、关键界面线框、交互规范 |
| [03 · 技术架构](docs/03-architecture.md) | 分层、接口、任务系统、Agent 编排、3D 管线、数据模型、打包发布 |
| [04 · 第三方集成调研与选型](docs/04-integrations.md) | 大模型 / 搜索 / 出图 / 3D 生成 / 切片 / 销售渠道的能力、价格、限制 |
| [05 · 增长与转化率方案](docs/05-growth-cro.md) | 北极星、漏斗、转化杠杆、测款、实验、复盘节奏 |
| [06 · 路线图与里程碑](docs/06-roadmap.md) | 分期、每周交付、出口标准、并行的业务准备 |
| [07 · 风险与合规](docs/07-risks-compliance.md) | 风险矩阵、上架合规清单、明确不做的事 |
| [08 · 市场与竞品调研](docs/08-market-research.md) | 市场数据、卖家痛点、竞品与缺口（带来源） |
| [ADR-0001 · 技术栈](docs/adr/0001-tech-stack.md) | 为什么是 Rust + Tauri + Leptos |
| [ADR-0002 · 渠道策略](docs/adr/0002-channel-strategy.md) | 为什么「半自动发布」是正式形态、红线是什么 |
| [ADR-0003 · 代码式 CAD](docs/adr/0003-code-cad-build123d.md) | 为什么 3D 主路线是「看图 → build123d 脚本」、质量和安全靠什么、引擎怎么分发、实测数字 |
| [ADR-0004 · 建模工作室](docs/adr/0004-modeling-studio.md) | 「设计」是什么、对话式修改、分叉与撤销、流式与取消 |
| [ADR-0005 · 图片工作台](docs/adr/0005-image-studio.md) | 「画板」和「当前这张图」、规划模型和图片模型怎么分工、各家供应商能做什么 |
| [ADR-0006 · 项目是主线](docs/adr/0006-project-spine.md) | 工作台怎么挂到项目上、阶段门清单为什么是「算出来的」 |
| [ADR-0007 · 成本定价器](docs/adr/0007-cost-pricing.md) | 成本怎么算、三档建议价、每机时毛利、档案为什么是「带入」而不是引用 |
| [ADR-0008 · 右键菜单](docs/adr/0008-context-menus.md) | 拦掉浏览器自带的菜单，换成按位置和上下文给出的应用内菜单 |

建议阅读顺序：00 → 01 → ADR-0001 / 0002 → 06，其余按需查。

## 开发

环境要求：Rust stable、`wasm32-unknown-unknown` target、`trunk` 0.21、`tauri-cli` 2.x。**不需要 Node**——只有升级 three.js 时才在 `viewer3d/` 里重新打包一次，产物是提交入库的。

```
.\scripts\dev.ps1                 # 开发：trunk serve(17431) + Tauri 窗口 + WebView 调试口(17439)
cargo test --workspace            # 全部单测（前端的纯逻辑与 i18n 护栏也在宿主机上跑）
cargo check --target wasm32-unknown-unknown -p printpilot-ui   # 只检查前端
cargo tauri build --bundles nsis  # 本机出 Windows 安装包（target/release/bundle/nsis）
```

代码建模需要开发机上有建模引擎（装一次即可，约 600 MB，需要 Python 3.10 ~ 3.13），装在 `%LOCALAPPDATA%\ai.printpilot\cad-engine\venv`：

```
.\scripts\setup-cad-engine.ps1            # 国内网络加 -Mirror；重装加 -Force
# 执行器与提示词速查表的 Python 单测——用引擎自己的解释器跑：
& "$env:LOCALAPPDATA\ai.printpilot\cad-engine\venv\Scripts\python.exe" -X utf8 -m unittest discover -s crates\pp-cad\py
```

没装引擎不影响其它功能，需要引擎的 Rust 测试会自动跳过。**最终用户不需要 Python**：正式版的安装包里自带引擎包（由 `scripts/build-engine-pack.py` 构建）。

想不填密钥先看效果：「设置 → 演示模式」打开后，在「建模」里新建一个设计、说一句话，内置脚本会在真引擎上把「出规格 → 生成 → 对话修改 → 改参数」完整走一遍。

**端口。** 刻意避开 Tauri 模板默认的 1420 一带，好让几个 Tauri 工程同时开着：开发服务器用 **17431**，WebView 调试口用 **17439**。`dev.ps1` 启动前会探测，被占用就顺延到下一个空闲端口，不改仓库里的文件。

**排查前端问题**（对打包版同样有效）：

```
py scripts/cdp.py console 10      # 回放 WebView 控制台；另有 targets / eval / watch / click / rclick / drag / type / shot
```

## 发版

推一个 `v*` 标签，GitHub Actions 会出三个安装包（Windows x64、macOS Apple 芯片、macOS Intel）；**三个平台全部成功**才会转为正式发布：

```
# 1. 改三处版本号：Cargo.toml、src-tauri/Cargo.toml、src-tauri/tauri.conf.json（单测守着三者一致）
# 2. 写 docs/releases/vX.Y.Z.md（会成为 Release 说明）
git commit -am "X.Y.Z" && git tag vX.Y.Z && git push origin main vX.Y.Z
```

流程细节、成本与还没做的事（代码签名、公证、自动更新）见 `CLAUDE.md` 和 `.github/workflows/release.yml` 顶部的说明。

## 代码结构

```
src/            前端（Leptos，编译到 wasm）：app · state · controller · ui(组件库) · view · ipc · viewer3d
src-tauri/      后端壳：命令、自定义 pp-asset:// 协议、配置、打包设置
crates/
  pp-common/    前后端共享的 DTO、枚举、错误码，以及纯函数的业务规则（阶段门、定价引擎）
  pp-db/        SQLite：schema、迁移、仓储
  pp-geometry/  网格内核：读入、度量、切平底并封口、导出 STL / 3MF
  pp-providers/ 供应商适配器：大模型（DeepSeek / OpenAI 兼容，流式）、联网搜索、出图、mock
  pp-agent/     有界流水线：市场调研、出图规划、看图建模；提示词在 prompts/
  pp-cad/       代码式 CAD：引擎定位、沙箱执行器(py/runner.py)、常驻进程槽位、参数解析与改写、代码契约
public/viewer3d/  three.js 视图桥与打包好的 three
locales/        界面文案 zh / en
docs/           产品与技术设计、ADR、各版本说明
```
