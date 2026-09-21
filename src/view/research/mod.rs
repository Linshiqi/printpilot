//! 调研(一级入口,侧栏「调研」):三个工作台里最靠前的一个。
//!
//!   左:调研输入(主题 / 价位 / 你的一手观察)+ 关联到哪个项目 + 历史
//!   右:报告(结论、机会卡、证据卡);机会卡可以「采用为项目」
//!
//! 可以独立用(灵感池:先调研,看到好机会再采用为项目),也可以从项目里发起(主题已经填好,报告挂在项目名下)。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::provider::ProviderId;
use pp_common::research::{Evidence, EvidenceGrade, IpRisk, ModelRoute, Opportunity, ResearchBrief, ResearchRunSummary, SavedResearch};
use pp_common::Project;

use crate::i18n_util::current_locale;
use crate::i18n::{use_i18n, Locale};
use crate::icon::IconKind;
use crate::ipc::{self, cmd};
use crate::state::{AppState, Handoff};
use crate::ui::{Badge, Button, ButtonVariant, Card, EmptyState, Field, IconButton, SectionTitle, TextArea, TextInput, Tone};
use crate::utils::{fmt_int, format_ts, local_tz_offset_minutes};

#[component]
pub fn ResearchView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let topic = RwSignal::new(String::new());
    let price_min = RwSignal::new(String::new());
    let price_max = RwSignal::new(String::new());
    let notes = RwSignal::new(String::new());
    let running = RwSignal::new(false);
    let result = RwSignal::new(None::<SavedResearch>);
    // 这次调研挂在哪个项目名下(空 = 不挂,纯灵感池)
    let project_id = RwSignal::new(None::<String>);
    let history = RwSignal::new(Vec::<ResearchRunSummary>::new());
    let tz = local_tz_offset_minutes();

    let reload_history = move || {
        spawn_local(async move {
            match ipc::call_no_args::<Vec<ResearchRunSummary>>(cmd::LIST_RESEARCH_RUNS).await {
                Ok(list) => history.set(list),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let open_run = move |run_id: String| {
        spawn_local(async move {
            match ipc::call::<_, SavedResearch>(cmd::GET_RESEARCH, &serde_json::json!({ "run_id": run_id })).await {
                Ok(saved) => result.set(Some(saved)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    reload_history();
    // 从项目里过来的:要么是「为这个项目做一次调研」(主题已填好),要么是打开一份已有的报告
    match state.handoff.get_untracked() {
        Some(Handoff::Research { project_id: pid, topic: t }) => {
            state.handoff.set(None);
            project_id.set(Some(pid));
            topic.set(t);
        }
        Some(Handoff::ResearchRun { run_id }) => {
            state.handoff.set(None);
            open_run(run_id);
        }
        _ => {}
    }

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
            let args = serde_json::json!({ "brief": brief, "project_id": project_id.get_untracked() });
            match ipc::call::<_, SavedResearch>(cmd::RESEARCH_RUN, &args).await {
                Ok(saved) => {
                    result.set(Some(saved));
                    reload_history();
                    // 挂在项目名下的调研会让那个项目的「调研」清单完成
                    state.reload_project_facts();
                }
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
                    state.reload_project_facts();
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
        <div class="h-full flex flex-col">
        <header class="shrink-0 px-6 h-14 flex items-center border-b border-gray-200 dark:border-gray-700">
            <div class="min-w-0">
                <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, research.title)}</h1>
                <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, research.subtitle)}</p>
            </div>
        </header>
        <div class="flex-1 min-h-0 flex">
            // ---- 左:调研输入 ----
            <aside class="w-80 shrink-0 h-full overflow-y-auto border-r border-gray-200 dark:border-gray-700 p-4 space-y-4">
                // 挂在哪个项目名下:从项目里发起时已经选好;不选 = 先调研,看到好机会再「采用为项目」
                <Field label=move || t_string!(i18n, research.for_project)>
                    <div class="flex items-center gap-1.5">
                    <select
                        class="w-full h-9 px-2 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-gray-100"
                        on:change=move |e| {
                            let v = event_target_value(&e);
                            project_id.set((!v.is_empty()).then_some(v));
                        }
                    >
                        <option value="" selected=move || project_id.with(Option::is_none)>{move || t_string!(i18n, research.no_project)}</option>
                        {move || state.projects.get().into_iter().map(|p| {
                            let pid = p.id.clone();
                            view! {
                                <option value=p.id.clone() selected=move || project_id.with(|cur| cur.as_deref() == Some(pid.as_str()))>
                                    {format!("{} {}", p.code, p.title)}
                                </option>
                            }
                        }).collect_view()}
                    </select>
                    // 选了项目:一键打开项目中枢(抽屉盖在当前页面上)
                    <Show when=move || project_id.with(Option::is_some)>
                        <IconButton
                            icon=IconKind::Kanban
                            label=move || t_string!(i18n, project.open_hub)
                            on_click=move || state.open_project.set(project_id.get_untracked())
                        />
                    </Show>
                    </div>
                </Field>
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
                <div class="flex items-center gap-2">
                    <Button icon=IconKind::Sparkles disabled=Signal::derive(move || !can_run.get()) on_click=run>
                        {move || if running.get() { progress_text() } else { t_string!(i18n, lab.r_run).to_string() }}
                    </Button>
                    // 一次调研要好几次模型调用 + 检索:可以中途停下,这一次什么都不入库
                    <Show when=move || running.get()>
                        <Button variant=ButtonVariant::Secondary icon=IconKind::Ban on_click=move || state.cancel_turn("research".to_string())>
                            {move || t_string!(i18n, studio.stop)}
                        </Button>
                    </Show>
                </div>

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

                // ---- 历史:做过的调研都在,点开就是当时的报告 ----
                <Show when=move || !history.with(Vec::is_empty)>
                    <div class="pt-3 border-t border-gray-200 dark:border-gray-700 space-y-1.5">
                        <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, research.history)}</div>
                        <For each=move || history.get() key=|r| r.id.clone() let:row>
                            {
                                let run_id = StoredValue::new(row.id.clone());
                                let active = move || result.with(|r| r.as_ref().is_some_and(|r| run_id.with_value(|id| &r.run_id == id)));
                                view! {
                                    <button
                                        type="button"
                                        class="w-full px-2.5 py-1.5 rounded-lg text-left transition-colors"
                                        class=("bg-brand-soft", active)
                                        class=("dark:bg-indigo-500/15", active)
                                        class=("hover:bg-gray-100", move || !active())
                                        class=("dark:hover:bg-gray-700/60", move || !active())
                                        on:click=move |_| open_run(run_id.get_value())
                                    >
                                        <div class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{row.topic.clone()}</div>
                                        <div class="text-[11px] tabular-nums text-gray-400">
                                            {move || t_string!(i18n, project.opportunities, n = row.opportunities).to_string()}" · "{format_ts(row.created_at, tz)}
                                        </div>
                                    </button>
                                }
                            }
                        </For>
                    </div>
                </Show>
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
