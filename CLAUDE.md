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
- **别在 bash heredoc 里写带反斜杠的内容**（Python 里的 `\\`、`\d`，JS 里的 Windows 路径 `'E:\\dir\\'`）：到脚本手里反斜杠已经被吃掉一层——`'\\'` 变成 `'\'`（Rust 直接编不过），`E:\\target` 变成带制表符的 `E:<TAB>arget`。带反斜杠的补丁脚本、测试用的 JS 一律用编辑工具写成文件再跑（这条注记自己就被这样弄坏过一次）。
- **用 Python 脚本打补丁时,读写都要带 `newline=""`**:Windows 上文本模式默认把 `\n` 写成 `\r\n`,一个只改三行的补丁会让 `git diff` 变成整个文件(仓库里的文件是 LF)。补丁脚本用编辑工具写到 `target/` 下、跑完删掉。
- **build123d 0.12 的几个性能坑(实测,见 ADR-0003「性能与质量复查」)**:默认的精确包围盒在环面 / 样条面上每个面 20 多毫秒(带圆角的零件一次 150~190 ms);`export_stl` 用的是**相对**偏差(小孔过度细分、大曲面没有误差上限);`x.is_valid` 是属性,`callable(getattr(x, "is_valid"))` 这种兼容写法会把整遍检查跑两次;循环里做布尔比一次减一批慢 25 倍。执行器里量东西 / 导出之前先想想这几条。
- **事件回调里不能 `expect_context` / `use_context`**：回调执行时没有响应式上下文，会直接 panic（`expected context of type … to be present`），整个界面跟着死。要用的东西（`AppState`、i18n）在**组件体里**取好，`Copy` 进闭包。右键菜单的回调踩过。
- **两个互相镜像的 `Effect` 必须「值不一样才写」**：Leptos 的 `set` 不管值变没变都会通知订阅者。`A 变了 → 写 B`、`B 变了 → 写 A` 这一对如果无条件地写，就是一个死循环——界面线程被占满，窗口卡死，连 DevTools（`cdp.py`）都连不上（表现为 `Runtime.enable` 超时）。项目抽屉的开关同步（`project_drawer.rs`）踩过。
- **web-sys 里的 `ClipboardEvent` 属于「不稳定接口」**（要加 `--cfg=web_sys_unstable_apis` 才有），所以 Leptos 的 `ev::paste` 给的是普通的 `web_sys::Event`。只是想看一眼剪贴板里有没有图，用 `js_sys::Reflect` 取 `clipboardData.types` 就够了（`src/view/imagery/mod.rs`）；图本身让后端用 `arboard` 去读。
- **把本地文件拖进窗口，WebView 里的 HTML5 拖放收不到路径**：Tauri 默认接管了文件拖放，要听的是 `tauri://drag-enter / over / drop / leave`（`app::listen_file_drops`）。事件里的坐标是**物理像素**，和元素位置比之前要除以 `devicePixelRatio`。端到端测试里可以用 `window.__TAURI__.event.emit('tauri://drag-drop', { paths, position })` 模拟；真的从资源管理器拖一次，调试通道做不到，得手动试。
- **「页面登记一个全局处理函数、卸载时撤掉」要带编号**：换页时新页面先挂载、旧页面后卸载，旧页面无条件地撤，会把新页面刚登记的撤掉（`AppState::accept_file_drops`）。
- **`on:blur` 里读信号要用 `try_` 系列**（`try_get_untracked` / `try_set`）：对话框或页面关掉时，正聚焦的输入框会在被移除的瞬间收到 `blur`，而那时它的信号已经随作用域销毁了——直接 `get` 会 panic（`…has already been disposed`）。`NumInput` 和两处重命名输入框踩过。同理：`For` 的 `key` 要包含**会变的显示内容**，键没变的行不会重画（三档建议价曾因此显示旧毛利）。

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
& "$env:LOCALAPPDATA\ai.printpilot\cad-engine\venv\Scripts\python.exe" -X utf8 -m unittest discover -s crates\pp-cad\py
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
- **在线升级**（`docs/adr/0010-online-update.md`）：配了仓库密钥 `TAURI_SIGNING_PRIVATE_KEY`（+ `…_PASSWORD`，和 velo 同一把）之后，每个平台**额外**出一个不带引擎的精简升级包（`…-update.exe` / `….app.tar.gz`）+ 签名，`publish` 拼出 `latest.json`；引擎包单独发到 `engine-<引擎版本>` 这个 Release（同一版本只传一次）。这些都是附加步骤：没配密钥就跳过，失败不拦发版。仓库是私有的，Release 附件匿名下载不了——要么把发布物同步到 `dl.zotrus.com/printpilot/` 并设仓库变量 `UPDATE_DOWNLOAD_BASE`，要么公开仓库。**改引擎包的构建方式时注意**：应用用 `ruzstd` 解码，zstd 窗口不能超过 2^26（100 MiB 上限）；换了引擎版本，用户会被要求单独下载一次引擎。
- 还没做：Windows 代码签名、macOS Developer ID 签名与公证——需要负责人提供证书。

## 代码结构

见 `docs/03-architecture.md` §4。依赖方向：`src-tauri → pp-core → (pp-db, pp-providers, pp-agent, pp-channels, pp-geometry, pp-cad)`；所有 crate 可依赖 `pp-common`；前端只依赖 `pp-common`（不开 `backend` feature）。

## 上架与发布包（M5）

设计见 `docs/adr/0009-publish-pack.md`。动这块之前要知道的：

- **红线（ADR-0002）**：不碰任何平台。「在电脑上发布」只是用默认浏览器打开官方发布页；「用手机发布」只是一个只读的局域网页面。不要加任何「替用户点发布 / 定时发 / 模拟登录」的东西，哪怕是可选项。
- 渠道规格、词表、检查规则在 `pp_common::publish`，是**前后端共用的纯函数**：界面实时用它，`publish_pack_build` 出包时再判一次——后端那次才算数。新增一条检查：加 `LintCode` + `locales/*.json` 的 `publish.l_*` + `view/publish/mod.rs::issue_text` 的分支（有单测守着每个 code 都有文案）。
- 词表用词组不用单字（「最」会误伤「最近」）；加词时顺手想一下误伤，并写进 `a_clean_note_passes` 这类用例。
- 发布包的文件夹名用 ID 的**尾段**：ID 是 UUIDv7，前缀是时间戳，取前几位会撞。
- 手机页服务（`src-tauri/src/share.rs`）：随机端口、令牌在路径里、只认 GET、连接串行处理——所以等请求行的超时要短（浏览器会预开空闲连接）。自动化验证用 `lan: false`（只听回环，不触发系统防火墙的授权提示）。
- 自动保存和定价页一样「停手 600 ms 落盘」，但要**和上次存的快照比，变了才存**：只是点开一篇笔记不该刷新它的修改时间（列表按修改时间排）。

## 右键菜单

设计与取舍见 `docs/adr/0008-context-menus.md`，代码在 `src/ui/context_menu.rs`。

- 浏览器自带的右键菜单被全局拦掉了（`context_menu::install`）。输入框、图片、选中的文字有通用菜单，不用管。
- **新增一种可以右键的对象**（列表行、卡片…）：元素上写 `on:contextmenu=move |ev| state.open_menu(&ev, vec![item(…), separator(), item(…).danger()])`。文案用 `td_string!(current_locale(), …)`（回调里没有 i18n 上下文）；菜单里**只放界面上已经有的操作**；做不了的用 `.disabled_if(..)` 置灰，不要不列；破坏性的放最后、`.danger()`、照样走确认框。
- 元素里面有图片或可选中的文字时，菜单开头接上 `basics(state, &ev)`（「复制」选中的文字 / 图片的那几项）——具体元素的菜单会拦住事件，全局兜底看不到这次右键。
- 操作要能**对指定的对象**做，而不是「对当前打开的那个」：右键的那一行未必是打开着的。参考建模页的 `delete_target` / `rename_pending`。
- 验证用 `py scripts/cdp.py rclick <x> <y>`（真实右键）+ `click`：剪切 / 复制需要用户手势，合成事件测不出来。

## 代码建模（build123d）与建模工作室

设计与实测见 `docs/adr/0003-code-cad-build123d.md`（引擎、沙箱、流水线）和 `docs/adr/0004-modeling-studio.md`（一级入口、设计、对话式修改），流水线图见 `docs/03-architecture.md` §8.0。动这块代码之前要知道的：

- **建模是一级入口**（`Route::Studio`，`src/view/studio/`），不要再往「预研」里塞。核心实体是**设计**（`cad_designs`）：参考图 + 规格 + 对话时间线（`cad_messages`）+ 版本树。
- **对话的意图判断和干活是同一次模型调用**（`pp-agent::chat`）：有 ```python 围栏 = 改模型，没有 = 回答 / 反问。别加「先分类再执行」的第二次调用。
- **一轮对话要么整轮入库，要么什么都不留**（`command/design.rs::design_send`）：先调模型、后写库；失败时输入框里的话原样留着。
- 撤销和分叉靠版本树（选回 `base_version_id`），不删任何东西。
- **回答是流式的**：`LlmProvider::chat_stream` → `CadProgress::Delta` → 后端攒 50 毫秒发一次 `cad-stream` 事件 → 前端 `state.cad_stream`（`LiveAnswer`）。只有给人看的自由文本走流式；JSON 模式的调用不流。界面上正文显示那句话，代码只报行数。
- **一轮可以中途取消**（`src-tauri/src/turns.rs`，`cancel_turn(scope)`，作用域键 `design:<id>` / `board:<id>` / `research`）：**只把花时间的那一段包进 `turn_guard.run(..)`**（问模型、出图、跑脚本），落盘入库那一段不包——这样取消时「要么整轮入库，要么什么都不留」依然成立。新增一个会等很久的命令时照这个写。取消会连建模引擎里正在跑的脚本一起杀（`pp_cad::CancelFlag`）；手动运行代码、改参数的重建也走同一个开关（界面忙的时候一直有「停止」）。错误码 `cancelled` 不是故障，前端当普通提示显示。
- 3D 视图的容器必须在组件创建时就在 DOM 里：不要把它放进 `<Show>` 分支（挂载的 Effect 只跑一次，容器晚出现就挂不上）。无内容时用覆盖层。

- **分层**：`pp-cad` 管「执行一段代码」（引擎定位、沙箱、常驻进程 `Worker` 和它的槽位 `WorkerPool`——进程被杀 / 跑满任务数之后在后台换新、参数解析与改写、代码契约）；`pp-agent::cad` 管「和模型来回」（出规格、生成、修复循环、指令修补、复核），通过 `CadExecutor` trait 拿执行能力，所以流水线测试不需要引擎；`src-tauri/src/command/cad.rs` 提供真执行器、入库、演示脚本。
- **代码契约**：`# ---- PARAMS ----`（每行 `名字 = 数值  # 单位 | 说明 | [最小, 最大]`）、每个特征一段 `# ---- FEATURE: name ----`、最终形体赋给 `result`。参数面板、改动段比对、局部修改都建立在它上面。
- **提示词里的 build123d 速查表和完整示例是被测试锁住的**（`crates/pp-cad/py/test_cheatsheet.py` 逐条在真引擎上跑）。往 `prompts/cad_code.md` 里加 API 写法，就要在那个测试里加一条；升级 build123d 之后先跑它。
- **检查结果是带类型的 `CadProblem`**：给模型的话写在 `for_model()`（英文），给用户的话在 `locales/*.json` 的 `cad.p_*`，前端 `side.rs::problem_text` 按 `kind` 分派。新增一种问题要三处一起加。
- **演示模式的脚本要过真流水线**（后端有测试守着）：演示的第一版故意带一个真实的 OpenCascade 错误，用来让「报错 → 修复」这条路在界面上看得见。
- **引擎进程是常驻的**（`AppCtx.cad_worker`，`runner.py --serve`）：冷启动约 4 秒、热的约 0.1 秒，参数面板靠它才做得到即改即见。同一个进程会跑很多段代码，所以执行器里那三道「不串味」的保险（每任务全新命名空间、AST 禁止给属性赋值、`math` 替身）不能拆；给白名单放行任何「能改到共享对象」的写法之前先想清楚。常驻进程起不来会自动退回单次执行。
- 需要真引擎的测试在没装引擎时自动跳过并打印一行说明——**CI 上看到它们「通过」不等于跑过**。

## 项目是主线（阶段门、项目中枢）

设计见 `docs/adr/0006-project-spine.md`。动之前要知道的：

- 工作台（调研 `Route::Ideas` / 图片 / 建模；定价 `Route::Pricing` 只能按项目用）都能独立用，也都能**从项目里发起**：产出挂在项目名下（`project_id`），输入框里先放一句由项目信息拼的草稿。跳转时带的东西走 `AppState.handoff`（目标页面挂载时取走，只用一次），发起动作在 `controller/project.rs`。
- **阶段门清单是从产出里算出来的**，不是手工打勾：规则全在 `pp_common::gate`（纯函数，前后端共用）。新增一个阶段的清单 = 在 `ProjectFacts` 里加计数（`pp-db/src/overview.rs` 里数出来）+ `stage_gate` 加分支 + `GateKey` 的文案。**空清单 = 没法自动判断，不是完成了。**
- **要不要写「跳过原因」由后端按证据判**（`move_project_stage`）：前端会先问，但不要相信前端——`stage_events.forced` 是后端写的。
- 工作台里做了可能改变清单的事（出图、采用、拿掉、建出模型、关联项目、调研完成、保存假设）之后要调 `state.reload_project_facts()`，否则看板卡片和中枢上的清单是旧的。
- 不自动推进阶段：清单完成只是提示 + 一个按钮。

## 成本定价器（M4）

设计见 `docs/adr/0007-cost-pricing.md`。动之前要知道的：

- **算钱的公式只有一份，在 `pp_common::cost`**（纯函数、有单测）。前端（`src/view/pricing/`）改参数时本地即时重算，后端（`command/pricing.rs`）只做存取、保存时用同一套公式算单位成本。**不要在前端或后端另写一遍公式。**
- 金额：公式和界面里是「元」的小数；落库才换成整数「分」（`pp-db/src/pricing.rs` 的 `to_fen` / `to_yuan`）。
- 一次成功率同时摊到**材料和机时**上（失败的打印同样占机器）；人工不摊。尾数价**只往上取**，所以三档建议价的实际毛利率一定 ≥ 目标。
- 成本模型里存的是数，不是对打印机 / 耗材档案的引用：选档案 = 带入。改档案、改资料库默认值都不会悄悄改掉已经定好的价。
- 「毛利率不到 50%」「每机时毛利不达标」「价格出了竞品价格带」是**提醒，不是阶段门**；阶段门只认事实（有成功的打样记录、定了价）。
- 定价页的格子是 `NumInput`（`src/ui/field.rs`）；事件回调 / 定时器里读参数用 `params_untracked()`，响应式上下文里才用 `params()`。

## 图片工作台（出图 / 改图）

设计见 `docs/adr/0005-image-studio.md`，流程图见 `docs/03-architecture.md` §8.3。和建模工作室**刻意同构**，动之前要知道的：

- **一级入口** `Route::Images`（`src/view/imagery/`）。核心实体是**画板**（`image_boards`）：参考图 + 对话时间线（`image_messages`）+ 出过的每一张图（`image_versions`，`parent_id` = 从哪张改来的）。
- **「当前选中的那张图」是对话的宾语**：点哪张哪张就是当前图，接下来的「改一下」改的就是它。编辑永远产生新图，不覆盖。
- **规划和出图是两个模型**：`pp_agent::plan_image_turn`（deepseek-flash 看图，JSON 模式，`prompts/image_plan.md`）决定 `generate / edit / reply` 并写提示词；`pp_providers::image::ImageProvider` 只管出图。提示词不让用户写。
- **`can_edit()` 是供应商抽象的核心**：MiniMax 只能文生图，通义千问图像能按指令改图。生成 → 最便宜的；编辑 → 能编辑的。**一家能编辑的都没有时不许假装能改**：规划模型会被告知、改写提示词重新生成并向用户说明主体会变；后端对 `edit` 再兜一层校验。新增供应商先想清楚它的 `can_edit`。
- **同一批图两份**：给规划模型看的缩到 1024 的 JPEG（`planner_view`）；给图片模型改的尽量是原图字节（`edit_input`）——每轮重压 JPEG 会越改越糊。
- **一轮要么整轮入库，要么什么都不留**（`command/imagery.rs::board_send`）：规划 → 出图 → 落盘入库，前两步失败什么都不写。
- 出图供应商的「测试连接」= **真的出一张图、会扣费**，界面文案里写明了，别改成静默调用。
- 场景图 / 封面常驻一句平台规则（首图必须是实物正面图、发布要声明 AI 生成）；**不做批量模板化换景**（小红书明文违规，`docs/04-integrations.md` §4.3）。
- 演示模式用本机「画师」`DemoPainter`（编辑 = 只换背景色、主体像素不动，有测试守着）；演示的回答必须自己写明「演示数据」。
- **图不一定是生成的**（[ADR-0011](docs/adr/0011-import-and-drop.md)）：自己的图可以导入——按钮（可多选）、直接拖进窗口、`Ctrl+V`。导入的图就是 `image_versions` 里 `mode = "import"` 的一行（没有提示词、`ai_generated = false`），和生成的图同等地位；三个入口共用 `import_into_board`。**落点决定去向**：画板这边 = 导入，对话栏 / 输入框里 = 只贴到这句话上当参考。入库前一律重新编码（去 EXIF——手机照片里有拍摄地点）、最长边 4096、真有透明的才存 PNG；一张坏图不让整批作废，逐个说明。送去建模时，大图和带透明的 PNG 会先另做一张 1536 的参考图（`design_reference`）。

## 拖进窗口的文件

机制是全局的，三个页面在用（图片 = 导入 / 贴参考图，建模 = 参考图，上架 = 实拍图）：

- `app::listen_file_drops` 只注册一次：悬着时写 `state.file_drag`，松手时调用当前页面用 `state.accept_file_drops(..)` 登记的处理函数；没登记的页面不收文件（拖进来的是图片就提示该拖到哪）。
- 页面要接收文件：组件体里调 `state.accept_file_drops(move |dropped| …)`（卸载时自动撤），再放一个提示层——只有一种去向用 `ui::FileDropHint`，分区的（像图片工作台）自己用 `ui::DropPanel` 拼，落点用 `getBoundingClientRect` 和 `dropped.x / y` 比。
- 「是不是图片」统一用 `pp_common::imagery::is_importable_image`（前后端同一份扩展名表）。

## 设置页

左边一列二级菜单、右边只显示选中的那一组（`state::SettingsSection`：通用 / 接口密钥 / 图片生成 / 资料库 / 关于与更新），**每一组都要在最小窗口（1080 × 680）里不用滚动就看得全**——当初就是因为一长页往下滚才改的。新增一组：枚举加一项（`ALL`、`as_str`）+ `settings.rs` 的 `SectionItem`（图标、文案）和 `SettingsView` 的 `match` 各加一支。别的页面说「去设置」时用 `state.go_settings(SettingsSection::Keys)` 直接落到那一组，不要只 `go(Route::Settings)`。

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
