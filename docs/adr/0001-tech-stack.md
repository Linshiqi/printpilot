# ADR-0001 · 技术栈：Rust + Tauri 2 + Leptos，内嵌 SQLite，单一 exe 安装包

- 状态：**已采纳**（2026-09-20）
- 取代：本 ADR 的第一版（Next.js + PostgreSQL + 独立 worker）。该方案要求用户机器上有 Docker / Postgres，违反下面的约束 1，已废弃。

## 背景与约束

负责人给出的两条硬约束：

1. **交付物是一个 Windows exe 安装包，装完即用**——用户机器上不需要再安装数据库、运行时、Docker、Python 或任何其他东西。
2. 优先评估 **Rust + Tauri + Leptos**，参考同栈的成熟工程 `E:\velo`（ztmail 0.9.x）；有更好的方案可以换。

产品的技术特征：大量表单 / 看板 / 仪表盘界面；分钟级的后台长任务（调用多家 LLM / 出图 / 3D 生成的 HTTP 接口）；本地保存大体积资产（图片、GLB / STL）；应用内 3D 预览与轻编辑。

## 备选方案

| | A. Tauri 2 + Rust + **Leptos** | B. Tauri 2 + Rust + React/TS | C. Electron + TS | D. 本地服务 + 浏览器 |
|---|---|---|---|---|
| 单 exe、零依赖 | ✅ | ✅ | ✅ | ✅ |
| 安装包体积 | ≈ 10~20 MB | ≈ 10~20 MB | ≈ 100 MB+ | 小 |
| 复用 velo 的成熟设计 | **几乎全部**（见下） | 仅后端与打包部分 | 无 | 无 |
| 前后端类型共享 | **同一语言、同一 DTO crate** | 需要类型生成 | 同语言（TS） | — |
| 界面组件生态 | 弱，需自建组件库 | 强（shadcn 等） | 强 | 强 |
| 3D 生态 | 通过 JS 桥使用 three.js | 原生 three.js / R3F | 同左 | 同左 |
| 维护者熟悉度 | **高**（velo 同栈） | 中 | 低 | 低 |

## 决定

采用 **方案 A：Rust + Tauri 2 + Leptos（CSR）+ 内嵌 SQLite（rusqlite）**，工程结构与约定对齐 velo。

**为什么不是 B**：单看界面层，React 生态确实更省事（组件库、R3F、拖拽库、图表库）。但 velo 已经把这条技术栈上最贵的坑全部踩完并沉淀成了代码与脚本——NSIS 安装包定制、代码签名流水线、自建更新服务器、CSP、WASM 构建参数、凭据存储、后台队列、LLM 流式输出、JS 库互操作。对这个团队来说，**复用 velo 省下的时间远多于 Leptos 组件生态欠的那部分**；而且全栈同一种语言、同一份 DTO，没有类型生成这一层。

**方案 A 的真实短板与对策**

| 短板 | 对策 |
|------|------|
| 3D 生态在 JS | 照搬 velo 嵌 PDF.js 的「JS 桥」模式嵌 three.js：JS 只负责渲染，接口约 10 个函数；**所有几何运算放在 Rust 后端**。备选是纯 Rust 的 `three-d`，仅在 JS 桥走不通时启用 |
| Leptos 组件生态弱；velo 的 `ui.rs` 很薄，按钮/对话框都是手写类串 | PrintPilot 是表单密集型应用，**第一周先建一套真正的组件库**（Button / Field / Select / Dialog / Tabs / Card / Badge / Table / Toast），这一点要和 velo 反着做 |
| AI 生成 Leptos 代码容易混用不同版本的 API | 固定与 velo 相同的主版本（Leptos 0.7、leptos_i18n 0.5、Tauri 2、Trunk 0.21、Tailwind v4）；CLAUDE.md 写明约定并指向 velo 的参考实现 |
| 看板拖拽、图表没有现成库 | 拖拽复用 velo 的 `pointer_drag.rs`（指针事件 + `data-drop-*`）；漏斗与趋势图用 Leptos 直接输出 SVG |

## 从 velo 照搬的清单

| 领域 | 照搬什么 | velo 出处 |
|------|----------|-----------|
| 前端状态 | `Copy` 的扁平 `AppState`（字段全是 `RwSignal`）+ `provide_context`；`Copy` 的 controller；view 只读信号、只调 controller | `src/state.rs`、`src/controller/` |
| IPC | `ipc::cmd` 命令名常量表 + `call / call_unit / fire`；后端 `#[tauri::command(rename_all = "snake_case")]` | `src/ipc.rs` |
| 事件 | kebab-case 事件名；`app.emit(name, json!({...}))`；并发任务用 `req_id` 区分 | `src/app/mod.rs`、`listeners.rs` |
| 错误 | 后端 `Result<T, String>`，错误串 `#错误码#细节`；前端按码本地化后弹 toast | `src/i18n_util.rs` |
| DTO | 前后端共用一个 crate，`backend` feature 控制后端专属部分 | `crates/velo-common` |
| 数据库 | rusqlite；`PRAGMA user_version` 线性迁移；WAL / NORMAL / busy_timeout；读写分连接；`Arc<dyn DatabaseBase>` 便于测试替换 | `crates/velo-common/src/db/` |
| 后台队列 | 单 worker，`tokio::select!`（mpsc 唤醒 vs. 睡到下一个到期时间）；重试次数、错误、到期时间全落库；计数变化才发事件 | `src-tauri/src/command/op_queue.rs` |
| 凭据 | `CredentialStore` trait + 系统凭据管理器实现 + 内存实现（测试用）；密钥永不出现在配置文件与前端 | `crates/velo-common/src/credential_store.rs` |
| 配置与数据目录 | 原子写、逐字段降级、读失败拒绝覆盖；数据目录可迁移；目录暂不可用时等待与兜底；给卸载器留指针文件 | `config_manager.rs`、`data_dir_wait.rs` |
| LLM 接入 | OpenAI 兼容 `/chat/completions` 流式；多 profile；强制 https；`ai-chunk` 事件；`AtomicBool` 取消；**SSE 按字节缓冲再切行**；提示词为 Rust 常量并用单测锁定拼装顺序 | `src-tauri/src/command/ai/` |
| 本地文件进 WebView | 自注册异步 URI scheme（带路径校验），不用 `asset://` | `src-tauri/src/lib.rs` |
| JS 互操作 | `#[wasm_bindgen(module = "/public/…/bridge.mjs")]`；JS 只渲染不做 UI；大库动态 `import()` 懒加载；资源用绝对路径；重型查看器放独立窗口（`?win=`） | `src/view/pdf_view.rs`、`public/pdfjs/bridge.mjs` |
| 界面基建 | Tailwind v4 经 Trunk 构建（不需要 Node）；`.dark` 选择器主题；`icon.rs` 枚举 → SVG path；toast；右键菜单；自绘标题栏 | `style/input.css`、`src/theme.rs`、`src/icon.rs` |
| i18n | leptos_i18n + `locales/*.json`；`t!` / `td_string!`；键对齐护栏单测 + `scripts/i18n.py` | `src/i18n_guard.rs` |
| 打包发布 | NSIS（currentUser）；`hooks.nsh`；`build_release.ps1` → 代码签名 → 单独生成更新签名 → 双站点发布；`+crt-static`（不依赖 VC++ 运行库） | `src-tauri/installer/`、`scripts/`、`.cargo/config.toml` |
| 调试与测试 | `scripts/cdp.py` 连打包版 WebView 取证；内联 `#[cfg(test)]`；联网测试 `#[ignore]`；配置一致性单测；前端错误上报（带配额） | `scripts/cdp.py`、`src/main.rs` |

**必须继承的构建教训**（velo 注释里标了 ⚠️ 的）：WASM 影子栈调到 8 MB；`index.html` 不写内联 `<style>` 块；dev profile 不给 WASM 开 `opt-level`；发布用 `trunk build --cargo-profile release`；更新签名必须对「代码签名之后」的文件计算；先传安装包后传 `latest.json`；`hooks.nsh` 存为 UTF-8 with BOM。

**复用方式**：第一天直接从 velo 复制需要的模块（copy-and-own，注明来源）；PrintPilot 的 MVP 稳定后，再把两边都在用的部分抽成共享 crate。现在就抽公共库会拖慢两个项目。

## 关于「零依赖」的三个细节

1. **SQLite 静态编译进 exe**（rusqlite `bundled`），没有外部数据库。
2. **MSVC 运行库静态链接**（`+crt-static`），不依赖 VC++ 可再发行组件。
3. **WebView2**：Windows 11 自带，Windows 10 绝大多数已由系统更新装好。安装包用 `embedBootstrapper`（内嵌约 2 MB 的引导程序，缺失时自动安装）；如需面向完全离线的环境，另出一个内嵌完整运行时的「离线版」安装包。

**可选集成不算依赖**：检测到用户已安装 Bambu Studio / OrcaSlicer 时提供「一键打开」与「精确切片估算」，没装则这两项降级，其余功能不受影响。

## 影响

- 正面：安装包小、启动快、无运行时依赖；全栈一种语言；发布基础设施当天可用。
- 负面：界面开发速度慢于 React 方案，需要前置投入组件库；WASM 调试体验较差（靠 velo 的 CDP 脚本与错误上报弥补）。
- 放弃的东西：原方案里的 Postgres / pg-boss / Next.js / Python sidecar / Blender 依赖全部移除；网格修复改为应用内的原生实现（见 [03-architecture.md](../03-architecture.md) §8）。
- 未来云端化：领域逻辑全部在与界面无关的 crate 里，将来可以外包一层 axum 变成服务端；现在不为它付任何成本。
