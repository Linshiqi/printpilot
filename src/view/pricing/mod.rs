//! 打样与定价(一级入口,侧栏「定价」):每个单品上架前就知道「卖多少钱、赚多少、一天最多接多少单」。
//!
//!   左:项目列表(成本模型是项目的东西,不像画板 / 设计那样可以不挂项目)
//!   中:成本参数——这一件(克数、时长、损耗、成功率)· 机器与耗材 · 人工与杂费 · 渠道 · 产能
//!   右:成本拆解 → 三档建议价 / 自己定价 → 这个价下的账(毛利率、每机时毛利、竞品价格带)→ 产能 → 打样记录
//!
//! 算钱的公式全在 `pp_common::cost`(纯函数):改任何一个格子都在本地即时重算,不走后端;
//! 改完停手约半秒自动保存。设计见 docs/adr/0007-cost-pricing.md。

mod profiles;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::cost::{
    breakdown, capacity, check_price, estimate_print, fee_preset, latest_actuals, observed_success_rate, price_tiers, CostDefaults, CostModel, CostParams,
    Material, ModelGeometry, PriceCheck, PricingDetail, PrintRun, Printer, FEE_PRESETS,
};
use pp_common::{Project, ProjectStatus};

use crate::i18n::{use_i18n, Locale};
use crate::i18n_util::{current_locale, stage_name};
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::{AppState, Handoff};
use crate::theme::{get_pref, set_pref};
use crate::ui::{copy_entry, item, separator, Badge, Button, ButtonVariant, Card, Dialog, EmptyState, IconButton, NumInput, SectionTitle, Segmented, TextInput, Tone};
use crate::utils::{format_ts, local_tz_offset_minutes};
use profiles::ProfilesPanel;

/// 停手多久之后自动保存
const AUTOSAVE_MS: u64 = 600;

fn yuan(v: f64) -> String {
    format!("¥{v:.2}")
}

fn percent(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

pub(super) fn fee_name(l: Locale, key: &str) -> &'static str {
    match key {
        "xhs_general" => td_string!(l, pricing.fee_xhs_general),
        "xhs_custom" => td_string!(l, pricing.fee_xhs_custom),
        "xianyu" => td_string!(l, pricing.fee_xianyu),
        "wechat_shop" => td_string!(l, pricing.fee_wechat_shop),
        _ => td_string!(l, pricing.fee_custom),
    }
}

/// 成本参数的每个格子一个信号(`NumInput` 要绑 `RwSignal<f64>`);`params()` 把它们拼回一份 `CostParams`。
#[derive(Clone, Copy)]
struct Fields {
    grams: RwSignal<f64>,
    print_hours: RwSignal<f64>,
    waste_rate: RwSignal<f64>,
    success_rate: RwSignal<f64>,
    printer_id: RwSignal<Option<String>>,
    material_id: RwSignal<Option<String>>,
    material_yuan_per_kg: RwSignal<f64>,
    power_w: RwSignal<f64>,
    electricity: RwSignal<f64>,
    printer_price: RwSignal<f64>,
    printer_lifetime: RwSignal<f64>,
    labor_minutes: RwSignal<f64>,
    labor_rate: RwSignal<f64>,
    packaging: RwSignal<f64>,
    shipping: RwSignal<f64>,
    channel: RwSignal<String>,
    fee_rate: RwSignal<f64>,
    printers: RwSignal<f64>,
    hours_per_day: RwSignal<f64>,
    /// 定下来的售价;0 = 还没定
    price: RwSignal<f64>,
}

impl Fields {
    fn new() -> Self {
        let p = CostParams::default();
        Self {
            grams: RwSignal::new(p.grams),
            print_hours: RwSignal::new(p.print_hours),
            waste_rate: RwSignal::new(p.waste_rate),
            success_rate: RwSignal::new(p.success_rate),
            printer_id: RwSignal::new(None),
            material_id: RwSignal::new(None),
            material_yuan_per_kg: RwSignal::new(p.material_yuan_per_kg),
            power_w: RwSignal::new(p.power_w),
            electricity: RwSignal::new(p.electricity_yuan_per_kwh),
            printer_price: RwSignal::new(p.printer_price_yuan),
            printer_lifetime: RwSignal::new(p.printer_lifetime_hours),
            labor_minutes: RwSignal::new(p.labor_minutes),
            labor_rate: RwSignal::new(p.labor_yuan_per_min),
            packaging: RwSignal::new(p.packaging_yuan),
            shipping: RwSignal::new(p.shipping_subsidy_yuan),
            channel: RwSignal::new(p.channel),
            fee_rate: RwSignal::new(p.fee_rate),
            printers: RwSignal::new(p.printers as f64),
            hours_per_day: RwSignal::new(p.hours_per_day),
            price: RwSignal::new(0.0),
        }
    }

    fn load(&self, m: &CostModel) {
        let p = &m.params;
        self.grams.set(p.grams);
        self.print_hours.set(p.print_hours);
        self.waste_rate.set(p.waste_rate);
        self.success_rate.set(p.success_rate);
        self.printer_id.set(p.printer_id.clone());
        self.material_id.set(p.material_id.clone());
        self.material_yuan_per_kg.set(p.material_yuan_per_kg);
        self.power_w.set(p.power_w);
        self.electricity.set(p.electricity_yuan_per_kwh);
        self.printer_price.set(p.printer_price_yuan);
        self.printer_lifetime.set(p.printer_lifetime_hours);
        self.labor_minutes.set(p.labor_minutes);
        self.labor_rate.set(p.labor_yuan_per_min);
        self.packaging.set(p.packaging_yuan);
        self.shipping.set(p.shipping_subsidy_yuan);
        self.channel.set(p.channel.clone());
        self.fee_rate.set(p.fee_rate);
        self.printers.set(p.printers as f64);
        self.hours_per_day.set(p.hours_per_day);
        self.price.set(m.chosen_price.unwrap_or(0.0));
    }

    /// 在响应式上下文里调用:跟踪每一个格子(任何一个变了都会重算)。
    fn params(&self) -> CostParams {
        self.read(true)
    }

    /// 事件回调、定时器里用:只取值、不跟踪(在那里用跟踪式的读取,Leptos 会逐个格子报警告)。
    fn params_untracked(&self) -> CostParams {
        self.read(false)
    }

    fn read(&self, tracked: bool) -> CostParams {
        let num = |s: RwSignal<f64>| if tracked { s.get() } else { s.get_untracked() };
        let opt = |s: RwSignal<Option<String>>| if tracked { s.get() } else { s.get_untracked() };
        CostParams {
            grams: num(self.grams),
            print_hours: num(self.print_hours),
            waste_rate: num(self.waste_rate),
            success_rate: num(self.success_rate),
            printer_id: opt(self.printer_id),
            material_id: opt(self.material_id),
            material_yuan_per_kg: num(self.material_yuan_per_kg),
            power_w: num(self.power_w),
            electricity_yuan_per_kwh: num(self.electricity),
            printer_price_yuan: num(self.printer_price),
            printer_lifetime_hours: num(self.printer_lifetime),
            labor_minutes: num(self.labor_minutes),
            labor_yuan_per_min: num(self.labor_rate),
            packaging_yuan: num(self.packaging),
            shipping_subsidy_yuan: num(self.shipping),
            channel: if tracked { self.channel.get() } else { self.channel.get_untracked() },
            fee_rate: num(self.fee_rate),
            printers: num(self.printers).round().clamp(1.0, 999.0) as u32,
            hours_per_day: num(self.hours_per_day),
        }
    }

    fn chosen_price(&self) -> Option<f64> {
        let p = self.price.get();
        (p > 0.0).then_some(p)
    }

    fn chosen_price_untracked(&self) -> Option<f64> {
        let p = self.price.get_untracked();
        (p > 0.0).then_some(p)
    }
}

/// 一行「标签 + 格子」。
#[component]
fn Cell(#[prop(into)] label: Signal<String>, children: Children) -> impl IntoView {
    view! {
        <label class="block space-y-1 min-w-0">
            <span class="block text-[11px] text-gray-500 dark:text-gray-400 truncate">{move || label.get()}</span>
            {children()}
        </label>
    }
}

#[component]
pub fn PricingView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let tz = local_tz_offset_minutes();
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());

    // ---- 状态 ----
    let project_id = RwSignal::new(None::<String>);
    let fields = Fields::new();
    let runs = RwSignal::new(Vec::<PrintRun>::new());
    let band = RwSignal::new(None::<(u32, u32)>);
    let geometry = RwSignal::new(None::<ModelGeometry>);
    let printers = RwSignal::new(Vec::<Printer>::new());
    let materials = RwSignal::new(Vec::<Material>::new());
    let defaults = RwSignal::new(CostDefaults::default());
    let profiles_open = RwSignal::new(false);
    // 上一次存进库里的样子(JSON):和现在不一样才保存
    let saved_json = StoredValue::new(String::new());
    let save_timer = StoredValue::new(None::<TimeoutHandle>);
    let saved_flash = RwSignal::new(false);

    let project = Memo::new(move |_| {
        let id = project_id.get()?;
        state.projects.with(|list| list.iter().find(|p| p.id == id).cloned())
    });
    let params = Memo::new(move |_| fields.params());
    let cost = Memo::new(move |_| breakdown(&params.get()));
    let tiers = Memo::new(move |_| price_tiers(&cost.get(), fields.fee_rate.get(), &defaults.get().target_margins));
    let chosen = Memo::new(move |_| fields.chosen_price().map(|p| check_price(p, fields.fee_rate.get(), &cost.get())));
    let snapshot = move || serde_json::json!({ "params": fields.params(), "price": fields.chosen_price() }).to_string();

    // ---- 数据 ----
    let load_profiles = move || {
        spawn_local(async move {
            if let Ok(list) = ipc::call_no_args::<Vec<Printer>>(cmd::PRINTER_LIST).await {
                printers.set(list);
            }
            if let Ok(list) = ipc::call_no_args::<Vec<Material>>(cmd::MATERIAL_LIST).await {
                materials.set(list);
            }
            if let Ok(d) = ipc::call_no_args::<CostDefaults>(cmd::COST_DEFAULTS_GET).await {
                defaults.set(d);
            }
        });
    };
    load_profiles();
    let open = move |id: String| {
        spawn_local(async move {
            match ipc::call::<_, PricingDetail>(cmd::PRICING_GET, &serde_json::json!({ "project_id": id })).await {
                Ok(d) => {
                    set_pref("pricing_project", &id);
                    fields.load(&d.model);
                    runs.set(d.runs);
                    band.set(d.price_band);
                    geometry.set(d.geometry);
                    // 刚读出来的就是库里的样子:草稿(还没存过)不算,第一次改动才会落库
                    saved_json.set_value(serde_json::json!({ "params": d.model.params, "price": d.model.chosen_price }).to_string());
                    project_id.set(Some(id));
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    // 从项目中枢过来的:打开那个项目;否则回到上次的
    let wanted = match state.handoff.get_untracked() {
        Some(Handoff::Pricing { project_id }) => {
            state.handoff.set(None);
            Some(project_id)
        }
        _ => get_pref("pricing_project"),
    };
    // 项目列表是异步读进来的:等它到了再决定打开哪个(只做一次)
    let picked = StoredValue::new(false);
    Effect::new(move |_| {
        if picked.get_value() || !state.projects_loaded.get() {
            return;
        }
        picked.set_value(true);
        let pick = state.projects.with_untracked(|list| {
            list.iter().find(|p| Some(&p.id) == wanted.as_ref()).or_else(|| list.iter().find(|p| p.status == ProjectStatus::Active)).map(|p| p.id.clone())
        });
        if let Some(id) = pick {
            open(id);
        }
    });

    // ---- 自动保存:和库里的不一样 → 停手半秒后存 ----
    Effect::new(move |_| {
        let now = snapshot();
        let Some(id) = project_id.get_untracked() else { return };
        if saved_json.with_value(|s| s == &now) {
            return;
        }
        if let Some(h) = save_timer.get_value() {
            h.clear();
        }
        let handle = set_timeout_with_handle(
            move || {
                let (p, price) = (fields.params_untracked(), fields.chosen_price_untracked());
                let json = serde_json::json!({ "params": p, "price": price }).to_string();
                // 期间换了项目就算了(新项目自己的快照已经就位)
                if project_id.get_untracked().as_deref() != Some(id.as_str()) {
                    return;
                }
                spawn_local(async move {
                    let args = serde_json::json!({ "project_id": id, "params": p, "chosen_price": price });
                    match ipc::call::<_, CostModel>(cmd::PRICING_SAVE, &args).await {
                        Ok(_) => {
                            saved_json.set_value(json);
                            saved_flash.set(true);
                            set_timeout(move || saved_flash.set(false), std::time::Duration::from_millis(1500));
                            // 「定了售价」是项目「打样」阶段的清单项
                            state.reload_project_facts();
                        }
                        Err(e) => state.notify_error(e),
                    }
                });
            },
            std::time::Duration::from_millis(AUTOSAVE_MS),
        );
        save_timer.set_value(handle.ok());
    });

    // ---- 带入 ----
    let use_printer = move |id: String| {
        let p = printers.with_untracked(|l| l.iter().find(|p| p.id == id).cloned()).unwrap_or_default();
        fields.printer_id.set((!p.id.is_empty()).then(|| p.id.clone()));
        fields.power_w.set(p.power_w);
        fields.printer_price.set(p.price_yuan);
        fields.printer_lifetime.set(p.lifetime_hours);
    };
    let use_material = move |id: String| {
        let m = materials.with_untracked(|l| l.iter().find(|m| m.id == id).cloned()).unwrap_or_default();
        fields.material_id.set((!m.id.is_empty()).then(|| m.id.clone()));
        fields.material_yuan_per_kg.set(m.yuan_per_kg);
    };
    let use_channel = move |key: String| {
        if let Some(preset) = fee_preset(&key).filter(|p| p.key != "custom") {
            fields.fee_rate.set(preset.rate);
        }
        fields.channel.set(key);
    };
    // 从名下的模型估:体积法,用选中的耗材密度和打印机出料速度
    let estimate = move || {
        let Some(g) = geometry.get_untracked() else { return };
        let density = fields
            .material_id
            .with_untracked(|id| materials.with_untracked(|l| l.iter().find(|m| Some(&m.id) == id.as_ref()).map(|m| m.density)))
            .unwrap_or(Material::default().density);
        let speed = fields
            .printer_id
            .with_untracked(|id| printers.with_untracked(|l| l.iter().find(|p| Some(&p.id) == id.as_ref()).map(|p| p.grams_per_hour)))
            .unwrap_or(Printer::default().grams_per_hour);
        let (grams, hours) = estimate_print(g.volume_mm3, g.area_mm2, density, speed);
        fields.grams.set((grams * 10.0).round() / 10.0);
        fields.print_hours.set((hours * 100.0).round() / 100.0);
    };
    let use_actuals = move || {
        if let Some((grams, hours)) = runs.with_untracked(|r| latest_actuals(r)) {
            fields.grams.set(grams);
            fields.print_hours.set(hours);
        }
    };
    let use_observed_rate = move || {
        if let Some(rate) = runs.with_untracked(|r| observed_success_rate(r)) {
            fields.success_rate.set(rate.max(0.05));
        }
    };

    // ---- 打样记录 ----
    let run_success = RwSignal::new(true);
    let run_grams = RwSignal::new(0.0);
    let run_hours = RwSignal::new(0.0);
    let run_reason = RwSignal::new(String::new());
    let run_note = RwSignal::new(String::new());
    let can_add_run = Signal::derive(move || project_id.with(Option::is_some) && (run_success.get() || !run_reason.with(|r| r.trim().is_empty())));
    let add_run = move || {
        let Some(id) = project_id.get_untracked() else { return };
        if !can_add_run.get_untracked() {
            return;
        }
        let positive = |v: f64| (v > 0.0).then_some(v);
        let p = fields.params_untracked();
        let run = PrintRun {
            id: String::new(),
            project_id: id.clone(),
            printer_id: p.printer_id,
            material_id: p.material_id,
            est_hours: positive(p.print_hours),
            est_grams: positive(p.grams),
            actual_hours: positive(run_hours.get_untracked()),
            actual_grams: positive(run_grams.get_untracked()),
            success: run_success.get_untracked(),
            fail_reason: run_reason.get_untracked(),
            note: run_note.get_untracked(),
            created_at: 0,
        };
        spawn_local(async move {
            match ipc::call::<_, PrintRun>(cmd::PRINT_RUN_ADD, &serde_json::json!({ "project_id": id, "run": run })).await {
                Ok(saved) => {
                    runs.update(|l| l.insert(0, saved));
                    run_grams.set(0.0);
                    run_hours.set(0.0);
                    run_reason.set(String::new());
                    run_note.set(String::new());
                    state.reload_project_facts();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let delete_run = move |id: String| {
        spawn_local(async move {
            match ipc::call_unit(cmd::PRINT_RUN_DELETE, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    runs.update(|l| l.retain(|r| r.id != id));
                    state.reload_project_facts();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };

    // `view!` 的属性里不能直接写 `>`(会被当成标签结束),比较放在宏外面
    let has_price = move || fields.price.get() > 0.0;
    let line = move |name: Signal<String>, detail: Signal<String>, amount: Signal<f64>| {
        view! {
            <div class="flex items-baseline gap-2 text-xs">
                <span class="shrink-0 w-16 text-gray-600 dark:text-gray-300">{move || name.get()}</span>
                <span class="min-w-0 flex-1 truncate text-gray-400 tabular-nums">{move || detail.get()}</span>
                <span class="shrink-0 tabular-nums text-gray-800 dark:text-gray-100">{move || yuan(amount.get())}</span>
            </div>
        }
    };

    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center justify-between gap-4 border-b border-gray-200 dark:border-gray-700">
                <div class="min-w-0">
                    <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, pricing.title)}</h1>
                    <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, pricing.subtitle)}</p>
                </div>
                <div class="shrink-0 flex items-center gap-3">
                    <span class="text-[11px] text-green-600 dark:text-green-400 transition-opacity" class=("opacity-0", move || !saved_flash.get())>
                        {move || t_string!(i18n, pricing.autosaved)}
                    </span>
                    <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Settings on_click=move || profiles_open.set(true)>
                        {move || t_string!(i18n, pricing.profiles)}
                    </Button>
                </div>
            </header>

            <div class="flex-1 min-h-0 flex">
                // ---- 左:项目 ----
                <aside class="w-56 shrink-0 h-full overflow-y-auto border-r border-gray-200 dark:border-gray-700 p-2 space-y-1">
                    <Show when=move || state.projects_loaded.get() && state.projects.with(Vec::is_empty)>
                        <p class="p-3 text-xs leading-relaxed text-gray-400">{move || t_string!(i18n, pricing.no_projects)}</p>
                    </Show>
                    <For each=move || state.projects.get() key=|p| (p.id.clone(), p.updated_at, p.stage_entered_at) let:p>
                        <ProjectRow state=state project=p current=project_id on_open=open/>
                    </For>
                </aside>

                <Show
                    when=move || project.with(Option::is_some)
                    fallback=move || view! {
                        <div class="flex-1">
                            <EmptyState icon=IconKind::Ruler title=move || t_string!(i18n, pricing.empty_title) hint=move || t_string!(i18n, pricing.empty_hint)/>
                        </div>
                    }
                >
                    // ---- 中:成本参数 ----
                    <section class="flex-1 min-w-0 h-full overflow-y-auto p-4 space-y-3">
                        <div class="flex items-center gap-2">
                            <h2 class="min-w-0 truncate text-sm font-semibold text-gray-900 dark:text-gray-50">
                                {move || project.with(|p| p.as_ref().map(|p| format!("{} · {}", p.code, p.title)).unwrap_or_default())}
                            </h2>
                            <IconButton
                                icon=IconKind::Kanban
                                label=move || t_string!(i18n, project.open_hub)
                                on_click=move || state.open_project.set(project_id.get_untracked())
                            />
                        </div>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_unit) hint=move || t_string!(i18n, pricing.sec_unit_hint)/>
                            <div class="grid grid-cols-4 gap-3">
                                <Cell label=label(|l| td_string!(l, pricing.grams))><NumInput value=fields.grams unit="g"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.print_hours))><NumInput value=fields.print_hours unit="h"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.waste_rate))><NumInput value=fields.waste_rate scale=100.0 unit="%"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.success_rate))><NumInput value=fields.success_rate scale=100.0 unit="%"/></Cell>
                            </div>
                            <div class="flex flex-wrap items-center gap-2">
                                <Show when=move || geometry.with(Option::is_some)>
                                    <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Box on_click=estimate>
                                        {move || t_string!(i18n, pricing.estimate_from_model, n = geometry.with(|g| g.map(|g| g.designs).unwrap_or(0))).to_string()}
                                    </Button>
                                </Show>
                                <Show when=move || runs.with(|r| latest_actuals(r).is_some())>
                                    <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Check on_click=use_actuals>
                                        {move || t_string!(i18n, pricing.use_actuals)}
                                    </Button>
                                </Show>
                                {move || runs.with(|r| observed_success_rate(r)).map(|rate| view! {
                                    <button type="button" class="text-[11px] text-brand hover:underline" on:click=move |_| use_observed_rate()>
                                        {move || t_string!(i18n, pricing.use_observed_rate, rate = percent(rate)).to_string()}
                                    </button>
                                })}
                            </div>
                            <Show when=move || !geometry.with(Option::is_some) && !params.with(CostParams::is_ready)>
                                <p class="text-[11px] leading-relaxed text-gray-400">{move || t_string!(i18n, pricing.no_model_hint)}</p>
                            </Show>
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_machine)/>
                            <div class="grid grid-cols-4 gap-3">
                                <div class="col-span-2">
                                    <Cell label=label(|l| td_string!(l, pricing.printer))>
                                        <select
                                            class="w-full h-8 px-2 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-gray-100"
                                            on:change=move |e| use_printer(event_target_value(&e))
                                        >
                                            <option value="" selected=move || fields.printer_id.with(Option::is_none)>{move || t_string!(i18n, pricing.factory_default)}</option>
                                            {move || printers.get().into_iter().map(|p| {
                                                let pid = p.id.clone();
                                                view! { <option value=p.id.clone() selected=move || fields.printer_id.with(|c| c.as_deref() == Some(pid.as_str()))>{p.name.clone()}</option> }
                                            }).collect_view()}
                                        </select>
                                    </Cell>
                                </div>
                                <div class="col-span-2">
                                    <Cell label=label(|l| td_string!(l, pricing.material))>
                                        <select
                                            class="w-full h-8 px-2 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-gray-100"
                                            on:change=move |e| use_material(event_target_value(&e))
                                        >
                                            <option value="" selected=move || fields.material_id.with(Option::is_none)>{move || t_string!(i18n, pricing.factory_default)}</option>
                                            {move || materials.get().into_iter().map(|m| {
                                                let mid = m.id.clone();
                                                view! { <option value=m.id.clone() selected=move || fields.material_id.with(|c| c.as_deref() == Some(mid.as_str()))>{format!("{} · {}", m.name, m.kind)}</option> }
                                            }).collect_view()}
                                        </select>
                                    </Cell>
                                </div>
                                <Cell label=label(|l| td_string!(l, pricing.power_w))><NumInput value=fields.power_w decimals=0 unit="W"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.printer_price))><NumInput value=fields.printer_price decimals=0 unit="¥"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.printer_lifetime))><NumInput value=fields.printer_lifetime decimals=0 unit="h"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.electricity))><NumInput value=fields.electricity unit="¥"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.material_price))><NumInput value=fields.material_yuan_per_kg unit="¥"/></Cell>
                            </div>
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_labor)/>
                            <div class="grid grid-cols-4 gap-3">
                                <Cell label=label(|l| td_string!(l, pricing.labor_minutes))><NumInput value=fields.labor_minutes decimals=1 unit="min"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.labor_rate))><NumInput value=fields.labor_rate unit="¥"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.packaging))><NumInput value=fields.packaging unit="¥"/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.shipping))><NumInput value=fields.shipping unit="¥"/></Cell>
                            </div>
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_channel) hint=move || t_string!(i18n, pricing.sec_channel_hint)/>
                            <div class="grid grid-cols-4 gap-3">
                                <div class="col-span-2">
                                    <Cell label=label(|l| td_string!(l, pricing.channel))>
                                        <select
                                            class="w-full h-8 px-2 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-gray-100"
                                            on:change=move |e| use_channel(event_target_value(&e))
                                        >
                                            {FEE_PRESETS.iter().map(|preset| {
                                                let key = preset.key;
                                                view! {
                                                    <option value=key selected=move || fields.channel.with(|c| c == key)>
                                                        {move || fee_name(i18n.get_locale(), key)}
                                                    </option>
                                                }
                                            }).collect_view()}
                                        </select>
                                    </Cell>
                                </div>
                                <Cell label=label(|l| td_string!(l, pricing.fee_rate))><NumInput value=fields.fee_rate scale=100.0 unit="%"/></Cell>
                            </div>
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_capacity)/>
                            <div class="grid grid-cols-4 gap-3">
                                <Cell label=label(|l| td_string!(l, pricing.printers))><NumInput value=fields.printers decimals=0/></Cell>
                                <Cell label=label(|l| td_string!(l, pricing.hours_per_day))><NumInput value=fields.hours_per_day decimals=1 unit="h"/></Cell>
                            </div>
                        </Card>
                    </section>

                    // ---- 右:成本 → 定价 → 产能 → 打样记录 ----
                    <aside class="w-[27rem] shrink-0 h-full overflow-y-auto border-l border-gray-200 dark:border-gray-700 p-4 space-y-3">
                        <Card class="p-4 space-y-2">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_cost)/>
                            {line(
                                label(|l| td_string!(l, pricing.line_material)),
                                Signal::derive(move || {
                                    let p = params.get();
                                    format!("{:.1} g × ¥{:.3}/g × (1+{}) ÷ {}", p.grams, p.material_yuan_per_kg / 1000.0, percent(p.waste_rate), percent(p.success_rate))
                                }),
                                Signal::derive(move || cost.get().material),
                            )}
                            {line(
                                label(|l| td_string!(l, pricing.line_machine)),
                                Signal::derive(move || {
                                    let (p, c) = (params.get(), cost.get());
                                    let per_hour = if c.machine_hours > 0.0 { c.machine / c.machine_hours } else { 0.0 };
                                    format!("{:.2} h ÷ {} × ¥{:.2}/h", p.print_hours, percent(p.success_rate), per_hour)
                                }),
                                Signal::derive(move || cost.get().machine),
                            )}
                            {line(
                                label(|l| td_string!(l, pricing.line_labor)),
                                Signal::derive(move || {
                                    let p = params.get();
                                    format!("{:.1} min × ¥{:.2}/min", p.labor_minutes, p.labor_yuan_per_min)
                                }),
                                Signal::derive(move || cost.get().labor),
                            )}
                            {line(
                                label(|l| td_string!(l, pricing.line_misc)),
                                Signal::derive(move || {
                                    let c = cost.get();
                                    format!("{} + {}", yuan(c.packaging), yuan(c.shipping))
                                }),
                                Signal::derive(move || cost.get().packaging + cost.get().shipping),
                            )}
                            <div class="flex items-baseline justify-between pt-2 border-t border-gray-200 dark:border-gray-700">
                                <span class="text-sm font-medium text-gray-800 dark:text-gray-100">{move || t_string!(i18n, pricing.unit_cost)}</span>
                                <span class="text-lg font-semibold tabular-nums text-gray-900 dark:text-gray-50">{move || yuan(cost.get().unit_cost)}</span>
                            </div>
                            <Show when=move || !params.with(CostParams::is_ready)>
                                <p class="text-[11px] leading-relaxed text-amber-600 dark:text-amber-400">{move || t_string!(i18n, pricing.not_ready)}</p>
                            </Show>
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_price) hint=move || t_string!(i18n, pricing.sec_price_hint)/>
                            <div class="grid grid-cols-3 gap-2">
                                // 键里要带上毛利:成本变了、但取完尾数价格没变的那一档,也得重画(不然显示的是旧毛利)
                                <For each=move || tiers.get() key=|t| ((t.price * 100.0) as i64, (t.margin * 1e4) as i64, (t.profit_per_hour * 100.0) as i64) let:tier>
                                    <TierButton tier=tier price=fields.price/>
                                </For>
                            </div>
                            <div class="flex items-end gap-2">
                                <div class="w-32"><Cell label=label(|l| td_string!(l, pricing.your_price))><NumInput value=fields.price unit="¥"/></Cell></div>
                                <Show when=has_price>
                                    <Button small=true variant=ButtonVariant::Ghost on_click=move || fields.price.set(0.0)>{move || t_string!(i18n, pricing.clear_price)}</Button>
                                </Show>
                            </div>
                            {move || chosen.get().map(|c| view! { <PriceVerdict check=c band=band min_per_hour=Signal::derive(move || defaults.with(|d| d.min_profit_per_hour))/> })}
                            <PriceBand band=band tiers=tiers price=fields.price/>
                        </Card>

                        <Card class="p-4 space-y-1.5">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_capacity)/>
                            {move || {
                                let cap = capacity(&params.get());
                                if cap.units_per_day <= 0.0 {
                                    return view! { <p class="text-xs text-gray-400">{move || t_string!(i18n, pricing.capacity_unknown)}</p> }.into_any();
                                }
                                let daily = chosen.get().map(|c| c.profit * cap.units_per_day.floor());
                                view! {
                                    <p class="text-xs leading-relaxed text-gray-600 dark:text-gray-300">
                                        {move || t_string!(i18n, pricing.capacity_line, per_day = format!("{:.1}", cap.units_per_day), in_48h = cap.units_in_48h).to_string()}
                                    </p>
                                    {daily.filter(|d| *d > 0.0).map(|d| view! {
                                        <p class="text-xs leading-relaxed text-gray-600 dark:text-gray-300">
                                            {move || t_string!(i18n, pricing.capacity_profit, yuan = format!("{d:.0}")).to_string()}
                                        </p>
                                    })}
                                }.into_any()
                            }}
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, pricing.sec_runs) hint=move || t_string!(i18n, pricing.sec_runs_hint)/>
                            <div class="space-y-2">
                                <div class="flex items-center gap-2">
                                    <Segmented
                                        value=Signal::derive(move || run_success.get())
                                        options=vec![(true, label(|l| td_string!(l, pricing.run_success))), (false, label(|l| td_string!(l, pricing.run_failed)))]
                                        on_change=move |ok: bool| run_success.set(ok)
                                    />
                                    <div class="w-24"><NumInput value=run_grams decimals=1 unit="g"/></div>
                                    <div class="w-24"><NumInput value=run_hours unit="h"/></div>
                                </div>
                                <Show when=move || !run_success.get()>
                                    <TextInput value=run_reason placeholder=move || t_string!(i18n, pricing.run_reason_placeholder)/>
                                </Show>
                                <div class="flex items-center gap-2">
                                    <div class="flex-1"><TextInput value=run_note placeholder=move || t_string!(i18n, pricing.run_note_placeholder) on_enter=add_run/></div>
                                    <Button small=true icon=IconKind::Plus disabled=Signal::derive(move || !can_add_run.get()) on_click=add_run>
                                        {move || t_string!(i18n, pricing.run_add)}
                                    </Button>
                                </div>
                            </div>
                            <Show when=move || runs.with(Vec::is_empty)>
                                <p class="text-xs text-gray-400">{move || t_string!(i18n, pricing.no_runs)}</p>
                            </Show>
                            <div class="space-y-1.5">
                                <For each=move || runs.get() key=|r| r.id.clone() let:run>
                                    <RunRow run=run tz=tz on_delete=delete_run/>
                                </For>
                            </div>
                        </Card>
                    </aside>
                </Show>
            </div>

            <Dialog open=profiles_open title=move || t_string!(i18n, pricing.profiles)>
                <ProfilesPanel state=state printers=printers materials=materials defaults=defaults/>
            </Dialog>
        </div>
    }
}

#[component]
fn ProjectRow(state: AppState, project: Project, current: RwSignal<Option<String>>, #[prop(into)] on_open: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(project.id.clone());
    let active = move || current.with(|c| id.with_value(|id| c.as_deref() == Some(id.as_str())));
    let stage = project.stage;
    let priced = move || id.with_value(|id| state.facts_of(id).has_price);
    view! {
        <div
            class="px-2.5 py-2 rounded-lg cursor-pointer transition-colors"
            class=("bg-brand-soft", active)
            class=("dark:bg-indigo-500/15", active)
            class=("hover:bg-gray-100", move || !active())
            class=("dark:hover:bg-gray-700/60", move || !active())
            on:click=move |_| on_open.run((id.get_value(),))
            on:contextmenu=move |ev| {
                let l = current_locale();
                state.open_menu(
                    &ev,
                    vec![
                        item(td_string!(l, pricing.menu_open_model), IconKind::Ruler, move || on_open.run((id.get_value(),))),
                        item(td_string!(l, board.menu_open), IconKind::Kanban, move || state.open_project.set(Some(id.get_value()))),
                    ],
                );
            }
        >
            <div class="flex items-center justify-between gap-2">
                <span class="text-[11px] font-mono text-gray-400">{project.code.clone()}</span>
                <Show when=priced>
                    <span class="text-[11px] text-green-600 dark:text-green-400">{move || t_string!(i18n, pricing.priced)}</span>
                </Show>
            </div>
            <div class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{project.title.clone()}</div>
            <div class="text-[11px] text-gray-400">{move || stage_name(i18n.get_locale(), stage)}</div>
        </div>
    }
}

#[component]
fn TierButton(tier: PriceCheck, price: RwSignal<f64>) -> impl IntoView {
    let i18n = use_i18n();
    let active = move || (price.get() - tier.price).abs() < 1e-9;
    view! {
        <button
            type="button"
            class="px-2 py-2 rounded-lg border text-center transition-colors"
            class=("border-brand", active)
            class=("bg-brand-soft", active)
            class=("dark:bg-indigo-500/15", active)
            class=("border-gray-200", move || !active())
            class=("dark:border-gray-700", move || !active())
            class=("hover:border-brand", move || !active())
            on:click=move |_| price.set(tier.price)
        >
            <div class="text-sm font-semibold tabular-nums text-gray-900 dark:text-gray-50">{format!("¥{}", format_price(tier.price))}</div>
            <div class="text-[11px] tabular-nums text-gray-500 dark:text-gray-400">
                {move || t_string!(i18n, pricing.tier_margin, margin = percent(tier.margin)).to_string()}
            </div>
            <div class="text-[11px] tabular-nums text-gray-400">{format!("¥{:.1}/h", tier.profit_per_hour)}</div>
        </button>
    }
}

fn format_price(p: f64) -> String {
    if (p - p.round()).abs() < 1e-9 {
        format!("{p:.0}")
    } else {
        format!("{p:.1}")
    }
}

/// 这个价下的账,和三条提醒:毛利率不到 50%、每机时毛利不达标、相对竞品价格带的位置。
#[component]
fn PriceVerdict(check: PriceCheck, band: RwSignal<Option<(u32, u32)>>, #[prop(into)] min_per_hour: Signal<f64>) -> impl IntoView {
    let i18n = use_i18n();
    let loss = check.profit < 0.0;
    view! {
        <div class="rounded-lg bg-gray-50 dark:bg-gray-900/50 px-3 py-2 space-y-1.5">
            <div class="grid grid-cols-3 gap-2 text-center">
                <div>
                    <div class="text-sm font-semibold tabular-nums" class=("text-red-600", loss) class=("text-gray-900", !loss) class=("dark:text-gray-50", !loss)>{percent(check.margin)}</div>
                    <div class="text-[11px] text-gray-400">{move || t_string!(i18n, pricing.margin)}</div>
                </div>
                <div>
                    <div class="text-sm font-semibold tabular-nums text-gray-900 dark:text-gray-50">{yuan(check.profit)}</div>
                    <div class="text-[11px] text-gray-400">{move || t_string!(i18n, pricing.profit_per_unit)}</div>
                </div>
                <div>
                    <div class="text-sm font-semibold tabular-nums text-gray-900 dark:text-gray-50">{format!("¥{:.1}", check.profit_per_hour)}</div>
                    <div class="text-[11px] text-gray-400">{move || t_string!(i18n, pricing.profit_per_hour)}</div>
                </div>
            </div>
            {move || {
                let mut notes: Vec<String> = Vec::new();
                if check.profit < 0.0 {
                    notes.push(t_string!(i18n, pricing.warn_loss).to_string());
                } else if check.margin < 0.5 {
                    notes.push(t_string!(i18n, pricing.warn_margin).to_string());
                }
                let floor = min_per_hour.get();
                if check.profit >= 0.0 && check.profit_per_hour < floor {
                    notes.push(t_string!(i18n, pricing.warn_per_hour, floor = format!("{floor:.0}")).to_string());
                }
                if let Some((lo, hi)) = band.get() {
                    if check.price > hi as f64 {
                        notes.push(t_string!(i18n, pricing.warn_above_band, hi = hi).to_string());
                    } else if check.price < lo as f64 {
                        notes.push(t_string!(i18n, pricing.warn_below_band, lo = lo).to_string());
                    }
                }
                notes.into_iter().map(|n| view! {
                    <p class="flex items-start gap-1.5 text-[11px] leading-relaxed text-amber-700 dark:text-amber-300">
                        <Icon kind=IconKind::Alert class="w-3 h-3 mt-0.5 shrink-0"/>
                        {n}
                    </p>
                }).collect_view()
            }}
        </div>
    }
}

/// 竞品价格带(来自调研)和「你的位置」。
#[component]
fn PriceBand(band: RwSignal<Option<(u32, u32)>>, tiers: Memo<Vec<PriceCheck>>, price: RwSignal<f64>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || match band.get() {
            None => view! { <p class="text-[11px] leading-relaxed text-gray-400">{move || t_string!(i18n, pricing.no_band)}</p> }.into_any(),
            Some((lo, hi)) => {
                // 轴:把价格带、三档价和你定的价都放得下
                let top = tiers.with(|t| t.iter().map(|t| t.price).fold(hi as f64, f64::max)).max(price.get()) * 1.1;
                let pos = move |v: f64| format!("{:.1}%", (v / top * 100.0).clamp(0.0, 100.0));
                let mine = price.get();
                view! {
                    <div class="space-y-1">
                        <div class="relative h-5">
                            <div class="absolute inset-x-0 top-2 h-1 rounded-full bg-gray-200 dark:bg-gray-700"></div>
                            <div
                                class="absolute top-1.5 h-2 rounded-full bg-brand/40"
                                style:left=pos(lo as f64)
                                style:width=format!("{:.1}%", ((hi - lo) as f64 / top * 100.0).clamp(0.0, 100.0))
                            ></div>
                            {tiers.get().into_iter().map(|t| view! {
                                <div class="absolute top-1 w-0.5 h-3 bg-gray-400" style:left=pos(t.price)></div>
                            }).collect_view()}
                            {(mine > 0.0).then(|| view! {
                                <div class="absolute top-0 -ml-1 w-2 h-5 rounded-sm bg-brand" style:left=pos(mine)></div>
                            })}
                        </div>
                        <p class="text-[11px] text-gray-500 dark:text-gray-400 tabular-nums">
                            {move || t_string!(i18n, pricing.band_line, lo = lo, hi = hi).to_string()}
                        </p>
                    </div>
                }.into_any()
            }
        }}
    }
}

#[component]
fn RunRow(run: PrintRun, tz: i32, #[prop(into)] on_delete: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<AppState>();
    let id = StoredValue::new(run.id.clone());
    // 失败原因 / 备注:改设计时要抄走的就是这两句
    let words = StoredValue::new([run.fail_reason.trim(), run.note.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"));
    // 组件体里不是响应式上下文:文案按当前语言取一次就够了(这一行不会跟着切语言变,列表重建时会)
    let est = td_string!(crate::i18n_util::current_locale(), pricing.est);
    let pair = |actual: Option<f64>, est_value: Option<f64>, unit: &str, decimals: usize| match (actual, est_value) {
        (Some(a), Some(e)) => format!("{a:.decimals$} {unit} ({est} {e:.decimals$})"),
        (Some(a), None) => format!("{a:.decimals$} {unit}"),
        _ => String::new(),
    };
    let numbers = [pair(run.actual_grams, run.est_grams, "g", 1), pair(run.actual_hours, run.est_hours, "h", 2)]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let success = run.success;
    view! {
        <div
            class="group flex items-start gap-2 px-2.5 py-2 rounded-lg border border-gray-200 dark:border-gray-700"
            on:contextmenu=move |ev| {
                let l = crate::i18n_util::current_locale();
                let mut entries = crate::ui::basics(state, &ev);
                if !words.with_value(String::is_empty) {
                    entries.push(copy_entry(state, td_string!(l, pricing.menu_copy_note), words.get_value()));
                    entries.push(separator());
                }
                entries.push(item(td_string!(l, pricing.menu_delete_run), IconKind::Trash, move || on_delete.run((id.get_value(),))).danger());
                state.open_menu(&ev, entries);
            }
        >
            <div class="pt-0.5">
                {if success {
                    view! { <Badge tone=Tone::Green>{move || t_string!(i18n, pricing.run_success)}</Badge> }.into_any()
                } else {
                    view! { <Badge tone=Tone::Red>{move || t_string!(i18n, pricing.run_failed)}</Badge> }.into_any()
                }}
            </div>
            <div class="min-w-0 flex-1 space-y-0.5">
                <div class="text-xs tabular-nums text-gray-800 dark:text-gray-100">{if numbers.is_empty() { "—".to_string() } else { numbers }}</div>
                {(!run.fail_reason.is_empty()).then(|| view! { <p class="text-[11px] text-red-600 dark:text-red-400 break-words selectable">{run.fail_reason.clone()}</p> })}
                {(!run.note.is_empty()).then(|| view! { <p class="text-[11px] text-gray-500 dark:text-gray-400 break-words selectable">{run.note.clone()}</p> })}
                <div class="text-[11px] tabular-nums text-gray-400">{format_ts(run.created_at, tz)}</div>
            </div>
            <div class="opacity-0 group-hover:opacity-100 transition-opacity">
                <IconButton icon=IconKind::Trash label=move || t_string!(i18n, common.delete) on_click=move || on_delete.run((id.get_value(),))/>
            </div>
        </div>
    }
}
