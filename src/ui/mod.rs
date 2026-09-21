//! 组件库。**页面只许用这里的组件,不许手写重复的 Tailwind 类串**(CLAUDE.md)。
//!
//! 这一点刻意和 velo 反着做:velo 的 `ui.rs` 很薄,按钮、对话框都是各处手写类串;
//! PrintPilot 是表单密集型应用,那样做很快就会失控。
//!
//! 写组件时的三个 Leptos 0.7 注意点:
//! - 文案 prop 用 `TextProp`(同时接受 `&'static str`、`String`、闭包);
//! - 要在 `<Show>` 里用的 props 先放进 `StoredValue`(`Show` 的子视图是 `Fn`,不能把值 move 出去);
//! - 回调 prop 用 `Callback<()>`,调用方直接传 `move || …`。

mod badge;
mod button;
mod card;
pub mod context_menu;
mod dialog;
mod drop_hint;
mod empty;
mod field;
mod tabs;
mod toast;

pub use badge::{Badge, Tone};
pub use button::{Button, ButtonVariant, IconButton};
pub use card::{Card, SectionTitle};
pub use context_menu::{basics, copy_entry, image_entries, item, separator, toggle, ContextMenu, ContextMenuHost};
pub use dialog::{Dialog, DialogFooter, Drawer};
pub use drop_hint::{DropPanel, FileDropHint};
pub use empty::EmptyState;
pub use field::{Field, NumInput, TextArea, TextInput, Toggle};
pub use tabs::Segmented;
pub use toast::Toasts;
