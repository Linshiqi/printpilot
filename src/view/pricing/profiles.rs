//! 档案与默认值(定价页右上角打开):打印机、耗材、这个资料库的成本默认值。
//! 成本模型里选一份档案 = 把它的数带进去(之后还可以在那个项目里单独改);新项目的成本模型从默认值起步。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::cost::{fee_preset, CostDefaults, Material, Printer, FEE_PRESETS};

use super::fee_name;
use crate::i18n::{use_i18n, Locale};
use crate::i18n_util::current_locale;
use crate::icon::IconKind;
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::ui::{Button, ButtonVariant, IconButton, NumInput, SectionTitle, TextInput};

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
pub fn ProfilesPanel(
    state: AppState,
    printers: RwSignal<Vec<Printer>>,
    materials: RwSignal<Vec<Material>>,
    defaults: RwSignal<CostDefaults>,
) -> impl IntoView {
    let i18n = use_i18n();
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());

    // ---- 打印机表单(id 为空 = 新建)----
    let blank = Printer::default();
    let p_id = RwSignal::new(String::new());
    let p_name = RwSignal::new(String::new());
    let p_power = RwSignal::new(blank.power_w);
    let p_price = RwSignal::new(blank.price_yuan);
    let p_life = RwSignal::new(blank.lifetime_hours);
    let p_speed = RwSignal::new(blank.grams_per_hour);
    let p_build = RwSignal::new(blank.build_mm[0]);
    let edit_printer = move |p: Printer| {
        p_id.set(p.id);
        p_name.set(p.name);
        p_power.set(p.power_w);
        p_price.set(p.price_yuan);
        p_life.set(p.lifetime_hours);
        p_speed.set(p.grams_per_hour);
        p_build.set(p.build_mm[0]);
    };
    let save_printer = move || {
        let side = p_build.get_untracked();
        let printer = Printer {
            id: p_id.get_untracked(),
            name: p_name.get_untracked(),
            model: String::new(),
            build_mm: [side, side, side],
            power_w: p_power.get_untracked(),
            price_yuan: p_price.get_untracked(),
            lifetime_hours: p_life.get_untracked(),
            grams_per_hour: p_speed.get_untracked(),
            created_at: 0,
        };
        spawn_local(async move {
            match ipc::call::<_, Printer>(cmd::PRINTER_SAVE, &serde_json::json!({ "printer": printer })).await {
                Ok(saved) => {
                    printers.update(|l| match l.iter_mut().find(|x| x.id == saved.id) {
                        Some(slot) => *slot = saved,
                        None => l.push(saved),
                    });
                    edit_printer(Printer::default());
                    state.notify_info(td_string!(current_locale(), project.saved));
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let delete_printer = move |id: String| {
        spawn_local(async move {
            match ipc::call_unit(cmd::PRINTER_DELETE, &serde_json::json!({ "id": id })).await {
                Ok(()) => printers.update(|l| l.retain(|p| p.id != id)),
                Err(e) => state.notify_error(e),
            }
        });
    };

    // ---- 耗材表单 ----
    let blank_m = Material::default();
    let m_id = RwSignal::new(String::new());
    let m_name = RwSignal::new(String::new());
    let m_kind = RwSignal::new(blank_m.kind.clone());
    let m_price = RwSignal::new(blank_m.yuan_per_kg);
    let m_density = RwSignal::new(blank_m.density);
    let edit_material = move |m: Material| {
        m_id.set(m.id);
        m_name.set(m.name);
        m_kind.set(m.kind);
        m_price.set(m.yuan_per_kg);
        m_density.set(m.density);
    };
    let save_material = move || {
        let material = Material {
            id: m_id.get_untracked(),
            name: m_name.get_untracked(),
            kind: m_kind.get_untracked(),
            color: String::new(),
            yuan_per_kg: m_price.get_untracked(),
            density: m_density.get_untracked(),
            stock_g: 0.0,
            created_at: 0,
        };
        spawn_local(async move {
            match ipc::call::<_, Material>(cmd::MATERIAL_SAVE, &serde_json::json!({ "material": material })).await {
                Ok(saved) => {
                    materials.update(|l| match l.iter_mut().find(|x| x.id == saved.id) {
                        Some(slot) => *slot = saved,
                        None => l.push(saved),
                    });
                    edit_material(Material::default());
                    state.notify_info(td_string!(current_locale(), project.saved));
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let delete_material = move |id: String| {
        spawn_local(async move {
            match ipc::call_unit(cmd::MATERIAL_DELETE, &serde_json::json!({ "id": id })).await {
                Ok(()) => materials.update(|l| l.retain(|m| m.id != id)),
                Err(e) => state.notify_error(e),
            }
        });
    };

    // ---- 成本默认值 ----
    let d0 = defaults.get_untracked();
    let d_elec = RwSignal::new(d0.electricity_yuan_per_kwh);
    let d_labor_min = RwSignal::new(d0.labor_minutes);
    let d_labor_rate = RwSignal::new(d0.labor_yuan_per_min);
    let d_pack = RwSignal::new(d0.packaging_yuan);
    let d_ship = RwSignal::new(d0.shipping_subsidy_yuan);
    let d_waste = RwSignal::new(d0.waste_rate);
    let d_success = RwSignal::new(d0.success_rate);
    let d_channel = RwSignal::new(d0.channel.clone());
    let d_fee = RwSignal::new(d0.fee_rate);
    let d_floor = RwSignal::new(d0.min_profit_per_hour);
    let d_m1 = RwSignal::new(d0.target_margins[0]);
    let d_m2 = RwSignal::new(d0.target_margins[1]);
    let d_m3 = RwSignal::new(d0.target_margins[2]);
    let save_defaults = move || {
        let next = CostDefaults {
            waste_rate: d_waste.get_untracked(),
            success_rate: d_success.get_untracked(),
            electricity_yuan_per_kwh: d_elec.get_untracked(),
            labor_minutes: d_labor_min.get_untracked(),
            labor_yuan_per_min: d_labor_rate.get_untracked(),
            packaging_yuan: d_pack.get_untracked(),
            shipping_subsidy_yuan: d_ship.get_untracked(),
            channel: d_channel.get_untracked(),
            fee_rate: d_fee.get_untracked(),
            target_margins: [d_m1.get_untracked(), d_m2.get_untracked(), d_m3.get_untracked()],
            min_profit_per_hour: d_floor.get_untracked(),
            ..defaults.get_untracked()
        };
        spawn_local(async move {
            match ipc::call_unit(cmd::COST_DEFAULTS_SET, &serde_json::json!({ "defaults": next })).await {
                Ok(()) => {
                    defaults.set(next);
                    state.notify_info(td_string!(current_locale(), project.saved));
                }
                Err(e) => state.notify_error(e),
            }
        });
    };

    view! {
        <div class="space-y-5 max-h-[70vh] overflow-y-auto pr-1">
            // ---- 打印机 ----
            <section class="space-y-2">
                <SectionTitle title=move || t_string!(i18n, pricing.printers_title) hint=move || t_string!(i18n, pricing.printers_hint)/>
                <For each=move || printers.get() key=|p| serde_json::to_string(p).unwrap_or_default() let:p>
                    {
                        let (for_edit, id) = (p.clone(), p.id.clone());
                        view! {
                            <div class="flex items-center gap-2 px-2.5 py-1.5 rounded-lg border border-gray-200 dark:border-gray-700">
                                <div class="min-w-0 flex-1">
                                    <div class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{p.name.clone()}</div>
                                    <div class="text-[11px] tabular-nums text-gray-400">
                                        {format!("{:.0} W · ¥{:.0} · {:.0} h · {:.0} g/h", p.power_w, p.price_yuan, p.lifetime_hours, p.grams_per_hour)}
                                    </div>
                                </div>
                                <Button small=true variant=ButtonVariant::Ghost on_click=move || edit_printer(for_edit.clone())>{move || t_string!(i18n, pricing.edit)}</Button>
                                <IconButton icon=IconKind::Trash label=move || t_string!(i18n, common.delete) on_click=move || delete_printer(id.clone())/>
                            </div>
                        }
                    }
                </For>
                <div class="grid grid-cols-3 gap-2">
                    <div class="col-span-3"><TextInput value=p_name placeholder=move || t_string!(i18n, pricing.printer_name_placeholder)/></div>
                    <Cell label=label(|l| td_string!(l, pricing.power_w))><NumInput value=p_power decimals=0 unit="W"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.printer_price))><NumInput value=p_price decimals=0 unit="¥"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.printer_lifetime))><NumInput value=p_life decimals=0 unit="h"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.grams_per_hour))><NumInput value=p_speed decimals=1 unit="g/h"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.build_side))><NumInput value=p_build decimals=0 unit="mm"/></Cell>
                </div>
                <div class="flex justify-end gap-2">
                    <Show when=move || !p_id.with(String::is_empty)>
                        <Button small=true variant=ButtonVariant::Ghost on_click=move || edit_printer(Printer::default())>{move || t_string!(i18n, common.cancel)}</Button>
                    </Show>
                    <Button small=true disabled=Signal::derive(move || p_name.with(|n| n.trim().is_empty())) on_click=save_printer>
                        {move || if p_id.with(String::is_empty) { t_string!(i18n, pricing.add_printer) } else { t_string!(i18n, common.save) }}
                    </Button>
                </div>
            </section>

            // ---- 耗材 ----
            <section class="space-y-2">
                <SectionTitle title=move || t_string!(i18n, pricing.materials_title)/>
                <For each=move || materials.get() key=|m| serde_json::to_string(m).unwrap_or_default() let:m>
                    {
                        let (for_edit, id) = (m.clone(), m.id.clone());
                        view! {
                            <div class="flex items-center gap-2 px-2.5 py-1.5 rounded-lg border border-gray-200 dark:border-gray-700">
                                <div class="min-w-0 flex-1">
                                    <div class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{format!("{} · {}", m.name, m.kind)}</div>
                                    <div class="text-[11px] tabular-nums text-gray-400">{format!("¥{:.1}/kg · {:.2} g/cm³", m.yuan_per_kg, m.density)}</div>
                                </div>
                                <Button small=true variant=ButtonVariant::Ghost on_click=move || edit_material(for_edit.clone())>{move || t_string!(i18n, pricing.edit)}</Button>
                                <IconButton icon=IconKind::Trash label=move || t_string!(i18n, common.delete) on_click=move || delete_material(id.clone())/>
                            </div>
                        }
                    }
                </For>
                <div class="grid grid-cols-3 gap-2">
                    <div class="col-span-2"><TextInput value=m_name placeholder=move || t_string!(i18n, pricing.material_name_placeholder)/></div>
                    <TextInput value=m_kind placeholder="PLA"/>
                    <Cell label=label(|l| td_string!(l, pricing.material_price))><NumInput value=m_price unit="¥"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.density))><NumInput value=m_density unit="g/cm³"/></Cell>
                </div>
                <div class="flex justify-end gap-2">
                    <Show when=move || !m_id.with(String::is_empty)>
                        <Button small=true variant=ButtonVariant::Ghost on_click=move || edit_material(Material::default())>{move || t_string!(i18n, common.cancel)}</Button>
                    </Show>
                    <Button small=true disabled=Signal::derive(move || m_name.with(|n| n.trim().is_empty())) on_click=save_material>
                        {move || if m_id.with(String::is_empty) { t_string!(i18n, pricing.add_material) } else { t_string!(i18n, common.save) }}
                    </Button>
                </div>
            </section>

            // ---- 成本默认值 ----
            <section class="space-y-2">
                <SectionTitle title=move || t_string!(i18n, pricing.defaults_title) hint=move || t_string!(i18n, pricing.defaults_hint)/>
                <div class="grid grid-cols-3 gap-2">
                    <Cell label=label(|l| td_string!(l, pricing.electricity))><NumInput value=d_elec unit="¥"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.labor_minutes))><NumInput value=d_labor_min decimals=1 unit="min"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.labor_rate))><NumInput value=d_labor_rate unit="¥"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.packaging))><NumInput value=d_pack unit="¥"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.shipping))><NumInput value=d_ship unit="¥"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.waste_rate))><NumInput value=d_waste scale=100.0 unit="%"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.success_rate))><NumInput value=d_success scale=100.0 unit="%"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.channel))>
                        <select
                            class="w-full h-8 px-2 rounded-lg border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 text-sm text-gray-900 dark:text-gray-100"
                            on:change=move |e| {
                                let key = event_target_value(&e);
                                if let Some(p) = fee_preset(&key).filter(|p| p.key != "custom") {
                                    d_fee.set(p.rate);
                                }
                                d_channel.set(key);
                            }
                        >
                            {FEE_PRESETS.iter().map(|preset| {
                                let key = preset.key;
                                view! { <option value=key selected=move || d_channel.with(|c| c == key)>{move || fee_name(i18n.get_locale(), key)}</option> }
                            }).collect_view()}
                        </select>
                    </Cell>
                    <Cell label=label(|l| td_string!(l, pricing.fee_rate))><NumInput value=d_fee scale=100.0 unit="%"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.margin_tier_1))><NumInput value=d_m1 scale=100.0 decimals=0 unit="%"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.margin_tier_2))><NumInput value=d_m2 scale=100.0 decimals=0 unit="%"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.margin_tier_3))><NumInput value=d_m3 scale=100.0 decimals=0 unit="%"/></Cell>
                    <Cell label=label(|l| td_string!(l, pricing.min_per_hour))><NumInput value=d_floor decimals=1 unit="¥"/></Cell>
                </div>
                <div class="flex justify-end">
                    <Button small=true on_click=save_defaults>{move || t_string!(i18n, common.save)}</Button>
                </div>
            </section>
        </div>
    }
}
