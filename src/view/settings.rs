use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::provider::ProviderId;
use pp_common::AppInfo;

use crate::i18n_util::current_locale;
use crate::i18n::{use_i18n, Locale};
use crate::icon::IconKind;
use crate::ipc::{self, cmd};
use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::{Badge, Button, ButtonVariant, Card, SectionTitle, Segmented, TextInput, Toggle, Tone};

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

#[component]
pub fn SettingsView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let info = move |pick: fn(&AppInfo) -> String| state.app_info.with(|i| i.as_ref().map(pick).unwrap_or_default());
    let label = move |f: fn(crate::i18n::Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());

    let set_demo = move |enabled: bool| {
        spawn_local(async move {
            match ipc::call::<_, AppInfo>(cmd::SET_DEMO_MODE, &serde_json::json!({ "enabled": enabled })).await {
                Ok(info) => state.app_info.set(Some(info)),
                Err(e) => state.notify_error(e),
            }
        });
    };

    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 px-6 h-14 flex items-center border-b border-gray-200 dark:border-gray-700">
                <h1 class="text-base font-semibold">{move || t_string!(i18n, settings.title)}</h1>
            </header>
            <div class="flex-1 overflow-y-auto">
                <div class="max-w-2xl mx-auto p-6 space-y-5">
                    <Show when=move || state.app_info.with(|i| i.as_ref().is_some_and(|i| !i.config_writable))>
                        <div class="rounded-lg border border-amber-300 bg-amber-50 dark:bg-amber-500/10 dark:border-amber-500/40 px-4 py-3 text-xs leading-relaxed text-amber-800 dark:text-amber-200">
                            {move || t_string!(i18n, settings.config_readonly)}
                        </div>
                    </Show>

                    <Card class="p-5 space-y-2">
                        <SectionTitle
                            title=move || t_string!(i18n, settings.library)
                            hint=move || t_string!(i18n, settings.library_hint)
                        />
                        <div class="flex items-center gap-3 pt-1">
                            <code class="flex-1 min-w-0 truncate selectable rounded-md bg-gray-100 dark:bg-gray-900 px-2.5 py-1.5 text-xs text-gray-700 dark:text-gray-300">
                                {move || info(|i| i.library_dir.clone())}
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

                    <Card class="p-5">
                        <SectionTitle
                            title=move || t_string!(i18n, settings.keys)
                            hint=move || t_string!(i18n, settings.keys_hint)
                        />
                        <div class="divide-y divide-gray-100 dark:divide-gray-700 pt-1">
                            <KeyRow
                                state=state
                                id=ProviderId::Deepseek
                                title=label(|l| leptos_i18n::td_string!(l, settings.key_deepseek))
                                hint=label(|l| leptos_i18n::td_string!(l, settings.key_deepseek_hint))
                            />
                            <KeyRow
                                state=state
                                id=ProviderId::ZhipuSearch
                                title=label(|l| leptos_i18n::td_string!(l, settings.key_zhipu_search))
                                hint=label(|l| leptos_i18n::td_string!(l, settings.key_zhipu_search_hint))
                            />
                        </div>
                    </Card>

                    <Card class="p-5">
                        <SectionTitle title=move || t_string!(i18n, settings.appearance)/>
                        <div class="divide-y divide-gray-100 dark:divide-gray-700 pt-1">
                            <Row label=label(|l| leptos_i18n::td_string!(l, settings.theme))>
                                <Segmented
                                    value=Signal::derive(move || state.theme.get())
                                    options=vec![
                                        (Theme::Light, label(|l| leptos_i18n::td_string!(l, common.theme_light))),
                                        (Theme::Dark, label(|l| leptos_i18n::td_string!(l, common.theme_dark))),
                                    ]
                                    on_change=move |t: Theme| state.theme.set(t)
                                />
                            </Row>
                            <Row label=label(|l| leptos_i18n::td_string!(l, settings.language))>
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
                            <SectionTitle
                                title=move || t_string!(i18n, settings.demo_mode)
                                hint=move || t_string!(i18n, settings.demo_mode_hint)
                            />
                            <Toggle
                                checked=Signal::derive(move || state.app_info.with(|i| i.as_ref().is_some_and(|i| i.demo_mode)))
                                disabled=Signal::derive(move || state.app_info.with(|i| !i.as_ref().is_some_and(|i| i.config_writable)))
                                on_change=set_demo
                            />
                        </div>
                    </Card>

                    <Card class="p-5">
                        <SectionTitle title=move || t_string!(i18n, settings.about)/>
                        <div class="divide-y divide-gray-100 dark:divide-gray-700 pt-1">
                            <Row label=label(|l| leptos_i18n::td_string!(l, settings.version))>
                                <span class="tabular-nums selectable">{move || info(|i| i.version.clone())}</span>
                            </Row>
                            <Row label=label(|l| leptos_i18n::td_string!(l, settings.schema))>
                                <span class="tabular-nums">{move || info(|i| format!("v{}", i.schema_version))}</span>
                            </Row>
                            <Row label=label(|l| leptos_i18n::td_string!(l, settings.projects))>
                                <span class="tabular-nums">{move || state.projects.with(Vec::len)}</span>
                            </Row>
                        </div>
                    </Card>
                </div>
            </div>
        </div>
    }
}
