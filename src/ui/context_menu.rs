//! 右键菜单。整体做法参考 velo(全局拦截 + 编辑菜单 + 一个宿主组件),但**菜单项由调用方给**:
//! PrintPilot 的操作散在各个页面的闭包里,没有 velo 那样一个总控制器可以让宿主自己去调。
//!
//! 三层:
//! 1. **全局兜底**([`install`]):窗口级的 `contextmenu` 监听,一律拦掉浏览器自带的菜单——
//!    「返回 / 刷新 / 另存为 / 打印 / 检查」在桌面应用里没有一项是对的,「刷新」还会把整个界面的状态冲掉。
//!    落在输入框里 → 编辑菜单(剪切 / 复制 / 粘贴 / 全选);落在图片上 → 复制图片 / 用默认程序打开;
//!    选中了一段文字 → 复制;其余位置不出菜单。
//! 2. **具体对象的菜单**:元素上写 `on:contextmenu=move |ev| state.open_menu(&ev, vec![...])`。
//!    事件不做委托(没开 leptos 的 `delegation`),所以最里层的元素先收到;`open_menu` 会 `stop_propagation`,
//!    外层和全局兜底就不会再插手。菜单里只放界面上**已经有**的操作——它是捷径,不是另一套功能。
//! 3. **宿主**([`ContextMenuHost`]):App 里挂一次,负责画、定位、关闭、键盘操作。
//!
//! 菜单从不抢焦点:项目上 `mousedown` 被 `prevent_default`,所以「剪切 / 复制 / 粘贴」执行时焦点还在原来的输入框里。

use std::sync::Arc;

use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::td_string;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::i18n_util::current_locale;
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::AppState;

pub type MenuAction = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct MenuItem {
    pub label: String,
    pub icon: Option<IconKind>,
    /// 右侧的灰字:快捷键,或者一句补充
    pub hint: Option<String>,
    pub danger: bool,
    pub disabled: bool,
    /// `Some` = 开关项,前面画勾(或留空)
    pub checked: Option<bool>,
    pub action: MenuAction,
}

#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

/// 一项带图标的菜单。文案在打开菜单的那一刻取当前语言(事件回调里拿不到 i18n 上下文,用 `td_string!`)。
pub fn item(label: impl Into<String>, icon: IconKind, action: impl Fn() + Send + Sync + 'static) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        label: label.into(),
        icon: Some(icon),
        hint: None,
        danger: false,
        disabled: false,
        checked: None,
        action: Arc::new(action),
    })
}

/// 开关项:前面画勾。
pub fn toggle(label: impl Into<String>, on: bool, action: impl Fn() + Send + Sync + 'static) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        label: label.into(),
        icon: None,
        hint: None,
        danger: false,
        disabled: false,
        checked: Some(on),
        action: Arc::new(action),
    })
}

pub fn separator() -> MenuEntry {
    MenuEntry::Separator
}

impl MenuEntry {
    fn map(self, f: impl FnOnce(&mut MenuItem)) -> Self {
        match self {
            MenuEntry::Item(mut it) => {
                f(&mut it);
                MenuEntry::Item(it)
            }
            other => other,
        }
    }

    /// 破坏性的操作(删除、拿掉):红字。
    pub fn danger(self) -> Self {
        self.map(|it| it.danger = true)
    }

    /// 现在做不了的操作照样列出来、置灰——比「这次有、下次没有」好找。
    pub fn disabled_if(self, off: bool) -> Self {
        self.map(|it| it.disabled = off)
    }

    pub fn hint(self, text: impl Into<String>) -> Self {
        self.map(|it| it.hint = Some(text.into()))
    }

    fn is_separator(&self) -> bool {
        matches!(self, MenuEntry::Separator)
    }
}

/// 开着的菜单:点击位置(视口坐标)+ 菜单项。
#[derive(Clone)]
pub struct ContextMenu {
    pub x: f64,
    pub y: f64,
    pub entries: Vec<MenuEntry>,
}

/// 去掉开头、结尾和连着的分隔线(调用方按条件拼菜单时,哪一组可能整组都没有)。
pub fn tidy(entries: Vec<MenuEntry>) -> Vec<MenuEntry> {
    let mut out: Vec<MenuEntry> = Vec::with_capacity(entries.len());
    for e in entries {
        if e.is_separator() && out.last().is_none_or(MenuEntry::is_separator) {
            continue;
        }
        out.push(e);
    }
    while out.last().is_some_and(MenuEntry::is_separator) {
        out.pop();
    }
    out
}

/// 菜单的宽度上限(和宿主里的 `max-w-64` 一致),定位时按它留出右边的空间。
const MENU_MAX_W: f64 = 256.0;
const EDGE: f64 = 4.0;

/// 菜单放在哪:不伸出视口。高度事先不知道,所以不去量——点在视口下半部分就**向上**展开(用 `bottom` 定位)。
pub fn place(x: f64, y: f64, vw: f64, vh: f64) -> String {
    let left = x.min(vw - MENU_MAX_W - EDGE).max(EDGE);
    if y > vh * 0.55 {
        format!("left:{left:.0}px;bottom:{:.0}px", (vh - y).max(EDGE))
    } else {
        format!("left:{left:.0}px;top:{:.0}px", y.max(EDGE))
    }
}

/// 键盘上下键:从 `current` 往下 / 往上找下一个能点的项,到头绕回去。没有能点的项 → `None`。
pub fn step(entries: &[MenuEntry], current: Option<usize>, down: bool) -> Option<usize> {
    let n = entries.len();
    let usable = |i: usize| matches!(&entries[i], MenuEntry::Item(it) if !it.disabled);
    let start = match (current, down) {
        (Some(i), true) => i + 1,
        (Some(i), false) => i + n - 1,
        (None, true) => 0,
        (None, false) => n.saturating_sub(1),
    };
    (0..n).map(|k| if down { (start + k) % n } else { (start + n - k) % n }).find(|&i| usable(i))
}

/// `<img src>` → 资产 ID。资产的 URL 是 `http://pp-asset.localhost/<id>`(Windows)或 `pp-asset://localhost/<id>`。
pub fn asset_id_from_src(src: &str) -> Option<String> {
    if !src.contains("pp-asset") {
        return None;
    }
    let tail = src.split(['?', '#']).next()?.rsplit('/').next()?;
    let ok = (8..=64).contains(&tail.len()) && tail.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    ok.then(|| tail.to_string())
}

impl AppState {
    /// 在鼠标的位置打开一个菜单。`entries` 整理之后是空的 → 不出菜单(浏览器自带的那个照样拦掉)。
    pub fn open_menu(self, ev: &web_sys::MouseEvent, entries: Vec<MenuEntry>) {
        ev.prevent_default();
        ev.stop_propagation();
        let entries = tidy(entries);
        self.context_menu.set((!entries.is_empty()).then(|| ContextMenu {
            x: ev.client_x() as f64,
            y: ev.client_y() as f64,
            entries,
        }));
    }
}

// ---------------------------------------------------------------- 全局兜底

fn exec(command: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let _ = doc.unchecked_into::<web_sys::HtmlDocument>().exec_command(command);
    }
}

/// 往当前焦点的输入框里插入文字。走 `insertText`:会触发 `input` 事件(界面上绑的信号跟着变),也进得了撤销栈。
fn insert_text(text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let _ = doc.unchecked_into::<web_sys::HtmlDocument>().exec_command_with_show_ui_and_value("insertText", false, text);
    }
}

fn page_selection() -> String {
    web_sys::window()
        .and_then(|w| w.get_selection().ok().flatten())
        .map(|s| String::from(s.to_string()))
        .unwrap_or_default()
}

/// 输入框:`(可写, 框里有选中的文字)`。不是输入框 → `None`。
fn field_state(el: &web_sys::Element) -> Option<(bool, bool)> {
    let tag = el.tag_name().to_ascii_uppercase();
    let (start, end, locked) = if tag == "TEXTAREA" {
        let t = el.dyn_ref::<web_sys::HtmlTextAreaElement>()?;
        (t.selection_start().ok().flatten(), t.selection_end().ok().flatten(), t.read_only() || t.disabled())
    } else if tag == "INPUT" {
        let i = el.dyn_ref::<web_sys::HtmlInputElement>()?;
        // 勾选框、滑块、文件框没有「文字」可言
        if matches!(i.type_().as_str(), "checkbox" | "radio" | "range" | "file" | "button" | "submit" | "color") {
            return None;
        }
        // number 之类的框读 selectionStart 会抛异常:当作没有选中
        (i.selection_start().ok().flatten(), i.selection_end().ok().flatten(), i.read_only() || i.disabled())
    } else {
        return None;
    };
    Some((!locked, matches!((start, end), (Some(a), Some(b)) if b > a)))
}

fn edit_menu(writable: bool, has_selection: bool) -> Vec<MenuEntry> {
    let l = current_locale();
    vec![
        item(td_string!(l, menu.cut), IconKind::Scissors, || exec("cut")).hint("Ctrl+X").disabled_if(!(writable && has_selection)),
        item(td_string!(l, menu.copy), IconKind::Copy, || exec("copy")).hint("Ctrl+C").disabled_if(!has_selection),
        item(td_string!(l, menu.paste), IconKind::Clipboard, || {
            spawn_local(async {
                // 读剪贴板走后端:WebView 自己读要弹权限框
                if let Ok(text) = ipc::call_no_args::<String>(cmd::READ_CLIPBOARD_TEXT).await {
                    insert_text(&text);
                }
            });
        })
        .hint("Ctrl+V")
        .disabled_if(!writable),
        separator(),
        item(td_string!(l, menu.select_all), IconKind::Check, || exec("selectAll")).hint("Ctrl+A"),
    ]
}

/// 任意一张资产图片都有的两项。具体页面(画板的胶片格)会在前面加自己的操作。
pub fn image_entries(state: AppState, asset_id: String) -> Vec<MenuEntry> {
    let l = current_locale();
    let (copy_id, open_id) = (asset_id.clone(), asset_id);
    vec![
        item(td_string!(l, menu.copy_image), IconKind::Copy, move || {
            let id = copy_id.clone();
            spawn_local(async move {
                match ipc::call_unit(cmd::COPY_ASSET_IMAGE, &serde_json::json!({ "id": id })).await {
                    Ok(()) => state.notify_info(td_string!(current_locale(), menu.image_copied)),
                    Err(e) => state.notify_error(e),
                }
            });
        }),
        item(td_string!(l, menu.open_external), IconKind::External, move || {
            let id = open_id.clone();
            spawn_local(async move {
                if let Err(e) = ipc::call_unit(cmd::OPEN_ASSET_EXTERNAL, &serde_json::json!({ "id": id })).await {
                    state.notify_error(e);
                }
            });
        }),
    ]
}

/// 复制一段文字到剪贴板(写剪贴板不需要权限,直接用 WebView 的)。
pub fn copy_text(state: AppState, text: String) {
    if let Some(w) = web_sys::window() {
        let _ = w.navigator().clipboard().write_text(&text);
        state.notify_info(td_string!(current_locale(), menu.copied));
    }
}

/// 「复制」一段现成的文字(对话里的一句话、一段代码)。
pub fn copy_entry(state: AppState, label: impl Into<String>, text: String) -> MenuEntry {
    item(label, IconKind::Copy, move || copy_text(state, text.clone()))
}

/// 具体对象的菜单开头都该带上的两样:选中了文字 →「复制」;点在一张资产图片上 → 图片的那几项。
/// 具体元素的菜单会拦住事件,全局兜底看不到这次右键——所以由它们自己带上。
pub fn basics(state: AppState, ev: &web_sys::MouseEvent) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    if !page_selection().trim().is_empty() {
        entries.push(item(td_string!(current_locale(), menu.copy), IconKind::Copy, || exec("copy")).hint("Ctrl+C"));
        entries.push(separator());
    }
    let image = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlImageElement>().ok()).and_then(|img| asset_id_from_src(&img.src()));
    if let Some(id) = image {
        entries.extend(image_entries(state, id));
        entries.push(separator());
    }
    entries
}

/// 装上全局兜底。App 启动时调一次。
pub fn install(state: AppState) {
    let Some(window) = web_sys::window() else { return };
    let handler = Closure::<dyn FnMut(web_sys::MouseEvent)>::wrap(Box::new(move |ev: web_sys::MouseEvent| {
        // 走到这里 = 没有哪个元素给出自己的菜单(给了的会 stop_propagation)
        let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else {
            ev.prevent_default();
            return;
        };
        let entries = match field_state(&el) {
            Some((writable, has_selection)) => edit_menu(writable, has_selection),
            None => basics(state, &ev),
        };
        state.open_menu(&ev, entries);
    }));
    let _ = window.add_event_listener_with_callback("contextmenu", handler.as_ref().unchecked_ref());
    handler.forget(); // 和应用同寿命
}

// ---------------------------------------------------------------- 宿主

fn viewport() -> (f64, f64) {
    let size = |v: Result<JsValue, JsValue>, fallback: f64| v.ok().and_then(|v| v.as_f64()).unwrap_or(fallback);
    web_sys::window().map(|w| (size(w.inner_width(), 1280.0), size(w.inner_height(), 800.0))).unwrap_or((1280.0, 800.0))
}

const ITEM: &str = "w-full flex items-center gap-2 px-3 py-1.5 text-start text-xs text-gray-700 dark:text-gray-200 hover:bg-gray-100 dark:hover:bg-gray-700";
const ITEM_DANGER: &str = "w-full flex items-center gap-2 px-3 py-1.5 text-start text-xs text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/30";
const ITEM_OFF: &str = "w-full flex items-center gap-2 px-3 py-1.5 text-start text-xs text-gray-400 dark:text-gray-600 cursor-default";

#[component]
pub fn ContextMenuHost(state: AppState) -> impl IntoView {
    // 键盘高亮的那一项(entries 里的下标)。每次打开新菜单清掉
    let active = RwSignal::new(None::<usize>);
    let close = move || state.context_menu.set(None);
    Effect::new(move |_| {
        state.context_menu.track();
        active.set(None);
    });

    let run = move |index: usize| {
        let action = state.context_menu.with_untracked(|m| {
            m.as_ref().and_then(|m| match m.entries.get(index) {
                Some(MenuEntry::Item(it)) if !it.disabled => Some(it.action.clone()),
                _ => None,
            })
        });
        if let Some(action) = action {
            close();
            action();
        }
    };

    // 菜单开着的时候:Esc 关、上下键选、回车执行。菜单不抢焦点,所以在窗口上听;开着时把这几个键吃掉,
    // 不让它们落到后面的输入框里。
    let keys = window_event_listener(ev::keydown, move |ev| {
        if state.context_menu.with_untracked(Option::is_none) {
            return;
        }
        match ev.key().as_str() {
            "Escape" => close(),
            "ArrowDown" | "ArrowUp" => {
                let down = ev.key() == "ArrowDown";
                let next = state.context_menu.with_untracked(|m| m.as_ref().and_then(|m| step(&m.entries, active.get_untracked(), down)));
                active.set(next);
            }
            "Enter" | " " => match active.get_untracked() {
                Some(i) => run(i),
                None => return,
            },
            _ => return,
        }
        ev.prevent_default();
        ev.stop_propagation();
    });
    // 窗口失焦、改尺寸、滚动:菜单的位置已经不对了,关掉
    let blur = window_event_listener(ev::blur, move |_| close());
    let resize = window_event_listener(ev::resize, move |_| close());
    let wheel = window_event_listener(ev::wheel, move |_| {
        if state.context_menu.with_untracked(Option::is_some) {
            close();
        }
    });
    on_cleanup(move || {
        keys.remove();
        blur.remove();
        resize.remove();
        wheel.remove();
    });

    // 在菜单外面再点一次右键:关掉这个,并把这次右键转交给底下的元素(系统菜单就是这样的手感)
    let retarget = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        close();
        let (x, y) = (ev.client_x(), ev.client_y());
        // 底板这一刻还在 DOM 里:等它撤掉再找底下是谁
        request_animation_frame(move || {
            let Some(target) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.element_from_point(x as f32, y as f32)) else {
                return;
            };
            let init = web_sys::MouseEventInit::new();
            init.set_bubbles(true);
            init.set_cancelable(true);
            init.set_client_x(x);
            init.set_client_y(y);
            init.set_button(2);
            if let Ok(again) = web_sys::MouseEvent::new_with_mouse_event_init_dict("contextmenu", &init) {
                let _ = target.dispatch_event(&again);
            }
        });
    };

    view! {
        {move || state.context_menu.get().map(|menu| {
            let (vw, vh) = viewport();
            let style = place(menu.x, menu.y, vw, vh);
            let rows = menu.entries.into_iter().enumerate().map(|(i, entry)| match entry {
                MenuEntry::Separator => view! { <div class="my-1 border-t border-gray-100 dark:border-gray-700" role="separator"></div> }.into_any(),
                MenuEntry::Item(it) => {
                    let base = if it.disabled { ITEM_OFF } else if it.danger { ITEM_DANGER } else { ITEM };
                    let disabled = it.disabled;
                    let lead = match (it.checked, it.icon) {
                        (Some(true), _) => view! { <Icon kind=IconKind::Check class="w-3.5 h-3.5 shrink-0"/> }.into_any(),
                        (Some(false), _) | (None, None) => view! { <span class="w-3.5 h-3.5 shrink-0"></span> }.into_any(),
                        (None, Some(kind)) => view! { <Icon kind=kind class="w-3.5 h-3.5 shrink-0"/> }.into_any(),
                    };
                    view! {
                        <button
                            type="button"
                            role=if it.checked.is_some() { "menuitemcheckbox" } else { "menuitem" }
                            aria-disabled=disabled.to_string()
                            aria-checked=it.checked.map(|c| c.to_string())
                            class=base
                            class=("bg-gray-100", move || !disabled && active.get() == Some(i))
                            class=("dark:bg-gray-700", move || !disabled && active.get() == Some(i))
                            // 不让菜单项拿走焦点:编辑菜单的操作要作用在原来那个输入框上
                            on:mousedown=move |ev| ev.prevent_default()
                            on:mousemove=move |_| if !disabled && active.get_untracked() != Some(i) { active.set(Some(i)) }
                            on:click=move |_| run(i)
                        >
                            {lead}
                            <span class="flex-1 min-w-0 truncate">{it.label}</span>
                            {it.hint.map(|h| view! { <span class="shrink-0 pl-4 text-[10px] text-gray-400 dark:text-gray-500">{h}</span> })}
                        </button>
                    }.into_any()
                }
            }).collect_view();

            view! {
                <div class="fixed inset-0 z-[300]" on:mousedown=move |_| close() on:contextmenu=retarget></div>
                <div
                    role="menu"
                    data-context-menu=""
                    class="fixed z-[301] min-w-44 max-w-64 py-1 rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 shadow-2xl select-none \
                           max-h-[calc(100vh-16px)] overflow-y-auto animate-menu-in"
                    style=style
                    on:contextmenu=move |ev| { ev.prevent_default(); ev.stop_propagation(); }
                >
                    {rows}
                </div>
            }
        })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(label: &str) -> MenuEntry {
        item(label, IconKind::Check, || {})
    }

    fn labels(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                MenuEntry::Item(it) => it.label.clone(),
                MenuEntry::Separator => "-".into(),
            })
            .collect()
    }

    #[test]
    fn stray_separators_are_dropped() {
        let menu = tidy(vec![separator(), named("打开"), separator(), separator(), named("删除"), separator()]);
        assert_eq!(labels(&menu), ["打开", "-", "删除"]);
        assert!(tidy(vec![separator(), separator()]).is_empty(), "只剩分隔线 = 没有菜单");
        assert!(tidy(Vec::new()).is_empty());
    }

    #[test]
    fn the_menu_stays_inside_the_viewport() {
        // 左上角附近:就放在鼠标的位置
        assert_eq!(place(100.0, 80.0, 1280.0, 800.0), "left:100px;top:80px");
        // 贴着右边:往左让,菜单整个还在视口里
        assert_eq!(place(1270.0, 80.0, 1280.0, 800.0), "left:1020px;top:80px");
        // 视口下半部分:向上展开(高度事先不知道,用 bottom 定位)
        assert_eq!(place(100.0, 700.0, 1280.0, 800.0), "left:100px;bottom:100px");
        // 窗口比菜单还窄:至少不伸到左边外面去
        assert_eq!(place(50.0, 10.0, 200.0, 800.0), "left:4px;top:10px");
    }

    #[test]
    fn arrow_keys_skip_separators_and_disabled_items_and_wrap_around() {
        let menu = vec![named("a"), separator(), named("b").disabled_if(true), named("c")];
        assert_eq!(step(&menu, None, true), Some(0));
        assert_eq!(step(&menu, Some(0), true), Some(3), "跳过分隔线和置灰的项");
        assert_eq!(step(&menu, Some(3), true), Some(0), "到底了绕回开头");
        assert_eq!(step(&menu, None, false), Some(3), "第一次按上键 = 最后一项");
        assert_eq!(step(&menu, Some(0), false), Some(3));
        assert_eq!(step(&menu, Some(3), false), Some(0));
        let dead = vec![named("x").disabled_if(true), separator()];
        assert_eq!(step(&dead, None, true), None, "没有能点的项");
        assert_eq!(step(&[], None, true), None);
    }

    #[test]
    fn asset_ids_are_read_back_from_image_urls() {
        let id = "0192f3a4-7b1c-7def-8a90-1234567890ab";
        assert_eq!(asset_id_from_src(&format!("http://pp-asset.localhost/{id}")).as_deref(), Some(id));
        assert_eq!(asset_id_from_src(&format!("pp-asset://localhost/{id}?v=2")).as_deref(), Some(id));
        for other in ["https://example.com/a.png", "data:image/png;base64,AAAA", "http://pp-asset.localhost/", "http://pp-asset.localhost/../../x", ""] {
            assert_eq!(asset_id_from_src(other), None, "{other:?}");
        }
    }

    #[test]
    fn builders_mark_items_without_touching_separators() {
        let MenuEntry::Item(it) = named("删除").danger().hint("Del").disabled_if(true) else { panic!() };
        assert!(it.danger && it.disabled && it.hint.as_deref() == Some("Del") && it.checked.is_none());
        let MenuEntry::Item(on) = toggle("棱线", true, || {}) else { panic!() };
        assert_eq!((on.checked, on.icon), (Some(true), None));
        assert!(separator().danger().is_separator());
    }
}
