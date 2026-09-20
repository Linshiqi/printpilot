//! 预研页 · 市场调研面板:验证「供应商适配器 + 调研流水线」(预研 ③)。
//! MVP-α 第 3 周会把它升级成正式的调研页(历史、追问、截图证据);报告区的组件到时直接复用。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::provider::ProviderId;
use pp_common::research::{Evidence, EvidenceGrade, IpRisk, ModelRoute, Opportunity, ResearchBrief, SavedResearch};
use pp_common::Project;

use crate::i18n_util::current_locale;
use crate::i18n::{use_i18n, Locale};
use crate::icon::IconKind;
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::ui::{Badge, Button, Card, EmptyState, Field, SectionTitle, TextArea, TextInput, Tone};
use crate::utils::fmt_int;

#[component]
pub fn ResearchPanel(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let topic = RwSignal::new(String::new());
    let price_min = RwSignal::new(String::new());
    let price_max = RwSignal::new(String::new());
    let notes = RwSignal::new(String::new());
    let running = RwSignal::new(false);
    let result = RwSignal::new(None::<SavedResearch>);

    let has_key = move |id: ProviderId| state.providers.with(|list| list.iter().any(|p| p.id == id && p.has_key));
    let demo = move || state.app_info.with(|i| i.as_ref().is_some_and(|i| i.demo_mode));
    let can_run = Signal::derive(move || !running.get() && !topic.with(|t| t.trim().is_empty()));

    let run = move || {
        if !can_run.get_untracked() {
            return;
        }
        let price = |s: RwSignal<String>| s.with_untracked(|v| v.trim().parse::<u32>().ok());
        let brief = ResearchBrief {
            topic: topic.get_untracked().trim().to_string(),
            price_min: price(price_min),
            price_max: price(price_max),
            user_notes: notes
                .with_untracked(|n| n.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()),
            // 成型空间、材料、渠道先用出厂默认;打印机档案做出来之后从那里取
            ..Default::default()
        };
        running.set(true);
        state.research_progress.set(None);
        spawn_local(async move {
            match ipc::call::<_, SavedResearch>(cmd::RESEARCH_RUN, &serde_json::json!({ "brief": brief })).await {
                Ok(saved) => result.set(Some(saved)),
                Err(e) => state.notify_error(e),
            }
            running.set(false);
            state.research_progress.set(None);
        });
    };

    let progress_text = move || {
        let Some((step, done, total)) = state.research_progress.get() else {
            return t_string!(i18n, lab.r_running).to_string();
        };
        match step.as_str() {
            "planning" => t_string!(i18n, lab.r_step_planning).to_string(),
            "searching" => t_string!(i18n, lab.r_step_searching, done = done + 1, total = total).to_string(),
            "scoring" => t_string!(i18n, lab.r_step_scoring).to_string(),
            "reviewing" => t_string!(i18n, lab.r_step_reviewing).to_string(),
            _ => t_string!(i18n, lab.r_running).to_string(),
        }
    };

    let adopt = move |index: usize| {
        let Some(run_id) = result.with_untracked(|r| r.as_ref().map(|r| r.run_id.clone())) else {
            return;
        };
        spawn_local(async move {
            let args = serde_json::json!({ "run_id": run_id, "index": index });
            match ipc::call::<_, Project>(cmd::ADOPT_OPPORTUNITY, &args).await {
                Ok(p) => {
                    state.notify_info(td_string!(current_locale(), lab.r_adopted, code = &p.code).to_string());
                    state.projects.update(|list| list.insert(0, p));
                }
                Err(e) => state.notify_error(e),
            }
        });
    };

    let key_badge = move |id: ProviderId| {
        move || {
            let (tone, text) = if has_key(id) {
                (Tone::Green, t_string!(i18n, settings.key_configured))
            } else {
                (Tone::Amber, t_string!(i18n, settings.key_missing))
            };
            view! { <Badge tone=tone>{text}</Badge> }
        }
    };

    view! {
        <div class="h-full flex">
            // ---- 左:调研输入 ----
            <aside class="w-80 shrink-0 h-full overflow-y-auto border-r border-gray-200 dark:border-gray-700 p-4 space-y-4">
                <Field label=move || t_string!(i18n, lab.r_topic)>
                    <TextInput value=topic autofocus=true placeholder=move || t_string!(i18n, lab.r_topic_placeholder) on_enter=run/>
                </Field>
                <Field label=move || t_string!(i18n, lab.r_price)>
                    <div class="flex items-center gap-2">
                        <TextInput value=price_min numeric=true placeholder=move || t_string!(i18n, lab.r_price_min)/>
                        <span class="text-gray-400">"~"</span>
                        <TextInput value=price_max numeric=true placeholder=move || t_string!(i18n, lab.r_price_max)/>
                    </div>
                </Field>
                <Field label=move || t_string!(i18n, lab.r_notes)>
                    <TextArea value=notes rows=6 placeholder=move || t_string!(i18n, lab.r_notes_placeholder)/>
                </Field>
                <Button icon=IconKind::Sparkles disabled=Signal::derive(move || !can_run.get()) on_click=run>
                    {move || if running.get() { progress_text() } else { t_string!(i18n, lab.r_run).to_string() }}
                </Button>

                <div class="pt-3 border-t border-gray-200 dark:border-gray-700 space-y-2 text-xs text-gray-500 dark:text-gray-400">
                    <Show
                        when=demo
                        fallback=move || view! {
                            <div class="flex items-center justify-between">
                                <span>{move || t_string!(i18n, lab.r_status_llm)}" · DeepSeek"</span>
                                {key_badge(ProviderId::Deepseek)}
                            </div>
                            <div class="flex items-center justify-between">
                                <span>{move || t_string!(i18n, lab.r_status_search)}" · 智谱"</span>
                                {key_badge(ProviderId::ZhipuSearch)}
                            </div>
                        }
                    >
                        <p class="leading-relaxed text-amber-600 dark:text-amber-400">{move || t_string!(i18n, lab.r_status_demo)}</p>
                    </Show>
                </div>
            </aside>

            // ---- 右:报告 ----
            <section class="flex-1 min-w-0 h-full overflow-y-auto">
                {move || match result.get() {
                    None => view! {
                        <EmptyState
                            icon=IconKind::Lightbulb
                            title=move || t_string!(i18n, lab.r_empty_title)
                            hint=move || t_string!(i18n, lab.r_empty_hint)
                        />
                    }.into_any(),
                    Some(saved) => view! { <Report saved=saved on_adopt=adopt/> }.into_any(),
                }}
            </section>
        </div>
    }
}

#[component]
fn Report(saved: SavedResearch, #[prop(into)] on_adopt: Callback<(usize,)>) -> impl IntoView {
    let i18n = use_i18n();
    let report = saved.report;
    let u = report.usage;
    let opportunities = report.opportunities.clone();
    let evidence = report.evidence.clone();

    view! {
        <div class="max-w-4xl mx-auto p-6 space-y-5">
            {report.degraded.then(|| view! {
                <Banner tone=Tone::Amber>{move || t_string!(i18n, lab.r_banner_degraded)}</Banner>
            })}
            {(!report.reviewed && !report.degraded).then(|| view! {
                <Banner tone=Tone::Neutral>{move || t_string!(i18n, lab.r_banner_unreviewed)}</Banner>
            })}

            <p class="text-xs tabular-nums text-gray-500 dark:text-gray-400 selectable">
                {move || t_string!(
                    i18n,
                    lab.r_usage,
                    calls = u.llm_calls,
                    searches = u.searches,
                    tokens = fmt_int((u.tokens_in + u.tokens_out) as u32),
                    yuan = format!("{:.3}", u.cost_fen / 100.0),
                    secs = format!("{:.1}", u.elapsed_ms as f64 / 1000.0),
                    retries = u.json_retries,
                ).to_string()}
            </p>

            <Card class="p-5 space-y-2">
                <SectionTitle title=move || t_string!(i18n, lab.r_summary)/>
                <p class="text-sm leading-relaxed whitespace-pre-wrap text-gray-800 dark:text-gray-100 selectable">{report.summary_md.clone()}</p>
            </Card>

            <section class="space-y-3">
                <SectionTitle title=move || t_string!(i18n, lab.r_opportunities)/>
                <div class="grid grid-cols-1 xl:grid-cols-2 gap-3">
                    {opportunities
                        .into_iter()
                        .enumerate()
                        .map(|(i, o)| view! { <OpportunityCard index=i opp=o on_adopt=on_adopt/> })
                        .collect_view()}
                </div>
            </section>

            <section class="space-y-3">
                <SectionTitle title=move || t_string!(i18n, lab.r_evidence)/>
                <ol class="space-y-2">
                    {evidence.into_iter().map(|e| view! { <EvidenceRow e=e/> }).collect_view()}
                </ol>
            </section>
        </div>
    }
}

#[component]
fn Banner(tone: Tone, children: Children) -> impl IntoView {
    let color = match tone {
        Tone::Amber => "border-amber-300 bg-amber-50 text-amber-800 dark:bg-amber-500/10 dark:border-amber-500/40 dark:text-amber-200",
        _ => "border-gray-300 bg-gray-50 text-gray-600 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-300",
    };
    view! { <div class=format!("rounded-lg border px-4 py-3 text-xs leading-relaxed {color}")>{children()}</div> }
}

fn score_label(l: Locale, i: usize) -> &'static str {
    match i {
        0 => td_string!(l, lab.r_score_demand),
        1 => td_string!(l, lab.r_score_differentiation),
        2 => td_string!(l, lab.r_score_competition),
        3 => td_string!(l, lab.r_score_printability),
        4 => td_string!(l, lab.r_score_margin),
        _ => td_string!(l, lab.r_score_logistics),
    }
}

#[component]
fn OpportunityCard(index: usize, opp: Opportunity, on_adopt: Callback<(usize,)>) -> impl IntoView {
    let i18n = use_i18n();
    let vetoed = opp.vetoed();
    let scores = opp.scores.as_array();
    let (ip, route) = (opp.ip_risk, opp.route);
    let cited = opp.evidence_ids.iter().map(|id| format!("[{id}]")).collect::<Vec<_>>().join(" ");
    let (size_mm, grams) = (opp.size_mm, opp.grams);

    view! {
        <article
            class="rounded-xl border bg-white dark:bg-gray-800 p-4 space-y-3"
            class=("border-red-300", vetoed)
            class=("dark:border-red-500/40", vetoed)
            class=("opacity-80", vetoed)
            class=("border-gray-200", !vetoed)
            class=("dark:border-gray-700", !vetoed)
        >
            <header class="flex items-start justify-between gap-3">
                <div class="min-w-0 space-y-1">
                    <h3 class="text-sm font-semibold text-gray-900 dark:text-gray-50">{opp.title.clone()}</h3>
                    <p class="text-xs text-gray-600 dark:text-gray-300">{opp.pitch.clone()}</p>
                </div>
                <div class="shrink-0 text-right">
                    <div class="text-2xl font-semibold tabular-nums leading-none text-brand dark:text-indigo-300">{format!("{:.1}", opp.total)}</div>
                    <div class="text-[10px] text-gray-400">{move || t_string!(i18n, lab.r_total)}</div>
                </div>
            </header>

            <div class="flex flex-wrap gap-1.5">
                {move || {
                    let (tone, text) = match ip {
                        IpRisk::Low => (Tone::Green, t_string!(i18n, lab.r_ip_low)),
                        IpRisk::Medium => (Tone::Amber, t_string!(i18n, lab.r_ip_medium)),
                        IpRisk::High => (Tone::Red, t_string!(i18n, lab.r_ip_high)),
                    };
                    view! { <Badge tone=tone>{text}</Badge> }
                }}
                {move || {
                    let text = match route {
                        ModelRoute::AiGenerated => t_string!(i18n, lab.r_route_ai),
                        ModelRoute::Parametric => t_string!(i18n, lab.r_route_parametric),
                        ModelRoute::Import => t_string!(i18n, lab.r_route_import),
                    };
                    view! { <Badge tone=Tone::Neutral>{text}</Badge> }
                }}
                <Badge tone=Tone::Brand>{format!("¥{}~{}", opp.price_low, opp.price_high)}</Badge>
                {(size_mm > 0.0 || grams > 0.0).then(|| view! {
                    <Badge tone=Tone::Neutral>
                        {move || t_string!(i18n, lab.r_size_hint, mm = format!("{size_mm:.0}"), grams = format!("{grams:.0}")).to_string()}
                    </Badge>
                })}
            </div>

            // 六个维度的小条形图:直接输出 SVG,不引图表库
            <div class="grid grid-cols-3 gap-x-3 gap-y-1.5">
                {scores
                    .into_iter()
                    .enumerate()
                    .map(|(i, s)| view! {
                        <div class="space-y-0.5">
                            <div class="flex justify-between text-[10px] text-gray-500 dark:text-gray-400">
                                <span>{move || score_label(i18n.get_locale(), i)}</span>
                                <span class="tabular-nums">{format!("{s:.1}")}</span>
                            </div>
                            <svg viewBox="0 0 100 4" class="w-full h-1" preserveAspectRatio="none">
                                <rect width="100" height="4" rx="2" class="fill-gray-200 dark:fill-gray-700"/>
                                <rect width=format!("{:.1}", s.clamp(0.0, 5.0) * 20.0) height="4" rx="2" class="fill-brand dark:fill-indigo-400"/>
                            </svg>
                        </div>
                    })
                    .collect_view()}
            </div>

            <dl class="space-y-1.5 text-xs leading-relaxed selectable">
                <div><dt class="inline text-gray-400">{move || t_string!(i18n, lab.r_persona)}":"</dt>" "<dd class="inline text-gray-700 dark:text-gray-200">{opp.persona.clone()}</dd></div>
                <div class="text-gray-700 dark:text-gray-200">{opp.selling_points.join(" · ")}</div>
                <div>
                    <dt class="inline text-gray-400">{move || t_string!(i18n, lab.r_rationale)}":"</dt>" "
                    <dd class="inline text-gray-700 dark:text-gray-200">{opp.rationale.clone()}" "<span class="text-gray-400">{cited}</span></dd>
                </div>
                {(!opp.risks.is_empty()).then(|| view! {
                    <div>
                        <dt class="inline text-gray-400">{move || t_string!(i18n, lab.r_risks)}":"</dt>" "
                        <dd class="inline text-amber-700 dark:text-amber-300">{opp.risks.join(";")}</dd>
                    </div>
                })}
            </dl>

            <div class="flex justify-end">
                <Button small=true icon=IconKind::Plus disabled=vetoed on_click=move || on_adopt.run((index,))>
                    {move || t_string!(i18n, lab.r_adopt)}
                </Button>
            </div>
        </article>
    }
}

#[component]
fn EvidenceRow(e: Evidence) -> impl IntoView {
    let i18n = use_i18n();
    let grade = e.grade;
    let source = [e.site.clone(), e.published.clone().unwrap_or_default()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    view! {
        <li class="flex gap-3 rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 p-3 text-xs">
            <span class="w-7 shrink-0 font-mono text-gray-400">{format!("[{}]", e.id)}</span>
            <div class="min-w-0 space-y-1 selectable">
                <div class="flex items-center gap-2 flex-wrap">
                    {move || {
                        let (tone, text) = match grade {
                            EvidenceGrade::A => (Tone::Green, format!("A · {}", t_string!(i18n, lab.r_evidence_user))),
                            EvidenceGrade::B => (Tone::Neutral, "B".to_string()),
                            EvidenceGrade::C => (Tone::Amber, "C".to_string()),
                        };
                        view! { <Badge tone=tone>{text}</Badge> }
                    }}
                    <span class="font-medium text-gray-800 dark:text-gray-100 break-words">{e.title.clone()}</span>
                    <span class="text-gray-400">{source}</span>
                </div>
                <p class="leading-relaxed text-gray-600 dark:text-gray-300 break-words">{e.excerpt.clone()}</p>
                {(!e.url.is_empty()).then(|| view! { <p class="text-[11px] text-gray-400 break-all">{e.url.clone()}</p> })}
            </div>
        </li>
    }
}
