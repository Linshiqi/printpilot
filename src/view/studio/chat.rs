//! 建模工作室 · 对话栏。时间线上有三种东西:用户的话、AI 的消息(回答 / 规格卡 / 改好了 / 没改成 / 复核结论)、
//! 以及事件(手动改参数、手改代码)。下面是输入区:文字 + 贴图 + 点选位置 + 模型档位。

use leptos::prelude::*;
use leptos_i18n::{t_string, td_string};
use pp_common::cad::DesignSpec;
use pp_common::design::{CadDesign, CadMessage, CadTier, MsgKind, MsgRole};
use pp_common::Asset;

use super::side::{changed_text, ReportCard};
use super::spec_editor::dims_to_text;
use crate::i18n::use_i18n;
use crate::i18n_util::current_locale;
use crate::icon::{Icon, IconKind};
use crate::ipc;
use crate::ui::{Badge, Button, ButtonVariant, IconButton, Segmented, Toggle, Tone};
use crate::utils::fmt_mm;
use crate::viewer3d::Pick;

/// 对话栏能触发的动作(都是 `Copy` 的回调,整个结构体可以直接 move 进闭包)。
#[derive(Clone, Copy)]
pub struct ChatActions {
    pub send: Callback<()>,
    pub attach: Callback<()>,
    pub generate: Callback<(DesignSpec,)>,
    pub edit_spec: Callback<(DesignSpec,)>,
    pub select_version: Callback<(String,)>,
    /// 把一句现成的话当作下一条消息发出去(复核差异的「按此修改」)
    pub say: Callback<(String,)>,
    /// 没改成的那一版代码 → 放进代码编辑器
    pub load_code: Callback<(String,)>,
}

fn size_text(v: [f64; 3]) -> String {
    format!("{} × {} × {} mm", fmt_mm(v[0]), fmt_mm(v[1]), fmt_mm(v[2]))
}

#[component]
pub fn ChatPane(
    messages: RwSignal<Vec<CadMessage>>,
    design: RwSignal<Option<CadDesign>>,
    busy: RwSignal<bool>,
    #[prop(into)] progress_text: Signal<String>,
    #[prop(into)] can_send: Signal<bool>,
    text: RwSignal<String>,
    pending_images: RwSignal<Vec<Asset>>,
    tier: RwSignal<CadTier>,
    pick_mode: RwSignal<bool>,
    picked: RwSignal<Option<Pick>>,
    actions: ChatActions,
) -> impl IntoView {
    let i18n = use_i18n();
    let has_model = move || design.with(|d| d.as_ref().is_some_and(|d| d.current_version_id.is_some()));
    // 只有最后一张规格卡上的按钮是活的:旧规格已经被后面的话修正过了
    let last_spec_id = Memo::new(move |_| {
        messages.with(|list| list.iter().rev().find(|m| m.kind == MsgKind::Spec).map(|m| m.id.clone()))
    });
    let current_version = Memo::new(move |_| design.with(|d| d.as_ref().and_then(|d| d.current_version_id.clone())));

    // 有新消息 / 开始干活时滚到底
    let scroller = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let _ = (messages.with(Vec::len), busy.get());
        request_animation_frame(move || {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        });
    });

    let label = move |f: fn(crate::i18n::Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    let suggestions = move || {
        [
            t_string!(i18n, studio.suggest_chamfer),
            t_string!(i18n, studio.suggest_bigger),
            t_string!(i18n, studio.suggest_support),
        ]
    };

    view! {
        <div class="h-full flex flex-col min-h-0">
            <div node_ref=scroller class="flex-1 min-h-0 overflow-y-auto p-3 space-y-3">
                <Show when=move || messages.with(Vec::is_empty) && !busy.get()>
                    <div class="pt-8 px-3 text-center space-y-2">
                        <div class="mx-auto w-10 h-10 rounded-xl bg-brand-soft dark:bg-indigo-500/20 flex items-center justify-center text-brand">
                            <Icon kind=IconKind::Sparkles class="w-5 h-5"/>
                        </div>
                        <p class="text-sm font-medium text-gray-800 dark:text-gray-100">{move || t_string!(i18n, studio.chat_empty_title)}</p>
                        <p class="text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || t_string!(i18n, studio.chat_empty_hint)}</p>
                    </div>
                </Show>
                <For each=move || messages.get() key=|m| m.id.clone() let:m>
                    <MessageView msg=m last_spec_id=last_spec_id current_version=current_version busy=busy has_model=Signal::derive(has_model) actions=actions/>
                </For>
                <Show when=move || busy.get()>
                    <div class="flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
                        <span class="inline-flex gap-1">
                            <span class="w-1.5 h-1.5 rounded-full bg-brand animate-bounce"></span>
                            <span class="w-1.5 h-1.5 rounded-full bg-brand animate-bounce [animation-delay:120ms]"></span>
                            <span class="w-1.5 h-1.5 rounded-full bg-brand animate-bounce [animation-delay:240ms]"></span>
                        </span>
                        {move || progress_text.get()}
                    </div>
                </Show>
            </div>

            // ---- 输入区 ----
            <div class="shrink-0 border-t border-gray-200 dark:border-gray-700 p-3 space-y-2">
                <Show when=move || has_model() && text.with(|t| t.is_empty()) && !busy.get()>
                    <div class="flex flex-wrap gap-1.5">
                        {move || suggestions().into_iter().map(|s| {
                            let s = s.to_string();
                            let shown = s.clone();
                            view! {
                                <button
                                    type="button"
                                    class="px-2 py-1 rounded-full border border-gray-200 dark:border-gray-600 text-[11px] text-gray-600 dark:text-gray-300 \
                                           hover:border-brand hover:text-brand transition-colors"
                                    on:click=move |_| text.set(s.clone())
                                >{shown}</button>
                            }
                        }).collect_view()}
                    </div>
                </Show>
                <Show when=move || !pending_images.with(Vec::is_empty)>
                    <div class="flex flex-wrap gap-2">
                        <For each=move || pending_images.get() key=|a| a.id.clone() let:asset>
                            <PendingImage asset=asset pending=pending_images/>
                        </For>
                    </div>
                </Show>
                <textarea
                    rows=3
                    class="w-full px-3 py-2 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 text-sm leading-relaxed \
                           text-gray-900 dark:text-gray-100 placeholder:text-gray-400 resize-none focus:outline-none focus:ring-2 focus:ring-brand/40 focus:border-brand"
                    placeholder=move || if has_model() {
                        t_string!(i18n, studio.composer_edit_placeholder)
                    } else {
                        t_string!(i18n, studio.composer_new_placeholder)
                    }
                    prop:value=move || text.get()
                    on:input=move |e| text.set(event_target_value(&e))
                    on:keydown=move |e| {
                        // 回车发送,Shift + 回车换行;输入法组合中的回车是「确认候选词」
                        if e.key() == "Enter" && !e.shift_key() && !e.is_composing() {
                            e.prevent_default();
                            if can_send.get_untracked() {
                                actions.send.run(());
                            }
                        }
                    }
                ></textarea>
                <div class="flex items-center gap-2 whitespace-nowrap">
                    <IconButton icon=IconKind::Image label=move || t_string!(i18n, studio.attach_image) disabled=Signal::derive(move || busy.get()) on_click=move || actions.attach.run(())/>
                    <Show when=has_model>
                        <label class="flex items-center gap-1.5 text-[11px] text-gray-500 dark:text-gray-400" title=move || t_string!(i18n, studio.pick_hint)>
                            <Toggle checked=pick_mode on_change=move |v: bool| pick_mode.set(v)/>
                            <Icon kind=IconKind::Crosshair class="w-3.5 h-3.5"/>
                            {move || match picked.get() {
                                Some(p) => format!("{}, {}, {}", fmt_mm(p.point[0]), fmt_mm(p.point[1]), fmt_mm(p.point[2])),
                                None => t_string!(i18n, cad.pick).to_string(),
                            }}
                        </label>
                    </Show>
                </div>
                <div class="flex items-center justify-between gap-2 whitespace-nowrap">
                    <Segmented
                        value=Signal::derive(move || tier.get())
                        options=vec![
                            (CadTier::Fast, label(|l| leptos_i18n::td_string!(l, studio.tier_fast))),
                            (CadTier::Balanced, label(|l| leptos_i18n::td_string!(l, studio.tier_balanced))),
                            (CadTier::Precise, label(|l| leptos_i18n::td_string!(l, studio.tier_precise))),
                        ]
                        on_change=move |t: CadTier| tier.set(t)
                    />
                    <Button small=true icon=IconKind::Play disabled=Signal::derive(move || !can_send.get()) on_click=move || actions.send.run(())>
                        {move || t_string!(i18n, studio.send)}
                    </Button>
                </div>
            </div>
        </div>
    }
}

#[component]
fn PendingImage(asset: Asset, pending: RwSignal<Vec<Asset>>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(asset.id.clone());
    view! {
        <div class="group relative w-12 h-12 rounded-lg overflow-hidden border border-gray-200 dark:border-gray-700">
            <img src=ipc::asset_url(&asset.id) class="w-full h-full object-cover" draggable="false"/>
            <div class="absolute top-0 right-0 opacity-0 group-hover:opacity-100 transition-opacity rounded bg-white/90 dark:bg-gray-800/90">
                <IconButton
                    icon=IconKind::Close
                    label=move || t_string!(i18n, common.delete)
                    on_click=move || pending.update(|list| list.retain(|a| id.with_value(|id| &a.id != id)))
                />
            </div>
        </div>
    }
}

#[component]
fn MessageView(
    msg: CadMessage,
    last_spec_id: Memo<Option<String>>,
    current_version: Memo<Option<String>>,
    busy: RwSignal<bool>,
    has_model: Signal<bool>,
    actions: ChatActions,
) -> impl IntoView {
    let i18n = use_i18n();
    let idle = Signal::derive(move || !busy.get());
    let usage = msg.extra.report.clone().map(|r| {
        view! {
            <div class="text-[10px] tabular-nums text-gray-400">
                {move || t_string!(
                    i18n,
                    studio.msg_usage,
                    yuan = format!("{:.3}", r.cost_fen / 100.0),
                    secs = format!("{:.1}", r.elapsed_ms as f64 / 1000.0),
                ).to_string()}
            </div>
        }
    });

    match (msg.role, msg.kind) {
        // ---- 用户 ----
        (MsgRole::User, _) => {
            let pick = msg.extra.pick;
            view! {
                <div class="flex flex-col items-end gap-1">
                    {(!msg.image_asset_ids.is_empty()).then(|| view! {
                        <div class="flex flex-wrap justify-end gap-1.5">
                            {msg.image_asset_ids.iter().map(|id| view! {
                                <img src=ipc::asset_url(id) class="w-16 h-16 object-cover rounded-lg border border-gray-200 dark:border-gray-700" draggable="false"/>
                            }).collect_view()}
                        </div>
                    })}
                    {(!msg.content.is_empty()).then(|| view! {
                        <div class="max-w-[92%] px-3 py-2 rounded-2xl rounded-br-md bg-brand text-white text-sm leading-relaxed whitespace-pre-wrap selectable">
                            {msg.content.clone()}
                        </div>
                    })}
                    {pick.map(|p| view! {
                        <div class="flex items-center gap-1 text-[10px] text-gray-400 tabular-nums">
                            <Icon kind=IconKind::Crosshair class="w-3 h-3"/>
                            {format!("{}, {}, {}", fmt_mm(p.point[0]), fmt_mm(p.point[1]), fmt_mm(p.point[2]))}
                        </div>
                    })}
                </div>
            }
            .into_any()
        }

        // ---- 事件:改参数 / 手改代码 ----
        (MsgRole::Event, kind) => {
            let version_id = msg.version_id.clone();
            let note = msg.content.clone();
            view! {
                <button
                    type="button"
                    class="mx-auto flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[11px] text-gray-500 dark:text-gray-400 hover:text-brand transition-colors"
                    on:click=move |_| {
                        if let Some(v) = version_id.clone() {
                            actions.select_version.run((v,));
                        }
                    }
                >
                    <Icon kind=if kind == MsgKind::Param { IconKind::Ruler } else { IconKind::Code } class="w-3 h-3"/>
                    {move || if kind == MsgKind::Param {
                        format!("{} · {note}", t_string!(i18n, cad.src_param))
                    } else {
                        t_string!(i18n, studio.event_manual).to_string()
                    }}
                </button>
            }
            .into_any()
        }

        // ---- AI:规格卡 ----
        (MsgRole::Assistant, MsgKind::Spec) => {
            let Some(spec) = msg.extra.spec.clone() else {
                return view! { <AssistantBubble text=msg.content.clone()/> }.into_any();
            };
            let id = msg.id.clone();
            let is_latest = move || last_spec_id.with(|l| l.as_deref() == Some(id.as_str()));
            let for_generate = StoredValue::new(spec.clone());
            let unsuitable = (!spec.suitable).then(|| spec.unsuitable_reason.clone());
            view! {
                <div class="rounded-xl border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 p-3 space-y-2">
                    <div class="flex items-center justify-between gap-2">
                        <div class="flex items-center gap-1.5 min-w-0">
                            <Badge tone=Tone::Brand>{move || t_string!(i18n, cad.spec_title)}</Badge>
                            <span class="text-sm font-semibold truncate text-gray-900 dark:text-gray-50 selectable">{spec.name.clone()}</span>
                        </div>
                        <span class="shrink-0 text-[11px] tabular-nums text-gray-500 dark:text-gray-400">{size_text(spec.overall_mm)}</span>
                    </div>
                    {(!spec.summary.is_empty()).then(|| view! {
                        <p class="text-xs leading-relaxed text-gray-600 dark:text-gray-300 selectable">{spec.summary.clone()}</p>
                    })}
                    {unsuitable.map(|reason| view! {
                        <p class="rounded-lg bg-amber-50 dark:bg-amber-500/10 px-2 py-1.5 text-xs leading-relaxed text-amber-800 dark:text-amber-200">
                            {move || t_string!(i18n, cad.spec_unsuitable, reason = reason.clone()).to_string()}
                        </p>
                    })}
                    <ul class="space-y-1">
                        {spec.features.iter().map(|f| {
                            let dims = dims_to_text(&f.dimensions).replace('\n', " · ");
                            view! {
                                <li class="text-xs leading-relaxed">
                                    <span class="font-mono text-[11px] text-brand">{f.name.clone()}</span>
                                    <span class="text-gray-700 dark:text-gray-200 selectable">{format!(" {}", f.description)}</span>
                                    {(!dims.is_empty()).then(|| view! { <div class="text-[11px] tabular-nums text-gray-400 selectable">{dims}</div> })}
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                    {(!spec.assumptions.is_empty()).then(|| view! {
                        <div class="text-[11px] leading-relaxed text-amber-700 dark:text-amber-300 selectable">
                            {move || t_string!(i18n, cad.spec_assumptions)}": "{spec.assumptions.join(";")}
                        </div>
                    })}
                    <Show when=is_latest>
                        <p class="text-[11px] leading-relaxed text-gray-400">{move || t_string!(i18n, studio.spec_hint)}</p>
                        <div class="flex flex-wrap gap-2">
                            <Button small=true icon=IconKind::Sparkles disabled=Signal::derive(move || !idle.get()) on_click=move || actions.generate.run((for_generate.get_value(),))>
                                {move || if has_model.get() { t_string!(i18n, studio.regenerate) } else { t_string!(i18n, studio.generate) }}
                            </Button>
                            <Button small=true variant=ButtonVariant::Secondary disabled=Signal::derive(move || !idle.get()) on_click=move || actions.edit_spec.run((for_generate.get_value(),))>
                                {move || t_string!(i18n, studio.edit_spec)}
                            </Button>
                        </div>
                    </Show>
                    {usage}
                </div>
            }
            .into_any()
        }

        // ---- AI:改好了 ----
        (MsgRole::Assistant, MsgKind::Build) => {
            let version_id = StoredValue::new(msg.version_id.clone().unwrap_or_default());
            let base = StoredValue::new(msg.extra.base_version_id.clone());
            let report = msg.extra.report.clone().unwrap_or_default();
            let changed = changed_text(&report.changed_sections);
            let warned = !report.warnings.is_empty();
            let repaired = report.rounds.len();
            let is_current = move || current_version.with(|c| version_id.with_value(|v| c.as_deref() == Some(v.as_str())));
            let text = msg.content.clone();
            view! {
                <div class="space-y-1.5">
                    <AssistantBubble text=if text.trim().is_empty() { td_string!(current_locale(), studio.built_default).to_string() } else { text }/>
                    <div class="flex flex-wrap items-center gap-1.5 pl-1">
                        <button
                            type="button"
                            class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full border text-[11px] transition-colors"
                            class=("border-brand", is_current.clone())
                            class=("text-brand", is_current.clone())
                            class=("border-gray-200", { let c = is_current.clone(); move || !c() })
                            class=("dark:border-gray-600", { let c = is_current.clone(); move || !c() })
                            class=("text-gray-500", { let c = is_current.clone(); move || !c() })
                            on:click=move |_| actions.select_version.run((version_id.get_value(),))
                        >
                            <Icon kind=IconKind::Box class="w-3 h-3"/>
                            {move || t_string!(i18n, studio.view_version)}
                        </button>
                        {(!changed.is_empty()).then(|| view! { <span class="text-[11px] text-gray-500 dark:text-gray-400 selectable">{changed.clone()}</span> })}
                        {(repaired > 0).then(|| view! {
                            <Badge tone=Tone::Neutral>{move || t_string!(i18n, studio.auto_repaired, n = repaired).to_string()}</Badge>
                        })}
                        {warned.then(|| view! { <Badge tone=Tone::Amber>{move || t_string!(i18n, cad.warnings)}</Badge> })}
                        <Show when={ let c = is_current.clone(); move || c() && base.with_value(Option::is_some) }>
                            <button
                                type="button"
                                class="text-[11px] text-gray-400 hover:text-red-500 transition-colors disabled:opacity-40"
                                disabled=move || busy.get()
                                on:click=move |_| {
                                    if let Some(b) = base.get_value() {
                                        actions.select_version.run((b,));
                                    }
                                }
                            >{move || t_string!(i18n, studio.undo)}</button>
                        </Show>
                    </div>
                    {warned.then(|| view! { <ReportCard report=report.clone()/> })}
                    <div class="pl-1">{usage}</div>
                </div>
            }
            .into_any()
        }

        // ---- AI:没改成 ----
        (MsgRole::Assistant, MsgKind::Failed) => {
            let report = msg.extra.report.clone().unwrap_or_default();
            let code = StoredValue::new(msg.extra.code.clone().unwrap_or_default());
            let text = msg.content.clone();
            view! {
                <div class="space-y-1.5">
                    {(!text.trim().is_empty()).then(|| view! { <AssistantBubble text=text.clone()/> })}
                    <ReportCard report=report failed=true/>
                    <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Code on_click=move || actions.load_code.run((code.get_value(),))>
                        {move || t_string!(i18n, studio.load_failed_code)}
                    </Button>
                </div>
            }
            .into_any()
        }

        // ---- AI:复核结论 ----
        (MsgRole::Assistant, MsgKind::Review) => {
            let review = msg.extra.review.clone().unwrap_or_default();
            let n = review.differences.len();
            if review.matches && n == 0 {
                return view! {
                    <div class="space-y-1">
                        <AssistantBubble text=td_string!(current_locale(), cad.review_match).to_string()/>
                        <div class="pl-1">{usage}</div>
                    </div>
                }
                .into_any();
            }
            view! {
                <div class="rounded-xl border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 p-3 space-y-2">
                    <div class="flex items-center gap-1.5">
                        <Badge tone=Tone::Amber>{move || t_string!(i18n, cad.review_title)}</Badge>
                        <span class="text-xs text-gray-600 dark:text-gray-300">{move || t_string!(i18n, cad.review_diff, n = n).to_string()}</span>
                    </div>
                    {review.differences.into_iter().map(|d| {
                        let say = StoredValue::new(d.clone());
                        view! {
                            <div class="flex items-start gap-2">
                                <p class="flex-1 text-xs leading-relaxed text-gray-700 dark:text-gray-200 selectable">{d}</p>
                                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Wand disabled=Signal::derive(move || !idle.get()) on_click=move || actions.say.run((say.get_value(),))>
                                    {move || t_string!(i18n, cad.review_fix)}
                                </Button>
                            </div>
                        }
                    }).collect_view()}
                    {usage}
                </div>
            }
            .into_any()
        }

        // ---- AI:回答 / 反问 ----
        (MsgRole::Assistant, _) => view! {
            <div class="space-y-1">
                <AssistantBubble text=msg.content.clone()/>
                <div class="pl-1">{usage}</div>
            </div>
        }
        .into_any(),
    }
}

#[component]
fn AssistantBubble(text: String) -> impl IntoView {
    view! {
        <div class="max-w-[95%] px-3 py-2 rounded-2xl rounded-bl-md bg-gray-100 dark:bg-gray-700/70 text-sm leading-relaxed whitespace-pre-wrap \
                    text-gray-900 dark:text-gray-100 selectable">
            {text}
        </div>
    }
}
