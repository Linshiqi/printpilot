# CLAUDE.md

PrintPilot：给 3D 打印个人卖家的桌面应用，把「选品调研 → 预览图 → 3D 模型 → 打样定价 → 上架发布 → 运营 → 复盘」做成一条流水线。产品设计在 `docs/`，动手前先读 `docs/00-product-brief.md` 和与任务相关的那一篇。

## 硬约束（不可违反）

1. **交付物是单个 Windows exe 安装包，零外部依赖。** 不引入需要用户另行安装的任何东西：数据库服务、Docker、Node / Python 运行时、VC++ 运行库、Blender。SQLite 静态编译；`+crt-static`。用户已安装的切片软件只能作为「可选集成」，缺失时功能降级而不是报错。
   代码建模的引擎（build123d + OpenCascade，解开约 740 MB）**不是例外**：它压成一个文件（约 224 MB）**打在安装包里**，应用第一次用到时自己校验、解到应用数据目录，不联网（ADR-0003「引擎的分发」）；引擎出问题时只有「代码建模」不可用，其余功能照常。开发机上用 `scripts\setup-cad-engine.ps1` 装一个等价的虚拟环境，或者用 `scripts/build-engine-pack.py` 造一个真的引擎包。
2. **渠道红线（ADR-0002）。** 不实现、不集成任何非官方的平台自动化：模拟登录、逆向接口、浏览器自动化发帖、定时代发、自动互动、站内数据抓取。「AI 起草打包 + 人工点发布」是正式形态。
3. **AI 起草，人拍板。** AI 产出都是候选，经用户「采用」才进入下一阶段；任何对外内容不自动发出。
4. **密钥只进系统凭据管理器**，不写配置文件、不进日志、不下发到前端（前端只知道 `has_key`）。
5. **买家个人信息不进入任何模型的上下文。**
   发给视觉模型的参考图，入库时一律重新编码（顺带去掉 EXIF 里的位置与机型）。
6. 依赖许可证只用 MIT / Apache / BSD / MPL；字体只用 OFL。不在进程内链接 GPL 库。
7. **模型写的代码只在沙箱里跑**（ADR-0003）：AST 白名单（只许 `build123d` / `math`，禁危险内建、双下划线、任何文件读写）+ 受限命名空间 + 独立子进程（环境变量清空——**里面没有任何密钥**、临时工作目录、超时即杀）。导出由执行器统一做。放宽白名单之前先想清楚它挡的是什么。

## 技术栈与 velo 对齐

Rust + Tauri 2 + Leptos 0.7（CSR）+ Trunk 0.21 + Tailwind v4 + leptos_i18n 0.5 + rusqlite。**版本与 `E:\velo` 保持一致**，那是同栈的已发布工程，也是本项目的参考实现（只读，不要改它）。

拿不准某件事怎么做时，先看 velo 怎么做的（对照表见 `docs/adr/0001-tech-stack.md`）：

- 前端：`AppState` 是 `Copy` 的扁平结构体（字段全是 `RwSignal`）；controller 也是 `Copy`；view 只读信号、只调 controller。
- IPC：命令名集中在 `src/ipc.rs` 的 `cmd` 常量表；后端 `#[tauri::command(rename_all = "snake_case")]`；返回 `Result<T, String>`，错误串 `#错误码#细节`，前端按码本地化。
- 事件：kebab-case；并发任务用载荷里的 `job_id` / `req_id` 区分。
- 数据库：`PRAGMA user_version` 线性迁移；WAL；读写分连接；仓储走 trait 便于测试替换。
- 本地文件进 WebView：自注册 URI scheme `pp-asset://<asset_id>`，后端查库得路径。
- JS 互操作：`#[wasm_bindgen(module = "/public/viewer3d/bridge.mjs")]`；JS 只渲染不做 UI；three.js 动态 `import()` 懒加载；桥内资源写绝对路径 `/public/...`。
- 与 velo **反着做**的一点：velo 的 `ui.rs` 很薄。本项目表单密集，页面只许用 `src/ui/` 的组件，不许手写重复的 Tailwind 类串。

### 必须记住的构建教训（来自 velo 的 ⚠️ 注释）

- WASM 影子栈 8 MB（`.cargo/config.toml`）；栈溢出是 trap 不是 panic，不进日志。
- `index.html` 不写内联 `<style>` 块（打包版 CSP 会因此让所有 `style=` 属性失效）。
- dev profile 不给 WASM 开 `opt-level`；发布用 `trunk build --cargo-profile release`，不要 `--release`。
- Tailwind 对不认识的类静默跳过，不报错。
- SSE 流按字节缓冲再切行，否则多字节汉字会被切坏。
- **dev 正常 ≠ 打包版正常。** 每个里程碑都要在打包版上走一遍黄金路径（`scripts/cdp.py` 取证）。
- 更新签名对「代码签名之后」的文件计算；先传安装包后传 `latest.json`；`hooks.nsh` 存为 UTF-8 with BOM。

### 本项目自己踩到的坑(持续追加)

- **`view!` 的属性里不能直接写带泛型尖括号的表达式**(如 `.collect::<Vec<_>>()`):会被当成标签解析,报一串「wrong close tag」。先在宏外面 `let` 好再传进去。
- 要在 `<Show>` / `<Dialog>` 里用的非 `Copy` 值(`TextProp`、`ChildrenFn`)先放进 `StoredValue`——`Show` 的子视图是 `Fn`,不能把值 move 出去。
- 文案 prop 用 `leptos::text_prop::TextProp`(不在 prelude 里);这个版本的 `Signal<T>` **没有** `From<闭包>`,要用 `Signal::derive(..)`。
- 回调 prop:`Callback<()>` 接 `move || …`,单参数是 `Callback<(T,)>`、调用写 `cb.run((x,))`。
- IPC 参数走 `Serializer::json_compatible()`(见 `src/ipc.rs`),不要依赖 Tauri 把 ES `Map` 转回对象的隐式行为。
- 切口封盖用约束 Delaunay(`spade`)而不是 earcut:earcut 会丢共线点,封盖与侧面的边就对不上,网格不再水密。
- 终端里有 `NO_COLOR=1` 时 trunk 会报 `invalid value '1' for '--no-color'`;用 `scripts/dev.ps1` 启动(它会处理)。
- **`.ps1` 脚本只写 ASCII**。Windows PowerShell 5.1 把没有 BOM 的 `.ps1` 当 ANSI(GBK)读,UTF-8 的中文注释会吞掉后面的换行,下一条语句被悄悄注释掉——`dev.ps1` 里修 NO_COLOR 的那一行就这样丢过一次。
- **异步回调 / 事件处理里取文案用 `td_string!(current_locale(), …)`,不要用 `t_string!(i18n, …)`**:后者在非响应式上下文里会触发「outside a reactive tracking context」警告。组件的 children 也是稍后才执行的——要跟随语言变化的文案,在外层的 `move ||` 闭包里先取好再传进去。
- 改前端文件时 `cargo tauri dev` 的 trunk 会立刻重编;如果同时在跑别的 cargo 命令或文件处于半改状态,trunk 编译失败会连带把 dev 会话结束掉,重启即可。
- 多参数回调 prop:`Callback<(A, B)>` 接的是**两个参数**的闭包 `move |a, b| …`,不是接一个元组的闭包。
- **PowerShell 5.1 里别给 `dev.ps1` 这类脚本加 `*>` / `2>&1` 重定向**:脚本里 `$ErrorActionPreference = 'Stop'`,原生命令写到 stderr 的每一行都会被包成错误记录,第一行进度输出就把脚本终止了。
- 清空环境变量起 Python 子进程时,`USERPROFILE / HOME / APPDATA / LOCALAPPDATA / TEMP / TMP` 要指到临时工作目录:build123d 的依赖链在 import 时就要「用户主目录」,没有会直接抛 `Could not determine home directory`。
- 子进程的 stderr 写文件,不要用管道:我们靠轮询等它退出、期间不读管道,管道写满它就一直卡到超时。
- `std::fs::copy` 在 Windows 上会保留源文件的修改时间——导出的文件「修改时间比现在早」是正常的。
- **应用退出时析构函数不会跑**（托管状态不 Drop）：靠 Drop 清理的临时目录要在下次启动时兜底清一遍（`command::cad::clean_scratch`）；靠 Drop 杀的子进程要有别的退出途径（常驻引擎读到 stdin EOF 自己退出）。
- Windows 上子进程刚被杀的那一瞬间，它的工作目录可能还删不掉——`remove_dir_all` 要带几次短重试。
- **别在 bash heredoc 里写带反斜杠转义的 Python 字符串**（`
`、`\d`）：到 Python 手里已经被吃掉一层，写出去的文件里变成真换行。改这类内容用编辑工具，或者用 `chr(10)` / `chr(92)` 拼。

## 常用命令

```
.\scripts\dev.ps1                                 # 开发:cargo tauri dev + 端口探测 + WebView 调试口(端口见下)
cargo test --workspace                            # 全部单测(前端纯逻辑与 i18n 护栏在宿主机上跑)
cargo check --target wasm32-unknown-unknown -p printpilot-ui   # 只检查前端
py scripts/cdp.py console 10                      # 连 WebView 抓控制台(调试口从 target/dev-ports.json 读)
py scripts/cdp.py shot out.png                    # 截图;另有 eval / eval-to / click / click-btn / drag / type / watch
.\scripts\setup-cad-engine.ps1                    # 装代码建模的引擎(build123d 虚拟环境,约 600 MB;国内网络加 -Mirror)
python scripts/build-engine-pack.py               # 造随安装包走的引擎包(要先 pip install zstandard;产物在 src-tauri/resources/cad-engine/,不入库)
cargo tauri build --bundles nsis                  # 本机出 Windows 安装包(目录里有引擎包就会带上)
# 执行器与提示词速查表的 Python 单测——用引擎自己的解释器跑:
& "$env:LOCALAPPDATAi.printpilot\cad-engineenv\Scripts\python.exe" -X utf8 -m unittest discover -s crates\pp-cad\py
```

**端口约定**:本机有多个 Tauri 工程,**不要用 Tauri 模板默认的 1420,也不要用它附近的端口**(单测会卡 1400~1500)。
开发服务器 17431(`Trunk.toml` 与 `tauri.conf.json` 的 `devUrl` 必须一致),WebView 调试口 17439。
`dev.ps1` 会探测端口,被占就顺延并用 `--config` 临时覆盖,不改仓库文件。以后新增任何本地监听端口(比如发布包的局域网页面)
一律「随机端口或先探测」,不要写死常见端口。

## 发版（CI/CD）

- `.github/workflows/ci.yml`：每次推送跑全部测试（Windows）。CI 上会装真引擎并用 `PRINTPILOT_CAD_PYTHON` 指给测试，所以「需要真引擎」的测试在 CI 上是真的跑了。
- `.github/workflows/release.yml`：推 `v*` 标签 → Windows x64、macOS Apple 芯片、macOS Intel 三个安装包 → GitHub Release。先建草稿，**三个平台全部成功**才转正式发布。每个平台现场构建引擎包（有缓存：只有打包脚本、版本锁、执行器变了才重建）。手动触发只构建不发布，用来验证 macOS 能不能编过。
- 发一个新版本：改 `Cargo.toml`（根包）、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处版本号（单测守着三者一致）→ 写 `docs/releases/vX.Y.Z.md`（会成为 Release 说明）→ 提交 → `git tag vX.Y.Z && git push origin main vX.Y.Z`。标签和版本号对不上，流程第一步就会失败。
- 仓库是**私有**的：Actions 分钟数 Windows 按 2 倍、macOS 按 10 倍计；Release 附件外部下载不了（要对外分发得另找地方放，或把仓库 / Release 公开）。
- 还没做：Windows 代码签名、macOS Developer ID 签名与公证、自动更新——都需要负责人提供证书 / 密钥。

## 代码结构

见 `docs/03-architecture.md` §4。依赖方向：`src-tauri → pp-core → (pp-db, pp-providers, pp-agent, pp-channels, pp-geometry, pp-cad)`；所有 crate 可依赖 `pp-common`；前端只依赖 `pp-common`（不开 `backend` feature）。

## 代码建模（build123d）

设计与实测见 `docs/adr/0003-code-cad-build123d.md`，流水线图见 `docs/03-architecture.md` §8.0。动这块代码之前要知道的：

- **分层**：`pp-cad` 管「执行一段代码」（引擎定位、沙箱、常驻进程 `Worker`、参数解析与改写、代码契约）；`pp-agent::cad` 管「和模型来回」（出规格、生成、修复循环、指令修补、复核），通过 `CadExecutor` trait 拿执行能力，所以流水线测试不需要引擎；`src-tauri/src/command/cad.rs` 提供真执行器、入库、演示脚本。
- **代码契约**：`# ---- PARAMS ----`（每行 `名字 = 数值  # 单位 | 说明 | [最小, 最大]`）、每个特征一段 `# ---- FEATURE: name ----`、最终形体赋给 `result`。参数面板、改动段比对、局部修改都建立在它上面。
- **提示词里的 build123d 速查表和完整示例是被测试锁住的**（`crates/pp-cad/py/test_cheatsheet.py` 逐条在真引擎上跑）。往 `prompts/cad_code.md` 里加 API 写法，就要在那个测试里加一条；升级 build123d 之后先跑它。
- **检查结果是带类型的 `CadProblem`**：给模型的话写在 `for_model()`（英文），给用户的话在 `locales/*.json` 的 `cad.p_*`，前端 `side.rs::problem_text` 按 `kind` 分派。新增一种问题要三处一起加。
- **演示模式的脚本要过真流水线**（后端有测试守着）：演示的第一版故意带一个真实的 OpenCascade 错误，用来让「报错 → 修复」这条路在界面上看得见。
- **引擎进程是常驻的**（`AppCtx.cad_worker`，`runner.py --serve`）：冷启动约 4 秒、热的约 0.1 秒，参数面板靠它才做得到即改即见。同一个进程会跑很多段代码，所以执行器里那三道「不串味」的保险（每任务全新命名空间、AST 禁止给属性赋值、`math` 替身）不能拆；给白名单放行任何「能改到共享对象」的写法之前先想清楚。常驻进程起不来会自动退回单次执行。
- 需要真引擎的测试在没装引擎时自动跳过并打印一行说明——**CI 上看到它们「通过」不等于跑过**。

## 供应商适配器

- 模型名、域名、价格都是配置，不是常量（DeepSeek 已换过模型命名，MiniMax 已换过域名）。
- 每个 HTTP 适配器都要：走统一代理设置、超时与有限重试、按供应商限速（MiniMax 出图 RPM 10）、**拿到结果立刻下载落盘**（文件 URL 普遍 24 小时过期）。
- DeepSeek 只有 `json_object` 模式：输出用 serde 反序列化进强类型并做业务校验，失败带错误信息重试一次。
- DeepSeek 只有 `deepseek-flash` 能看图（OpenAI 风格的 `image_url` 内容块，data URL）；`deepseek-v4-pro` 不能。所以「看图」与「写代码」是两个模型。思考模式下思维链计入 `max_tokens`，要给足。
- HTTP 总超时 600 s + 读超时 120 s：非流式的长回答靠服务端的空行保活撑着，读超时才是「连接坏了」的判据。
- 每个适配器都要有 mock 实现（演示模式用）和录制回放的契约测试。

## 工作约定

- 文档、注释、提交说明、界面文案用中文；标识符用英文。提交说明写「用户能感知到什么变化」，一句话，参考 velo 的提交历史。
- 范围或技术方向变了：先改 `docs/`，重大决定新增一条 ADR，再写代码。
- 测试：`#[cfg(test)] mod tests` 内联；联网测试打 `#[ignore]`；几何与提示词有固定样例做回归。
- 新增界面文案必须同时进 `locales/zh.json` 与 `en.json`（护栏单测会卡）。
- `docs/04-integrations.md` 里的价格与接口信息有时效，接入某家之前先核对官方文档，并把变化回写文档。
