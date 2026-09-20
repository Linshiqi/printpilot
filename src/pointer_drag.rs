//! 指针事件拖拽(**原样取自 velo 的 src/view/pointer_drag.rs**,copy-and-own)。
//!
//! 不用 HTML5 DnD:WebView 里它的拖影、光标和 drop 事件都不可控。这里的做法是 pointerdown 记下按压,
//! window 级监听 pointermove/up/cancel/blur/Escape;移动超过阈值才算拖拽并 setPointerCapture;
//! 放置目标用 `elementFromPoint(x, y).closest("[<attr>]")` 取属性值。配套 `PointerDragGhost` 跟手浮标,
//! 以及 `swallow_click()` 吞掉拖完之后那一次多余的 click。

use std::cell::{Cell, RefCell};

use leptos::prelude::*;
use wasm_bindgen::{closure::Closure, JsCast};

use crate::icon::{Icon, IconKind};

const THRESHOLD: f64 = 6.0;

type Listener = Closure<dyn FnMut(web_sys::Event)>;

struct Press {
    owner: u64,
    payload: String,
    label: String,
    source: web_sys::Element,
    pointer_id: i32,
    x: f64,
    y: f64,
    active: bool,
}

enum Step {
    Cancel,
    Start(String, String, f64, f64),
    Move(f64, f64),
}

thread_local! {
    static PRESS: RefCell<Option<Press>> = const { RefCell::new(None) };
    static SWALLOW_CLICK: Cell<bool> = const { Cell::new(false) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    static LISTENERS: RefCell<Vec<(u64, Listener)>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy)]
pub struct PointerDrag {
    id: u64,
    attr: &'static str,
    from_buttons: bool,
    active: RwSignal<Option<(String, String)>>,
    pos: RwSignal<(f64, f64)>,
    over: RwSignal<Option<String>>,
}

impl PointerDrag {
    pub fn new(
        attr: &'static str,
        from_buttons: bool,
        on_drop: impl Fn(String, String) + Copy + 'static,
    ) -> Self {
        let id = NEXT_ID.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        let drag = Self {
            id,
            attr,
            from_buttons,
            active: RwSignal::new(None),
            pos: RwSignal::new((0.0, 0.0)),
            over: RwSignal::new(None),
        };
        Effect::new(move |_| drag.listen(on_drop));
        drag
    }

    pub fn press(
        self,
        ev: &web_sys::PointerEvent,
        source: web_sys::Element,
        payload: String,
        label: String,
    ) {
        SWALLOW_CLICK.with(|s| s.set(false));
        if ev.button() != 0 || !ev.is_primary() || (!self.from_buttons && on_button(ev)) {
            return;
        }
        PRESS.with(|p| {
            *p.borrow_mut() = Some(Press {
                owner: self.id,
                payload,
                label,
                source,
                pointer_id: ev.pointer_id(),
                x: ev.client_x() as f64,
                y: ev.client_y() as f64,
                active: false,
            });
        });
    }

    pub fn swallow_click() -> bool {
        SWALLOW_CLICK.with(|s| s.replace(false))
    }

    pub fn is_over(self, key: &str) -> bool {
        self.over.with(|o| o.as_deref() == Some(key))
    }

    pub fn is_dragging(self, payload: &str) -> bool {
        self.active
            .with(|a| a.as_ref().is_some_and(|(p, _)| p == payload))
    }

    fn listen(self, on_drop: impl Fn(String, String) + Copy + 'static) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let handlers: [(&'static str, Box<dyn FnMut(web_sys::Event)>); 5] = [
            (
                "pointermove",
                Box::new(move |ev: web_sys::Event| self.moved(ev.unchecked_ref())),
            ),
            (
                "pointerup",
                Box::new(move |ev: web_sys::Event| self.released(ev.unchecked_ref(), on_drop)),
            ),
            (
                "pointercancel",
                Box::new(move |_: web_sys::Event| self.cancel()),
            ),
            ("blur", Box::new(move |_: web_sys::Event| self.cancel())),
            (
                "keydown",
                Box::new(move |ev: web_sys::Event| {
                    if ev.unchecked_ref::<web_sys::KeyboardEvent>().key() == "Escape" {
                        self.cancel();
                    }
                }),
            ),
        ];
        let id = self.id;
        let mut funcs = Vec::with_capacity(handlers.len());
        for (name, handler) in handlers {
            let cb = Listener::wrap(handler);
            let func = cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let _ = window.add_event_listener_with_callback(name, &func);
            LISTENERS.with(|l| l.borrow_mut().push((id, cb)));
            funcs.push((name, func));
        }
        on_cleanup(move || {
            for (name, func) in &funcs {
                let _ = window.remove_event_listener_with_callback(name, func);
            }
            LISTENERS.with(|l| l.borrow_mut().retain(|(owner, _)| *owner != id));
            PRESS.with(|p| {
                p.borrow_mut().take_if(|press| press.owner == id);
            });
        });
    }

    fn moved(self, ev: &web_sys::PointerEvent) {
        let step = PRESS.with(|p| {
            let mut slot = p.borrow_mut();
            let press = slot
                .as_mut()
                .filter(|press| press.owner == self.id && press.pointer_id == ev.pointer_id())?;
            if ev.buttons() & 1 == 0 {
                return Some(Step::Cancel);
            }
            let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
            if press.active {
                return Some(Step::Move(x, y));
            }
            if (x - press.x).hypot(y - press.y) < THRESHOLD {
                return None;
            }
            press.active = true;
            let _ = press.source.set_pointer_capture(press.pointer_id);
            Some(Step::Start(
                press.payload.clone(),
                press.label.clone(),
                x,
                y,
            ))
        });
        match step {
            Some(Step::Cancel) => self.cancel(),
            Some(Step::Start(payload, label, x, y)) => {
                self.active.set(Some((payload, label)));
                self.hover(x, y);
            }
            Some(Step::Move(x, y)) => self.hover(x, y),
            None => {}
        }
    }

    fn hover(self, x: f64, y: f64) {
        self.pos.set((x, y));
        let over = self.target_at(x, y);
        if self.over.with_untracked(|o| *o != over) {
            self.over.set(over);
        }
    }

    fn released(self, ev: &web_sys::PointerEvent, on_drop: impl Fn(String, String)) {
        let Some(press) = PRESS.with(|p| {
            p.borrow_mut()
                .take_if(|press| press.owner == self.id && press.pointer_id == ev.pointer_id())
        }) else {
            return;
        };
        if !press.active {
            return;
        }
        self.clear();
        SWALLOW_CLICK.with(|s| s.set(true));
        if let Some(key) = self.target_at(ev.client_x() as f64, ev.client_y() as f64) {
            on_drop(press.payload, key);
        }
    }

    fn cancel(self) {
        let active = PRESS
            .with(|p| p.borrow_mut().take_if(|press| press.owner == self.id))
            .is_some_and(|press| press.active);
        if active {
            SWALLOW_CLICK.with(|s| s.set(true));
            self.clear();
        }
    }

    fn clear(self) {
        self.active.set(None);
        if self.over.with_untracked(Option::is_some) {
            self.over.set(None);
        }
    }

    fn target_at(self, x: f64, y: f64) -> Option<String> {
        web_sys::window()?
            .document()?
            .element_from_point(x as f32, y as f32)?
            .closest(&format!("[{}]", self.attr))
            .ok()
            .flatten()?
            .get_attribute(self.attr)
    }
}

fn on_button(ev: &web_sys::PointerEvent) -> bool {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|el| el.closest("button").ok().flatten())
        .is_some()
}

#[component]
pub fn PointerDragGhost(drag: PointerDrag, icon: IconKind) -> impl IntoView {
    view! {
        <Show when=move || drag.active.with(Option::is_some)>
            <div
                class="fixed z-[90] pointer-events-none flex items-center gap-1.5 max-w-[260px] px-2.5 py-1.5 rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 shadow-lg text-xs text-gray-700 dark:text-gray-200"
                style:left=move || format!("{}px", drag.pos.get().0 + 14.0)
                style:top=move || format!("{}px", drag.pos.get().1 + 10.0)
            >
                <Icon kind=icon class="w-3.5 h-3.5 shrink-0"/>
                <span class="truncate">
                    {move || drag.active.with(|a| a.as_ref().map(|(_, label)| label.clone()).unwrap_or_default())}
                </span>
            </div>
        </Show>
    }
}
