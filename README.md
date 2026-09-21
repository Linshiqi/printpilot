# PrintPilot · 3D 打印爆品工作台

给 3D 打印个人卖家的一条可度量的爆品流水线：**选品调研 → 预览图 → 3D 模型 → 打样定价 → 上架发布 → 运营 → 复盘**。AI 负责起草，人负责拍板。

- **形态**：Windows 桌面应用，单个 exe 安装包，零外部依赖，数据全在本机，用户自带接口密钥。
- **技术栈**：Rust + Tauri 2 + Leptos（CSR）+ 内嵌 SQLite，工程约定对齐 `E:\velo`。
- **当前状态**：W0 进行中——工程骨架、数据层、组件库、项目看板、几何内核、3D 视图桥、调研流水线、**看图 → build123d 代码建模**（出规格 → 生成 → 自动修复 → 参数面板 / 指令修补 → 看图复核）、**建模工作室**与**图片工作台**（两个一级入口，都是「和 AI 持续对话着改」；图片可一键送去建模）已落地；进度见 [W0 状态](docs/w0-status.md)，路线见 [路线图](docs/06-roadmap.md)。

## 文档导航

| 文档 | 回答什么问题 |
|------|--------------|
| [00 · 产品一页纸](docs/00-product-brief.md) | 这是什么、为谁、为什么、怎么衡量 |
| [01 · PRD](docs/01-prd.md) | 每个模块做什么、优先级、验收标准、待拍板的问题 |
| [02 · 用户旅程与信息架构](docs/02-ux-flows.md) | 主流程、页面结构、关键界面线框、交互规范 |
| [03 · 技术架构](docs/03-architecture.md) | 分层、接口、任务系统、Agent 编排、3D 管线、数据模型、打包发布 |
| [04 · 第三方集成调研与选型](docs/04-integrations.md) | DeepSeek / 搜索 / MiniMax / 3D 生成 / 切片 / 销售渠道的能力、价格、限制 |
| [05 · 增长与转化率方案](docs/05-growth-cro.md) | 北极星、漏斗、转化杠杆、测款、实验、复盘节奏 |
| [06 · 路线图与里程碑](docs/06-roadmap.md) | 分期、每周交付、出口标准、并行的业务准备 |
| [07 · 风险与合规](docs/07-risks-compliance.md) | 风险矩阵、上架合规清单、发行方合规、明确不做的事 |
| [08 · 市场与竞品调研](docs/08-market-research.md) | 市场数据、卖家痛点、竞品与缺口（带来源） |
| [ADR-0001 · 技术栈](docs/adr/0001-tech-stack.md) | 为什么是 Rust + Tauri + Leptos，从 velo 照搬什么 |
| [ADR-0002 · 渠道策略](docs/adr/0002-channel-strategy.md) | 为什么「半自动发布」是正式形态、红线是什么 |
| [ADR-0004 · 建模工作室](docs/adr/0004-modeling-studio.md) | 为什么建模是一级入口、「设计」是什么、对话式修改怎么工作（意图判断、上下文、贴图、分叉与撤销） |
| [ADR-0006 · 项目是主线](docs/adr/0006-project-spine.md) | 三个工作台怎么挂到项目上、阶段门清单为什么是「算出来的」而不是手工打勾的、什么时候要写「跳过原因」 |
| [ADR-0005 · 图片工作台](docs/adr/0005-image-studio.md) | 为什么出图也是一条对话、「画板」和「当前这张图」是什么、规划模型和图片模型怎么分工、不能改图的供应商怎么如实处理、接了哪两家 |
| [ADR-0003 · 代码式 CAD](docs/adr/0003-code-cad-build123d.md) | 为什么 3D 主路线是「看图 → build123d 脚本」、质量靠什么、怎么局部修改、怎么安全地执行模型写的代码、引擎怎么分发、实测数字 |

建议阅读顺序：00 → 01 → ADR-0001 / 0002 → 06，其余按需查。

## 开发

```
.\scripts\dev.ps1               # 开发：trunk serve(17431) + Tauri 窗口 + WebView 调试口(17439)；端口被占会自动顺延
cargo test --workspace          # 全部单测（前端的纯逻辑与 i18n 护栏也在宿主机上跑）
cargo check --target wasm32-unknown-unknown -p printpilot-ui   # 只检查前端
cargo tauri build --bundles nsis  # 打包 Windows 安装包（target/release/bundle/nsis）；发版走 CI，见下
```

代码建模需要本机有建模引擎（build123d + OpenCascade）。开发机上装一次即可，装在 `%LOCALAPPDATA%i.printpilot\cad-engineenv`，约 600 MB，需要本机有 Python 3.10 ~ 3.13：

```
.\scripts\setup-cad-engine.ps1            # 国内网络加 -Mirror；重装加 -Force
# 执行器与提示词速查表的 Python 单测(用引擎自己的解释器):
& "$env:LOCALAPPDATAi.printpilot\cad-engineenv\Scripts\python.exe" -X utf8 -m unittest discover -s crates\pp-cad\py
```

没装引擎不影响其它功能，需要引擎的 Rust 测试会自动跳过。**最终用户不需要 Python**：正式版的安装包里自带引擎（一个约 224 MB 的压缩文件，由 `scripts/build-engine-pack.py` 构建），应用第一次用到时自己解开，不联网（ADR-0003「引擎的分发」）。想不填密钥先看效果：「设置 → 演示模式」打开后，侧栏「建模」里新建一个设计、说一句话，就能用内置脚本在真引擎上把「出规格 → 生成 → 对话修改 → 改参数」完整走一遍。

环境要求：Rust stable、`wasm32-unknown-unknown` target、trunk 0.21、tauri-cli 2.x。**不需要 Node**——只有升级 three.js 时才在 `viewer3d/` 里跑一次 `pnpm install && pnpm build`，产物 `public/viewer3d/three-bundle.mjs` 是提交入库的。

排查前端问题（打包版尤其要用）：

```
.\scripts\dev.ps1               # 自动带上 WebView 调试口，并把实际用到的端口写进 target/dev-ports.json
py scripts/cdp.py console 10    # 回放控制台；还有 targets / eval / watch / click-btn / drag / type / shot
```

打包版同理：先设 `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=17439 --remote-allow-origins=*'` 再启动 `target\release\printpilot.exe`。

**端口**：本机同时有多个 Tauri 工程，所以这里刻意避开 Tauri 模板默认的 1420 一带——开发服务器用 **17431**（`Trunk.toml` 与 `tauri.conf.json` 的 `devUrl`，两者由单测保证一致），WebView 调试口用 **17439**。`dev.ps1` 启动前会探测端口，被占用就顺延到下一个空闲的，并用 `--config` 临时覆盖 `devUrl`，不需要改仓库里的文件；也可以显式指定：`.\scripts\dev.ps1 -Port 18000 -CdpPort 18100`。直接敲 `cargo tauri dev` 也能用，只是不带端口探测。

## 发版

推一个 `v*` 标签，GitHub Actions 会出三个安装包（Windows x64、macOS Apple 芯片、macOS Intel）并发布到 Release——三个平台全部成功才会从草稿转为正式发布：

```
# 1. 改三处版本号：Cargo.toml（根包）、src-tauri/Cargo.toml、src-tauri/tauri.conf.json（单测守着三者一致）
# 2. 写 docs/releases/vX.Y.Z.md（会成为 Release 说明）
git commit -am "发布 X.Y.Z" && git tag vX.Y.Z && git push origin main vX.Y.Z
```

流程细节、成本与还没做的事（代码签名、公证、自动更新）见 `CLAUDE.md`「发版」和 `.github/workflows/release.yml` 顶部的说明。

## 代码结构

```
src/            前端（Leptos，编译到 wasm）：app · state · controller · ui(组件库) · view · ipc · viewer3d
src-tauri/      后端壳：命令、自定义 pp-asset:// 协议、配置、打包设置
crates/
  pp-common/    前后端共享的 DTO、枚举、错误码
  pp-db/        SQLite：schema、迁移、仓储
  pp-geometry/  网格内核：读入、度量、切平底并封口、导出 STL / 3MF
  pp-providers/ 供应商适配器：LLM(DeepSeek / OpenAI 兼容)、联网搜索(智谱)、mock
  pp-agent/     有界流水线：市场调研(规划 → 检索 → 评分 → 审校)、看图建模(出规格 → 写代码 → 执行 → 修复 → 复核)，提示词在 prompts/
  pp-cad/       代码式 CAD：引擎定位、沙箱执行器(py/runner.py)、参数解析与改写、代码契约
public/viewer3d/  three.js 视图桥（bridge.mjs）与打包好的 three
viewer3d/       three.js 的打包入口（仅升级时用）
locales/        界面文案 zh / en
docs/           产品与技术设计
```
