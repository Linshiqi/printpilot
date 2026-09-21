use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::imagery::ImageSettings;
use pp_common::provider::ProviderId;
use pp_common::AppInfo;

use crate::i18n_util::current_locale;
use crate::i18n::{use_i18n, Locale};
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::{AppState, SettingsSection};
use crate::theme::{set_pref, Theme};
use crate::ui::{Badge, Button, ButtonVariant, Card, Field, SectionTitle, Segmented, TextInput, Toggle, Tone};

#[component]
fn Row(#[prop(into)] label: Signal<String>, children: Children) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-6 py-2.5">
            <span class="text-sm text-gray-600 dark:text-gray-300 shrink-0">{move || label.get()}</span>
            <div class="min-w-0 flex items-center gap-2 text-sm text-gray-900 dark:text-gray-100">{children()}</div>
        </div>
    }
}

/// 一个供应商的密钥行:状态、输入、保存 / 测试连接 / 删除 / 去获取。
/// 密钥只从这个输入框去往后端的系统凭据管理器,保存后立刻清空输入框;前端任何地方都读不回它。
#[component]
fn KeyRow(state: AppState, id: ProviderId, #[prop(into)] title: Signal<String>, #[prop(into)] hint: Signal<String>) -> impl IntoView {
    let i18n = use_i18n();
    let input = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let has_key = move || state.providers.with(|list| list.iter().any(|p| p.id == id && p.has_key));

    let save = move || {
        let key = input.get_untracked().trim().to_string();
        if key.is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match ipc::call_unit(cmd::SAVE_PROVIDER_KEY, &serde_json::json!({ "id": id, "key": key })).await {
                Ok(()) => {
                    input.set(String::new());
                    state.notify_info(td_string!(current_locale(), settings.key_saved));
                    state.reload_providers();
                }
                Err(e) => state.notify_error(e),
            }
            busy.set(false);
        });
    };
    let test = move || {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match ipc::call::<_, u64>(cmd::TEST_PROVIDER, &serde_json::json!({ "id": id })).await {
                Ok(ms) => state.notify_info(td_string!(current_locale(), settings.key_test_ok, ms = ms).to_string()),
                Err(e) => state.notify_error(e),
            }
            busy.set(false);
        });
    };
    let remove = move || {
        spawn_local(async move {
            match ipc::call_unit(cmd::DELETE_PROVIDER_KEY, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    state.notify_info(td_string!(current_locale(), settings.key_deleted));
                    state.reload_providers();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let open_console = move || {
        spawn_local(async move {
            let _ = ipc::call_unit(cmd::OPEN_URL, &serde_json::json!({ "url": id.console_url() })).await;
        });
    };

    view! {
        <div class="py-3 space-y-2">
            <div class="flex items-start justify-between gap-4">
                <div class="min-w-0 space-y-0.5">
                    <div class="flex items-center gap-2">
                        <span class="text-sm font-medium text-gray-800 dark:text-gray-100">{move || title.get()}</span>
                        {move || {
                            let (tone, text) = if has_key() {
                                (Tone::Green, t_string!(i18n, settings.key_configured))
                            } else {
                                (Tone::Amber, t_string!(i18n, settings.key_missing))
                            };
                            view! { <Badge tone=tone>{text}</Badge> }
                        }}
                    </div>
                    <p class="text-xs leading-relaxed text-gray-500 dark:text-gray-400">{move || hint.get()}</p>
                </div>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::External on_click=open_console>
                    {move || t_string!(i18n, settings.key_get)}
                </Button>
            </div>
            <div class="flex items-center gap-2">
                <TextInput value=input password=true placeholder=move || t_string!(i18n, settings.key_placeholder) on_enter=save/>
                <Button small=true disabled=Signal::derive(move || busy.get() || input.with(|s| s.trim().is_empty())) on_click=save>
                    {move || t_string!(i18n, common.save)}
                </Button>
                <Button small=true variant=ButtonVariant::Secondary disabled=Signal::derive(move || busy.get() || !has_key()) on_click=test>
                    {move || t_string!(i18n, settings.key_test)}
                </Button>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Trash disabled=Signal::derive(move || busy.get() || !has_key()) on_click=remove>
                    {move || t_string!(i18n, common.delete)}
                </Button>
            </div>
        </div>
    }
}

/// 出图设置:生成时优先用哪家、两家的接入地址(百炼给的地址可能带工作空间,所以必须能改)。
/// 密钥不在这里——密钥在上面的「接口密钥」里。
#[component]
fn ImageSettingsCard(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let provider = RwSignal::new("auto");
    let minimax_url = RwSignal::new(String::new());
    let qwen_url = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());

    let apply = move |s: ImageSettings| {
        provider.set(match s.provider.as_str() {
            "minimax" => "minimax",
            "qwen" => "qwen",
            _ => "auto",
        });
        minimax_url.set(s.minimax_base_url);
        qwen_url.set(s.qwen_base_url);
    };
    spawn_local(async move {
        match ipc::call_no_args::<ImageSettings>(cmd::IMAGE_SETTINGS_GET).await {
            Ok(s) => apply(s),
            Err(e) => state.notify_error(e),
        }
    });
    let save = move || {
        if busy.get_untracked() {
            return;
        }
        let settings = ImageSettings {
            provider: provider.get_untracked().to_string(),
            minimax_base_url: minimax_url.get_untracked(),
            qwen_base_url: qwen_url.get_untracked(),
        };
        busy.set(true);
        spawn_local(async move {
            match ipc::call_unit(cmd::IMAGE_SETTINGS_SET, &serde_json::json!({ "settings": settings })).await {
                Ok(()) => {
                    // 后端会规整地址(去掉末尾的 /,空 = 出厂默认),读回来显示真正生效的值
                    if let Ok(s) = ipc::call_no_args::<ImageSettings>(cmd::IMAGE_SETTINGS_GET).await {
                        apply(s);
                    }
                    state.notify_info(td_string!(current_locale(), settings.image_saved));
                }
                Err(e) => state.notify_error(e),
            }
            busy.set(false);
        });
    };

    view! {
        <Card class="p-5">
            <SectionTitle title=move || t_string!(i18n, settings.image) hint=move || t_string!(i18n, settings.image_hint)/>
            <div class="divide-y divide-gray-100 dark:divide-gray-700 pt-1">
                <Row label=label(|l| td_string!(l, settings.image_provider))>
                    <Segmented
                        value=Signal::derive(move || provider.get())
                        options=vec![
                            ("auto", label(|l| td_string!(l, settings.image_provider_auto))),
                            ("minimax", label(|l| td_string!(l, settings.image_provider_minimax))),
                            ("qwen", label(|l| td_string!(l, settings.image_provider_qwen))),
                        ]
                        on_change=move |p: &'static str| {
                            provider.set(p);
                            save();
                        }
                    />
                </Row>
                <div class="py-3 space-y-3">
                    <Field label=move || t_string!(i18n, settings.image_minimax_url) hint=move || t_string!(i18n, settings.image_minimax_url_hint)>
                        <TextInput value=minimax_url on_enter=save/>
                    </Field>
                    <Field label=move || t_string!(i18n, settings.image_qwen_url) hint=move || t_string!(i18n, settings.image_qwen_url_hint)>
                        <TextInput value=qwen_url on_enter=save/>
                    </Field>
                    <div class="flex justify-end">
                        <Button small=true disabled=Signal::derive(move || busy.get()) on_click=save>{move || t_string!(i18n, common.save)}</Button>
                    </div>
                </div>
            </div>
        </Card>
    }
}

/// 软件更新:查、下、装。下载的是不带建模引擎的精简包(十几 MB),引擎不动。
#[component]
fn UpdateCard(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let checking = RwSignal::new(false);
    let installing = RwSignal::new(false);
    // None = 还没查过;Some(true) = 查过了,已经是最新
    let up_to_date = RwSignal::new(None::<bool>);

    let check = move || {
        if checking.get_untracked() {
            return;
        }
        checking.set(true);
        spawn_local(async move {
            #[derive(serde::Deserialize)]
            struct Found {
                version: String,
                #[serde(default)]
                notes: String,
            }
            let result = ipc::call_no_args::<Option<Found>>(cmd::UPDATE_CHECK).await;
            checking.set(false);
            match result {
                Ok(Some(found)) => {
                    up_to_date.set(Some(false));
                    state.update_available.set(Some((found.version, found.notes)));
                }
                Ok(None) => {
                    up_to_date.set(Some(true));
                    state.update_available.set(None);
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let install = move || {
        if installing.get_untracked() {
            return;
        }
        installing.set(true);
        state.update_progress.set(None);
        spawn_local(async move {
            // 成功的话应用会被安装程序关掉 / 自己重启,走不到下面
            if let Err(e) = ipc::call_unit_no_args(cmd::UPDATE_INSTALL).await {
                installing.set(false);
                state.notify_error(e);
            }
        });
    };
    let percent = move || match state.update_progress.get() {
        Some((done, total)) if total > 0 => done as f64 / total as f64 * 100.0,
        _ => 0.0,
    };

    view! {
        <Card class="p-5 space-y-3">
            <SectionTitle title=move || t_string!(i18n, settings.update) hint=move || t_string!(i18n, settings.update_hint)/>
            <div class="flex items-center gap-3">
                <div class="flex-1 min-w-0 text-xs text-gray-600 dark:text-gray-300">
                    {move || match (state.update_available.get(), up_to_date.get()) {
                        (Some((version, _)), _) => t_string!(i18n, settings.update_found, version = version).to_string(),
                        (None, Some(true)) => t_string!(i18n, settings.update_none).to_string(),
                        _ => String::new(),
                    }}
                </div>
                <Show
                    when=move || state.update_available.with(Option::is_some)
                    fallback=move || view! {
                        <Button small=true variant=ButtonVariant::Secondary disabled=Signal::derive(move || checking.get()) on_click=check>
                            {move || if checking.get() { t_string!(i18n, settings.update_checking) } else { t_string!(i18n, settings.update_check) }}
                        </Button>
                    }
                >
                    <Button small=true disabled=Signal::derive(move || installing.get()) on_click=install>
                        {move || if installing.get() { t_string!(i18n, settings.update_installing) } else { t_string!(i18n, settings.update_install) }}
                    </Button>
                </Show>
            </div>
            <Show when=move || installing.get()>
                <div class="h-1.5 rounded-full bg-gray-200 dark:bg-gray-700 overflow-hidden">
                    <div class="h-full bg-brand transition-all" style:width=move || format!("{:.1}%", percent())></div>
                </div>
            </Show>
            {move || state.update_available.get().map(|(_, notes)| notes).filter(|n| !n.trim().is_empty()).map(|notes| view! {
                <pre class="max-h-48 overflow-y-auto whitespace-pre-wrap break-words text-[11px] leading-relaxed text-gray-500 dark:text-gray-400 selectable">{notes}</pre>
            })}
        </Card>
    }
}

/// 二级菜单里的一项。右边可以带一点状态:密钥配了几个、有没有新版本。
#[component]
fn SectionItem(state: AppState, section: SettingsSection) -> impl IntoView {
    let i18n = use_i18n();
    let active = move || state.settings_section.get() == section;
    let icon = match section {
        SettingsSection::General => IconKind::Settings,
        SettingsSection::Keys => IconKind::Key,
        SettingsSection::Images => IconKind::Image,
        SettingsSection::Library => IconKind::Folder,
        SettingsSection::About => IconKind::Info,
    };
    let label = move || match section {
        SettingsSection::General => t_string!(i18n, settings.nav_general),
        SettingsSection::Keys => t_string!(i18n, settings.keys),
        SettingsSection::Images => t_string!(i18n, settings.image),
        SettingsSection::Library => t_string!(i18n, settings.library),
        SettingsSection::About => t_string!(i18n, settings.nav_about),
    };
    view! {
        <button
            type="button"
            class="w-full flex items-center gap-2.5 h-9 px-3 rounded-lg text-sm transition-colors"
            class=("bg-brand-soft", active)
            class=("text-brand", active)
            class=("font-medium", active)
            class=("dark:bg-indigo-500/15", active)
            class=("dark:text-indigo-300", active)
            class=("text-gray-600", move || !active())
            class=("dark:text-gray-300", move || !active())
            class=("hover:bg-gray-100", move || !active())
            class=("dark:hover:bg-gray-700/60", move || !active())
            on:click=move |_| {
                state.settings_section.set(section);
                set_pref("settings_section", section.as_str());
            }
        >
            <Icon kind=icon class="w-4 h-4 shrink-0"/>
            <span class="flex-1 min-w-0 truncate text-left">{label}</span>
            {move || match section {
                // 配了几个密钥:一眼看出还差哪一步
                SettingsSection::Keys => {
                    let (have, all) = state.providers.with(|l| (l.iter().filter(|p| p.has_key).count(), l.len()));
                    (all > 0).then(|| view! { <span class="text-[11px] tabular-nums text-gray-400">{format!("{have}/{all}")}</span> }.into_any())
                }
                SettingsSection::About => state
                    .update_available
                    .with(Option::is_some)
                    .then(|| view! { <span class="w-1.5 h-1.5 rounded-full bg-brand"></span> }.into_any()),
                _ => None,
            }}
        </button>
    }
}

#[component]
fn GeneralSection(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    let set_demo = move |enabled: bool| {
        spawn_local(async move {
            match ipc::call::<_, AppInfo>(cmd::SET_DEMO_MODE, &serde_json::json!({ "enabled": enabled })).await {
                Ok(info) => state.app_info.set(Some(info)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    view! {
        <Card class="p-5">
            <SectionTitle title=move || t_string!(i18n, settings.appearance)/>
            <div class="divide-y divide-gray-100 dark:divide-gray-700 pt-1">
                <Row label=label(|l| td_string!(l, settings.theme))>
                    <Segmented
                        value=Signal::derive(move || state.theme.get())
                        options=vec![
                            (Theme::Light, label(|l| td_string!(l, common.theme_light))),
                            (Theme::Dark, label(|l| td_string!(l, common.theme_dark))),
                        ]
                        on_change=move |t: Theme| state.theme.set(t)
                    />
                </Row>
                <Row label=label(|l| td_string!(l, settings.language))>
                    <Segmented
                        value=Signal::derive(move || i18n.get_locale())
                        options=vec![
                            (Locale::zh, Signal::stored("中文".to_string())),
                            (Locale::en, Signal::stored("English".to_string())),
                        ]
                        on_change=move |l: Locale| i18n.set_locale(l)
                    />
                </Row>
            </div>
        </Card>

        <Card class="p-5 space-y-2">
            <div class="flex items-start justify-between gap-6">
                <SectionTitle title=move || t_string!(i18n, settings.demo_mode) hint=move || t_string!(i18n, settings.demo_mode_hint)/>
                <Toggle
                    checked=Signal::derive(move || state.app_info.with(|i| i.as_ref().is_some_and(|i| i.demo_mode)))
                    disabled=Signal::derive(move || state.app_info.with(|i| !i.as_ref().is_some_and(|i| i.config_writable)))
                    on_change=set_demo
                />
            </div>
        </Card>
    }
}

#[component]
fn KeysSection(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    view! {
        <Card class="p-5">
            <SectionTitle title=move || t_string!(i18n, settings.keys) hint=move || t_string!(i18n, settings.keys_hint)/>
            <div class="divide-y divide-gray-100 dark:divide-gray-700 pt-1">
                <KeyRow
                    state=state
                    id=ProviderId::Deepseek
                    title=label(|l| td_string!(l, settings.key_deepseek))
                    hint=label(|l| td_string!(l, settings.key_deepseek_hint))
                />
                <KeyRow
                    state=state
                    id=ProviderId::ZhipuSearch
                    title=label(|l| td_string!(l, settings.key_zhipu_search))
                    hint=label(|l| td_string!(l, settings.key_zhipu_search_hint))
                />
                <KeyRow
                    state=state
                    id=ProviderId::Minimax
                    title=label(|l| td_string!(l, settings.key_minimax))
                    hint=label(|l| td_string!(l, settings.key_minimax_hint))
                />
                <KeyRow
                    state=state
                    id=ProviderId::QwenImage
                    title=label(|l| td_string!(l, settings.key_qwen_image))
                    hint=label(|l| td_string!(l, settings.key_qwen_image_hint))
                />
            </div>
        </Card>
    }
}

#[component]
fn LibrarySection(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <Card class="p-5 space-y-2">
            <SectionTitle title=move || t_string!(i18n, settings.library) hint=move || t_string!(i18n, settings.library_hint)/>
            <div class="flex items-center gap-3 pt-1">
                <code class="flex-1 min-w-0 truncate selectable rounded-md bg-gray-100 dark:bg-gray-900 px-2.5 py-1.5 text-xs text-gray-700 dark:text-gray-300">
                    {move || state.app_info.with(|i| i.as_ref().map(|i| i.library_dir.clone()).unwrap_or_default())}
                </code>
                <Button
                    small=true
                    variant=ButtonVariant::Secondary
                    icon=IconKind::Folder
                    on_click=move || spawn_local(async move {
                        if let Err(e) = ipc::call_unit_no_args(cmd::REVEAL_LIBRARY).await {
                            state.notify_error(e);
                        }
                    })
                >
                    {move || t_string!(i18n, settings.open_library)}
                </Button>
            </div>
        </Card>
    }
}

#[component]
fn AboutSection(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let info = move |pick: fn(&AppInfo) -> String| state.app_info.with(|i| i.as_ref().map(pick).unwrap_or_default());
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());
    view! {
        <UpdateCard state=state/>
        <Card class="p-5">
            <SectionTitle title=move || t_string!(i18n, settings.about)/>
            <div class="divide-y divide-gray-100 dark:divide-gray-700 pt-1">
                <Row label=label(|l| td_string!(l, settings.version))>
                    <span class="tabular-nums selectable">{move || info(|i| i.version.clone())}</span>
                </Row>
                <Row label=label(|l| td_string!(l, settings.schema))>
                    <span class="tabular-nums">{move || info(|i| format!("v{}", i.schema_version))}</span>
                </Row>
                <Row label=label(|l| td_string!(l, settings.projects))>
                    <span class="tabular-nums">{move || state.projects.with(Vec::len)}</span>
                </Row>
            </div>
        </Card>
    }
}

/// 设置页:左边一列二级菜单,右边只显示选中的那一组——不再是一长页往下滚。
#[component]
pub fn SettingsView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center border-b border-gray-200 dark:border-gray-700">
                <h1 class="text-base font-semibold">{move || t_string!(i18n, settings.title)}</h1>
            </header>
            <div class="flex-1 min-h-0 flex">
                <nav class="w-48 shrink-0 h-full overflow-y-auto p-2 space-y-0.5 border-r border-gray-200 dark:border-gray-700">
                    {SettingsSection::ALL.into_iter().map(|section| view! { <SectionItem state=state section=section/> }).collect_view()}
                </nav>
                <div class="flex-1 min-w-0 h-full overflow-y-auto">
                    <div class="max-w-2xl p-6 space-y-5">
                        <Show when=move || state.app_info.with(|i| i.as_ref().is_some_and(|i| !i.config_writable))>
                            <div class="rounded-lg border border-amber-300 bg-amber-50 dark:bg-amber-500/10 dark:border-amber-500/40 px-4 py-3 text-xs leading-relaxed text-amber-800 dark:text-amber-200">
                                {move || t_string!(i18n, settings.config_readonly)}
                            </div>
                        </Show>
                        {move || match state.settings_section.get() {
                            SettingsSection::General => view! { <GeneralSection state=state/> }.into_any(),
                            SettingsSection::Keys => view! { <KeysSection state=state/> }.into_any(),
                            SettingsSection::Images => view! { <ImageSettingsCard state=state/> }.into_any(),
                            SettingsSection::Library => view! { <LibrarySection state=state/> }.into_any(),
                            SettingsSection::About => view! { <AboutSection state=state/> }.into_any(),
                        }}
                    </div>
                </div>
            </div>
        </div>
    }
}
