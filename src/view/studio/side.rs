//! 「代码建模」右栏的几块:模型指标、过程报告(花费 / 遗留问题 / 修复过程)、参数面板。

use leptos::prelude::*;
use leptos_i18n::{t_string, td_string};
use pp_common::cad::{CadBuildReport, CadParam, CadProblem, CadVersion};

use crate::i18n::{use_i18n, Locale};
use crate::ui::{Badge, Card, SectionTitle, Tone};
use crate::utils::{fmt_int, fmt_mm, parse_positive};

fn mm3(v: [f64; 3]) -> String {
    format!("{} × {} × {}", fmt_mm(v[0]), fmt_mm(v[1]), fmt_mm(v[2]))
}

/// 一条问题 → 当前语言的一句话。后端给的是带类型的数据(`kind` + 数字),不是现成的句子。
pub fn problem_text(l: Locale, p: &CadProblem) -> String {
    match p {
        CadProblem::Contract { detail } => td_string!(l, cad.p_contract, detail = detail).to_string(),
        CadProblem::Script { error } => {
            let at = error.line.map(|n| format!(" (line {n})")).unwrap_or_default();
            let detail = format!("{}: {}{at}", error.error_type, error.message);
            td_string!(l, cad.p_script, detail = detail).to_string()
        }
        CadProblem::Timeout { secs } => td_string!(l, cad.p_timeout, secs = *secs).to_string(),
        CadProblem::Solids { count: 0 } => td_string!(l, cad.p_no_volume).to_string(),
        CadProblem::Solids { count } => td_string!(l, cad.p_solids, count = *count).to_string(),
        CadProblem::Invalid => td_string!(l, cad.p_invalid).to_string(),
        CadProblem::OffPlate { z } => td_string!(l, cad.p_off_plate, z = format!("{z:.2}")).to_string(),
        CadProblem::Size { got, want } => td_string!(l, cad.p_size, got = mm3(*got), want = mm3(*want)).to_string(),
        CadProblem::TooBig { size, .. } => td_string!(l, cad.p_too_big, size = mm3(*size)).to_string(),
        CadProblem::NoParams => td_string!(l, cad.p_no_params).to_string(),
        CadProblem::ParamRange { name, value, .. } => td_string!(l, cad.p_param_range, name = name, value = *value).to_string(),
        CadProblem::Unchanged => td_string!(l, cad.p_unchanged).to_string(),
    }
}

/// `changed_sections` → 给人看的文字:`-name` 是删掉的段;其余原样。
pub fn changed_text(sections: &[String]) -> String {
    sections
        .iter()
        .map(|s| match s.strip_prefix('-') {
            Some(removed) => format!("− {removed}"),
            None => s.clone(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[component]
pub fn MetricsCard(version: CadVersion) -> impl IntoView {
    let i18n = use_i18n();
    let m = version.metrics.clone();
    let fits = m.fits(super::BUILD_VOLUME) || m.fits([super::BUILD_VOLUME[1], super::BUILD_VOLUME[0], super::BUILD_VOLUME[2]]);
    let yes_no = move |ok: bool| {
        if ok {
            view! { <Badge tone=Tone::Green>{move || t_string!(i18n, common.yes)}</Badge> }
        } else {
            view! { <Badge tone=Tone::Red>{move || t_string!(i18n, common.no)}</Badge> }
        }
    };
    let row = move |label: Signal<String>, value: AnyView| {
        view! {
            <div class="flex items-center justify-between gap-3 py-1 text-xs">
                <span class="text-gray-500 dark:text-gray-400">{move || label.get()}</span>
                <span class="tabular-nums text-gray-900 dark:text-gray-100 selectable">{value}</span>
            </div>
        }
    };
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    let one_solid = if m.solids == 1 {
        "1".into_any()
    } else {
        view! { <span class="text-red-600 dark:text-red-400 font-medium">{m.solids}</span> }.into_any()
    };

    view! {
        <Card class="p-4 space-y-2">
            <SectionTitle title=move || t_string!(i18n, cad.metrics)/>
            <div>
                {row(label(|l| td_string!(l, lab.size)), format!("{} mm", mm3(m.size)).into_any())}
                {row(label(|l| td_string!(l, lab.volume)), format!("{:.1} cm³", m.volume_mm3 / 1000.0).into_any())}
                {row(label(|l| td_string!(l, lab.area)), format!("{:.1} cm²", m.area_mm2 / 100.0).into_any())}
                {row(label(|l| td_string!(l, cad.faces_edges)), format!("{} / {}", fmt_int(m.faces), fmt_int(m.edges)).into_any())}
                {row(label(|l| td_string!(l, cad.solids)), one_solid)}
                {row(label(|l| td_string!(l, cad.valid)), yes_no(m.is_valid).into_any())}
                {row(label(|l| td_string!(l, lab.fits_bed)), yes_no(fits).into_any())}
                {row(label(|l| td_string!(l, cad.exec_time)), format!("{:.1} s", version.elapsed_ms as f64 / 1000.0).into_any())}
            </div>
        </Card>
    }
}

#[component]
fn ProblemList(problems: Vec<CadProblem>, #[prop(optional)] muted: bool) -> impl IntoView {
    let i18n = use_i18n();
    let color = if muted {
        "text-gray-500 dark:text-gray-400"
    } else {
        "text-amber-700 dark:text-amber-300"
    };
    view! {
        <ul class=format!("list-disc pl-4 space-y-1 text-xs leading-relaxed selectable {color}")>
            {problems
                .into_iter()
                .map(|p| view! { <li class="break-words">{move || problem_text(i18n.get_locale(), &p)}</li> })
                .collect_view()}
        </ul>
    }
}

/// 这一版是怎么来的:花费、自动贴床、遗留问题、每一轮没通过的原因。
#[component]
pub fn ReportCard(report: CadBuildReport, #[prop(optional)] failed: bool) -> impl IntoView {
    let i18n = use_i18n();
    let r = report.clone();
    let rounds: Vec<(usize, Vec<CadProblem>)> = report.rounds.iter().cloned().enumerate().collect();
    let warnings = report.warnings.clone();
    let changed = changed_text(&report.changed_sections);

    view! {
        <Card class="p-4 space-y-3">
            {failed.then(|| view! {
                <div class="space-y-1">
                    <div class="text-sm font-semibold text-red-600 dark:text-red-400">{move || t_string!(i18n, cad.failed_title)}</div>
                    <p class="text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || t_string!(i18n, cad.failed_hint)}</p>
                </div>
            })}
            <p class="text-xs tabular-nums text-gray-500 dark:text-gray-400 selectable">
                {move || t_string!(
                    i18n,
                    cad.usage,
                    calls = r.llm_calls,
                    runs = r.runs,
                    yuan = format!("{:.3}", r.cost_fen / 100.0),
                    secs = format!("{:.1}", r.elapsed_ms as f64 / 1000.0),
                ).to_string()}
            </p>
            {(!changed.is_empty()).then(|| view! {
                <p class="text-xs text-gray-700 dark:text-gray-200 selectable">
                    {move || t_string!(i18n, cad.changed, sections = changed.clone()).to_string()}
                </p>
            })}
            {report.plate_snapped.then(|| view! {
                <p class="text-xs text-gray-500 dark:text-gray-400">{move || t_string!(i18n, cad.snapped)}</p>
            })}
            {(!warnings.is_empty()).then(|| view! {
                <div class="space-y-1.5">
                    <div class="text-xs font-medium text-amber-700 dark:text-amber-300">{move || t_string!(i18n, cad.warnings)}</div>
                    <ProblemList problems=warnings.clone()/>
                </div>
            })}
            {(!rounds.is_empty()).then(|| view! {
                <details class="group">
                    <summary class="cursor-pointer text-xs font-medium text-gray-600 dark:text-gray-300">
                        {move || t_string!(i18n, cad.rounds)}
                    </summary>
                    <div class="mt-2 space-y-2">
                        {rounds
                            .clone()
                            .into_iter()
                            .map(|(i, problems)| view! {
                                <div class="space-y-1">
                                    <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                                        {move || t_string!(i18n, cad.round_n, n = i + 1).to_string()}
                                    </div>
                                    <ProblemList problems=problems muted=true/>
                                </div>
                            })
                            .collect_view()}
                    </div>
                </details>
            })}
        </Card>
    }
}

fn fmt_value(p: &CadParam) -> String {
    if p.integer || p.value.fract() == 0.0 {
        format!("{:.0}", p.value)
    } else {
        format!("{}", p.value)
    }
}

/// 输入框里的文字 → 数值(整数参数四舍五入)。看不懂返回 `None`。
fn parse_param_number(p: &CadParam, text: &str) -> Option<f64> {
    let raw = if text.trim() == "0" { 0.0 } else { parse_positive(text)? };
    Some(if p.integer { raw.round() } else { raw })
}

/// 用户敲的新值 → 要不要提交重建。没变不提交;超出参数自己声明的范围不提交(后端也会再查一遍)。
pub fn accept_param_input(p: &CadParam, text: &str) -> Option<f64> {
    let value = parse_param_number(p, text)?;
    let in_range = p.min.is_none_or(|min| value >= min) && p.max.is_none_or(|max| value <= max);
    (in_range && (value - p.value).abs() > 1e-9).then_some(value)
}

fn slider_step(p: &CadParam, min: f64, max: f64) -> f64 {
    if p.integer {
        1.0
    } else if max - min >= 20.0 {
        0.5
    } else {
        0.1
    }
}

/// 参数面板:PARAMS 段里的每个参数一行。改数字回车(或拖完滑块)→ 直接重建,不经过模型。
#[component]
pub fn ParamsCard(params: Vec<CadParam>, #[prop(into)] busy: Signal<bool>, #[prop(into)] on_change: Callback<(String, f64)>) -> impl IntoView {
    let i18n = use_i18n();
    let empty = params.is_empty();
    view! {
        <Card class="p-4 space-y-3">
            <SectionTitle title=move || t_string!(i18n, cad.params) hint=move || t_string!(i18n, cad.params_hint)/>
            {empty.then(|| view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, cad.no_params)}</p> })}
            <div class="space-y-3">
                {params.into_iter().map(|p| view! { <ParamRow param=p busy=busy on_change=on_change/> }).collect_view()}
            </div>
        </Card>
    }
}

#[component]
fn ParamRow(param: CadParam, busy: Signal<bool>, on_change: Callback<(String, f64)>) -> impl IntoView {
    let text = RwSignal::new(fmt_value(&param));
    let stored = StoredValue::new(param.clone());
    let invalid = RwSignal::new(false);
    let submit = move |raw: String| {
        stored.with_value(|p| match accept_param_input(p, &raw) {
            Some(v) => {
                invalid.set(false);
                on_change.run((p.name.clone(), v));
            }
            // 没变 = 不用做事;看不懂或超范围 = 标红
            None => {
                let unchanged = parse_param_number(p, &raw).is_some_and(|v| (v - p.value).abs() <= 1e-9);
                invalid.set(!unchanged);
            }
        });
    };
    let title = if param.label.is_empty() { param.name.clone() } else { param.label.clone() };
    let range = match (param.min, param.max) {
        (Some(lo), Some(hi)) => format!("{} ~ {}", fmt_mm(lo).trim_end_matches(".0"), fmt_mm(hi).trim_end_matches(".0")),
        _ => String::new(),
    };
    let slider = param.min.zip(param.max).filter(|(lo, hi)| hi > lo);

    view! {
        <div class="space-y-1">
            <div class="flex items-baseline justify-between gap-2">
                <span class="text-xs font-medium text-gray-700 dark:text-gray-200 truncate" title=param.name.clone()>{title}</span>
                <span class="text-[11px] tabular-nums text-gray-400 shrink-0">{range}</span>
            </div>
            <div class="flex items-center gap-2">
                <input
                    type="text"
                    inputmode="decimal"
                    autocomplete="off"
                    spellcheck="false"
                    class="w-20 h-8 px-2 rounded-lg border bg-white dark:bg-gray-900 text-sm tabular-nums text-gray-900 dark:text-gray-100 \
                           focus:outline-none focus:ring-2 focus:ring-brand/40 focus:border-brand disabled:opacity-50"
                    class=("border-gray-300", move || !invalid.get())
                    class=("dark:border-gray-600", move || !invalid.get())
                    class=("border-red-500", move || invalid.get())
                    disabled=move || busy.get()
                    prop:value=move || text.get()
                    on:input=move |e| text.set(event_target_value(&e))
                    on:keydown=move |e| {
                        if e.key() == "Enter" && !e.is_composing() {
                            submit(text.get_untracked());
                        }
                    }
                />
                <span class="text-xs text-gray-400 w-6 shrink-0">{param.unit.clone()}</span>
                {slider.map(|(lo, hi)| {
                    let step = stored.with_value(|p| slider_step(p, lo, hi));
                    view! {
                        <input
                            type="range"
                            class="flex-1 min-w-0 accent-indigo-600"
                            min=lo
                            max=hi
                            step=step
                            disabled=move || busy.get()
                            prop:value=move || text.with(|t| parse_positive(t).unwrap_or(lo))
                            on:input=move |e| text.set(event_target_value(&e))
                            // change 只在松手时触发:拖动过程中不重建
                            on:change=move |e| submit(event_target_value(&e))
                        />
                    }
                })}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;
    use pp_common::cad::CadScriptError;

    fn param(value: f64, integer: bool) -> CadParam {
        CadParam {
            name: "slot_count".into(),
            value,
            unit: "个".into(),
            label: "线槽数量".into(),
            min: Some(1.0),
            max: Some(8.0),
            line: 5,
            integer,
        }
    }

    #[test]
    fn param_input_is_rounded_range_checked_and_ignored_when_unchanged() {
        assert_eq!(accept_param_input(&param(3.0, true), "5"), Some(5.0));
        assert_eq!(accept_param_input(&param(3.0, true), "4.6"), Some(5.0), "整数参数四舍五入");
        assert_eq!(accept_param_input(&param(3.0, false), "4.6"), Some(4.6));
        assert_eq!(accept_param_input(&param(3.0, true), "3"), None, "没变就不重建");
        assert_eq!(accept_param_input(&param(3.0, true), "9"), None, "超出范围");
        assert_eq!(accept_param_input(&param(3.0, true), "abc"), None);
        assert_eq!(accept_param_input(&param(3.0, true), "5"), Some(5.0), "全角数字也认");
    }

    #[test]
    fn every_problem_kind_has_a_sentence_in_every_locale() {
        let problems = [
            CadProblem::Contract { detail: "Add a PARAMS section".into() },
            CadProblem::Script {
                error: CadScriptError {
                    stage: "exec".into(),
                    error_type: "ValueError".into(),
                    message: "Failed creating a chamfer".into(),
                    line: Some(24),
                    traceback: String::new(),
                },
            },
            CadProblem::Timeout { secs: 90 },
            CadProblem::Solids { count: 0 },
            CadProblem::Solids { count: 3 },
            CadProblem::Invalid,
            CadProblem::OffPlate { z: -3.0 },
            CadProblem::Size { got: [60.0, 24.0, 12.0], want: [60.0, 24.0, 16.0] },
            CadProblem::TooBig { size: [300.0, 24.0, 12.0], build: [256.0; 3] },
            CadProblem::NoParams,
            CadProblem::ParamRange { name: "width".into(), value: 5.0, min: Some(10.0), max: None },
            CadProblem::Unchanged,
        ];
        for l in [Locale::zh, Locale::en] {
            for p in &problems {
                assert!(!problem_text(l, p).is_empty());
            }
        }
        let zh = problem_text(Locale::zh, &problems[1]);
        assert!(zh.contains("ValueError") && zh.contains("line 24"), "{zh}");
        assert!(problem_text(Locale::zh, &problems[4]).contains('3'));
        assert!(problem_text(Locale::zh, &problems[7]).contains("60.0 × 24.0 × 12.0"));
    }

    #[test]
    fn removed_sections_are_marked_in_the_change_summary() {
        assert_eq!(changed_text(&["slot".into(), "-holes".into()]), "slot · − holes");
        assert_eq!(changed_text(&[]), "");
    }
}
