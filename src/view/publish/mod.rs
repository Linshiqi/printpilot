//! 上架(一级入口,M5)。把一件打样定价完的单品变成「手机上点一下发布」:
//!
//!   左:项目列表
//!   中:笔记(按角度起草、多篇候选)· 商品(标题 / 卖点 / 详情 / 规格与价格)+ 配图
//!   右:合规检查(边打字边出)→ 发布包(电脑上发布 / 手机扫码发布)→ 回填链接
//!
//! 红线(ADR-0002):应用只负责起草和打包,**点「发布」的永远是人**——这里没有任何自动发布。
//! 检查的规则是前后端共用的纯函数(`pp_common::publish`);出包时后端再判一次,那一次才算数。设计见 docs/adr/0009-publish-pack.md。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_i18n::{t_string, td_string};
use pp_common::publish::{
    char_len, lint_listing, lint_note, parse_tags, spec, tags_line, Channel, DraftKind, DraftStatus, DraftedNotes, ImageFit, LintCode, LintField, LintIssue,
    ListingDraft, ModelLicense, NoteAngle, NoteDraft, PublishImage, PublishMarked, PublishOverview, PublishPack, Severity, ShareInfo, ShipMode,
};
use pp_common::imagery::{is_importable_image, IMPORT_EXTENSIONS};
use pp_common::{errcode, Project};

use crate::i18n::{use_i18n, Locale};
use crate::i18n_util::{current_locale, stage_name};
use crate::icon::{Icon, IconKind};
use crate::ipc::{self, cmd};
use crate::state::{AppState, DroppedFiles, Handoff};
use crate::ui::{
    copy_entry, image_entries, item, separator, Badge, Button, ButtonVariant, Card, Dialog, DialogFooter, EmptyState, FileDropHint, IconButton, NumInput, SectionTitle,
    Segmented, TextArea, TextInput, Tone,
};

fn angle_name(l: Locale, a: NoteAngle) -> &'static str {
    match a {
        NoteAngle::Pain => td_string!(l, publish.angle_pain),
        NoteAngle::Scene => td_string!(l, publish.angle_scene),
        NoteAngle::Process => td_string!(l, publish.angle_process),
        NoteAngle::Backstage => td_string!(l, publish.angle_backstage),
        NoteAngle::Custom => td_string!(l, publish.angle_custom),
    }
}

fn field_name(l: Locale, f: LintField) -> &'static str {
    match f {
        LintField::Title => td_string!(l, publish.f_title),
        LintField::Body => td_string!(l, publish.f_body),
        LintField::Tags => td_string!(l, publish.f_tags),
        LintField::Images => td_string!(l, publish.f_images),
        LintField::Price => td_string!(l, publish.f_price),
        LintField::License => td_string!(l, publish.f_license),
    }
}

/// 一条检查结果 → 当前语言的一句话。
pub fn issue_text(l: Locale, i: &LintIssue) -> String {
    let field = field_name(l, i.field);
    let hit = i.hit.as_str();
    match i.code {
        LintCode::TitleEmpty => td_string!(l, publish.l_title_empty).to_string(),
        LintCode::TitleTooLong => td_string!(l, publish.l_too_long, field = field, count = hit).to_string(),
        LintCode::BodyEmpty => td_string!(l, publish.l_body_empty).to_string(),
        LintCode::BodyTooLong => td_string!(l, publish.l_too_long, field = field, count = hit).to_string(),
        LintCode::TooManyTags => td_string!(l, publish.l_too_many_tags, count = hit).to_string(),
        LintCode::TooFewImages => td_string!(l, publish.l_too_few_images, count = hit).to_string(),
        LintCode::TooManyImages => td_string!(l, publish.l_too_many_images, count = hit).to_string(),
        LintCode::FirstImageNotPhoto => td_string!(l, publish.l_first_not_photo).to_string(),
        LintCode::AiImages => td_string!(l, publish.l_ai_images, count = hit).to_string(),
        LintCode::Extreme => td_string!(l, publish.l_extreme, field = field, word = hit).to_string(),
        LintCode::Medical => td_string!(l, publish.l_medical, field = field, word = hit).to_string(),
        LintCode::IpName => td_string!(l, publish.l_ip, field = field, word = hit).to_string(),
        LintCode::Diversion => td_string!(l, publish.l_diversion, field = field, word = hit).to_string(),
        LintCode::FakeReview => td_string!(l, publish.l_fake_review, field = field, word = hit).to_string(),
        LintCode::CategoryKids => td_string!(l, publish.l_cat_kids, word = hit).to_string(),
        LintCode::CategoryElectric => td_string!(l, publish.l_cat_electric, word = hit).to_string(),
        LintCode::CategoryFood => td_string!(l, publish.l_cat_food, word = hit).to_string(),
        LintCode::CategoryWeapon => td_string!(l, publish.l_cat_weapon, word = hit).to_string(),
        LintCode::PriceMissing => td_string!(l, publish.l_price_missing).to_string(),
        LintCode::PriceBelowCost => td_string!(l, publish.l_price_below_cost, price = hit).to_string(),
        LintCode::LicenseMissing => td_string!(l, publish.l_license_missing).to_string(),
        LintCode::LicenseNonCommercial => td_string!(l, publish.l_license_nc).to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Notes,
    Listing,
}

#[component]
pub fn PublishView(state: AppState) -> impl IntoView {
    let i18n = use_i18n();
    let label = move |f: fn(Locale) -> &'static str| Signal::derive(move || f(i18n.get_locale()).to_string());

    // ---- 状态 ----
    let current = RwSignal::new(None::<String>); // 打开着的项目
    let overview = RwSignal::new(None::<PublishOverview>);
    let tab = RwSignal::new(Tab::Notes);
    let busy = RwSignal::new(false);
    // 笔记编辑器(选中的那一篇)
    let note_id = RwSignal::new(None::<String>);
    let n_title = RwSignal::new(String::new());
    let n_body = RwSignal::new(String::new());
    let n_tags = RwSignal::new(String::new());
    let n_images = RwSignal::new(Vec::<String>::new());
    let n_angle = RwSignal::new(NoteAngle::Pain);
    // 商品编辑器
    let l_title = RwSignal::new(String::new());
    let l_points = RwSignal::new(String::new());
    let l_body = RwSignal::new(String::new());
    let l_price = RwSignal::new(0.0_f64);
    let l_presale = RwSignal::new(0.0_f64); // 0 = 现货
    let l_license = RwSignal::new(None::<ModelLicense>);
    let l_images = RwSignal::new(Vec::<String>::new());
    // 起草:要写哪些角度
    let angles = RwSignal::new(vec![NoteAngle::Pain, NoteAngle::Scene]);
    // 发布包
    let fit = RwSignal::new(ImageFit::Cover);
    let pack = RwSignal::new(None::<PublishPack>);
    let share = RwSignal::new(None::<ShareInfo>);
    let copied = RwSignal::new(0usize); // 逐项复制走到第几步
    let link = RwSignal::new(String::new());
    let override_dialog = RwSignal::new(false);
    let override_reason = RwSignal::new(String::new());
    let voice_dialog = RwSignal::new(false);
    let voice = RwSignal::new(String::new());
    // 上一次载入 / 存下来的内容(JSON 快照)。和它不一样才自动保存:
    // 只是点开一篇笔记不该刷新它的修改时间——列表是按修改时间排的,一点就乱序
    let saved_note = StoredValue::new(String::new());
    let saved_listing = StoredValue::new(String::new());

    let pool = move || overview.with(|o| o.as_ref().map(|o| o.images.clone()).unwrap_or_default());
    let project_of = move |id: &str| state.projects.with_untracked(|l| l.iter().find(|p| p.id == id).cloned());

    let load_note = move |n: &NoteDraft| {
        saved_note.set_value(note_snapshot(&n.title, &n.body, &n.tags, &n.images, n.angle));
        note_id.set(Some(n.id.clone()));
        n_title.set(n.title.clone());
        n_body.set(n.body.clone());
        n_tags.set(n.tags.join(" "));
        n_images.set(n.images.clone());
        n_angle.set(n.angle);
    };
    let load_listing = move |l: &ListingDraft| {
        saved_listing.set_value(serde_json::to_string(&(&l.title, &l.selling_points, &l.body, l.price_yuan, l.ship, l.model_license, &l.images)).unwrap_or_default());
        l_title.set(l.title.clone());
        l_points.set(l.selling_points.join("\n"));
        l_body.set(l.body.clone());
        l_price.set(l.price_yuan.unwrap_or(0.0));
        l_presale.set(match l.ship {
            ShipMode::InStock => 0.0,
            ShipMode::Presale { days } => days as f64,
        });
        l_license.set(l.model_license);
        l_images.set(l.images.clone());
    };
    let stop_share = move || {
        if share.get_untracked().is_some() {
            share.set(None);
            spawn_local(async move {
                let _ = ipc::call_unit_no_args(cmd::PUBLISH_SHARE_STOP).await;
            });
        }
    };
    let show = move |o: PublishOverview, keep_note: Option<String>| {
        let pick = keep_note.and_then(|id| o.notes.iter().find(|n| n.id == id).cloned()).or_else(|| o.notes.first().cloned());
        match &pick {
            Some(n) => load_note(n),
            None => {
                note_id.set(None);
                n_title.set(String::new());
                n_body.set(String::new());
                n_tags.set(String::new());
                n_images.set(Vec::new());
            }
        }
        load_listing(&o.listing);
        voice.set(o.brand_voice.clone());
        overview.set(Some(o));
    };
    let open = move |project_id: String| {
        stop_share();
        pack.set(None);
        crate::theme::set_pref("publish_project", &project_id);
        current.set(Some(project_id.clone()));
        spawn_local(async move {
            match ipc::call::<_, PublishOverview>(cmd::PUBLISH_OVERVIEW, &serde_json::json!({ "project_id": project_id })).await {
                Ok(o) => show(o, None),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let reload = move || {
        let Some(project_id) = current.get_untracked() else { return };
        let keep = note_id.get_untracked();
        spawn_local(async move {
            if let Ok(o) = ipc::call::<_, PublishOverview>(cmd::PUBLISH_OVERVIEW, &serde_json::json!({ "project_id": project_id })).await {
                show(o, keep);
            }
        });
    };
    // 从项目里过来的:打开那个项目;否则回到上次的
    let wanted = match state.handoff.get_untracked() {
        Some(Handoff::Publish { project_id }) => {
            state.handoff.set(None);
            Some(project_id)
        }
        _ => crate::theme::get_pref("publish_project"),
    };
    if let Some(id) = wanted.filter(|id| project_of(id).is_some()) {
        open(id);
    }
    on_cleanup(move || {
        // 离开页面:手机页跟着停(「页面仅在本窗口打开时可用」)
        if share.try_get_untracked().flatten().is_some() {
            spawn_local(async move {
                let _ = ipc::call_unit_no_args(cmd::PUBLISH_SHARE_STOP).await;
            });
        }
    });

    // ---- 当前草稿(从编辑器的信号拼出来;检查和保存都用它) ----
    let note_now = move |tracked: bool| -> Option<NoteDraft> {
        let get_s = |s: RwSignal<String>| if tracked { s.get() } else { s.get_untracked() };
        let id = if tracked { note_id.get() } else { note_id.get_untracked() }?;
        Some(NoteDraft {
            id,
            project_id: if tracked { current.get() } else { current.get_untracked() }?,
            angle: if tracked { n_angle.get() } else { n_angle.get_untracked() },
            title: get_s(n_title),
            body: get_s(n_body),
            tags: parse_tags(&get_s(n_tags)),
            images: if tracked { n_images.get() } else { n_images.get_untracked() },
            ..Default::default()
        })
    };
    let listing_now = move |tracked: bool| -> Option<ListingDraft> {
        let get_s = |s: RwSignal<String>| if tracked { s.get() } else { s.get_untracked() };
        let get_f = |s: RwSignal<f64>| if tracked { s.get() } else { s.get_untracked() };
        let base = if tracked { overview.with(|o| o.as_ref().map(|o| o.listing.clone())) } else { overview.with_untracked(|o| o.as_ref().map(|o| o.listing.clone())) }?;
        let days = get_f(l_presale).round().max(0.0) as u32;
        Some(ListingDraft {
            title: get_s(l_title),
            selling_points: get_s(l_points).lines().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
            body: get_s(l_body),
            price_yuan: Some(get_f(l_price)).filter(|p| *p > 0.0),
            ship: if days == 0 { ShipMode::InStock } else { ShipMode::Presale { days } },
            model_license: if tracked { l_license.get() } else { l_license.get_untracked() },
            images: if tracked { l_images.get() } else { l_images.get_untracked() },
            ..base
        })
    };
    let issues = Memo::new(move |_| -> Vec<LintIssue> {
        let images = pool();
        match tab.get() {
            Tab::Notes => note_now(true).map(|n| lint_note(&n, &images)).unwrap_or_default(),
            Tab::Listing => listing_now(true).map(|l| lint_listing(&l, &images, overview.with(|o| o.as_ref().and_then(|o| o.unit_cost)))).unwrap_or_default(),
        }
    });

    // ---- 自动保存:停手 600 毫秒落盘(和定价页一样) ----
    let save_timer = StoredValue::new_local(None::<leptos::leptos_dom::helpers::TimeoutHandle>);
    let save_now = move || {
        let which = tab.get_untracked();
        match which {
            Tab::Notes => {
                let Some(n) = note_now(false) else { return };
                let snapshot = note_snapshot(&n.title, &n.body, &n.tags, &n.images, n.angle);
                if saved_note.get_value() == snapshot {
                    return;
                }
                spawn_local(async move {
                    match ipc::call::<_, NoteDraft>(cmd::PUBLISH_NOTE_SAVE, &serde_json::json!({ "draft": n })).await {
                        Ok(saved) => overview.update(|o| {
                            saved_note.set_value(snapshot.clone());
                            if let Some(o) = o {
                                if let Some(slot) = o.notes.iter_mut().find(|x| x.id == saved.id) {
                                    *slot = saved;
                                }
                            }
                        }),
                        Err(e) => state.notify_error(e),
                    }
                });
            }
            Tab::Listing => {
                let Some(l) = listing_now(false) else { return };
                let snapshot = serde_json::to_string(&(&l.title, &l.selling_points, &l.body, l.price_yuan, l.ship, l.model_license, &l.images)).unwrap_or_default();
                if saved_listing.get_value() == snapshot {
                    return;
                }
                spawn_local(async move {
                    match ipc::call::<_, ListingDraft>(cmd::PUBLISH_LISTING_SAVE, &serde_json::json!({ "draft": l })).await {
                        Ok(saved) => overview.update(|o| {
                            saved_listing.set_value(snapshot.clone());
                            if let Some(o) = o {
                                o.listing = saved;
                            }
                        }),
                        Err(e) => state.notify_error(e),
                    }
                });
            }
        }
    };
    let schedule_save = move || {
        if let Some(h) = save_timer.get_value() {
            h.clear();
        }
        save_timer.set_value(set_timeout_with_handle(save_now, std::time::Duration::from_millis(600)).ok());
    };
    Effect::new(move |prev: Option<()>| {
        let _ = (n_title.get(), n_body.get(), n_tags.get(), n_images.get(), n_angle.get());
        if prev.is_some() && note_id.get_untracked().is_some() {
            schedule_save();
        }
    });
    Effect::new(move |prev: Option<()>| {
        let _ = (l_title.get(), l_points.get(), l_body.get(), l_price.get(), l_presale.get(), l_license.get(), l_images.get());
        if prev.is_some() && current.get_untracked().is_some() {
            schedule_save();
        }
    });

    // ---- 操作 ----
    let draft = move || {
        let Some(project_id) = current.get_untracked() else { return };
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let which = tab.get_untracked();
        spawn_local(async move {
            let result = match which {
                Tab::Notes => ipc::call::<_, DraftedNotes>(cmd::PUBLISH_DRAFT_NOTES, &serde_json::json!({ "project_id": project_id, "angles": angles.get_untracked() }))
                    .await
                    .map(|d| d.notes.first().map(|n| n.id.clone())),
                Tab::Listing => ipc::call::<_, ListingDraft>(cmd::PUBLISH_DRAFT_LISTING, &serde_json::json!({ "project_id": project_id })).await.map(|_| None),
            };
            busy.set(false);
            match result {
                Ok(first) => {
                    if first.is_some() {
                        note_id.set(first);
                    }
                    reload();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let stop = move || {
        if let Some(id) = current.get_untracked() {
            state.cancel_turn(format!("publish:{id}"));
        }
    };
    let new_note = move || {
        let Some(project_id) = current.get_untracked() else { return };
        spawn_local(async move {
            let blank = NoteDraft {
                project_id,
                angle: NoteAngle::Pain,
                ..Default::default()
            };
            match ipc::call::<_, NoteDraft>(cmd::PUBLISH_NOTE_SAVE, &serde_json::json!({ "draft": blank })).await {
                Ok(n) => {
                    note_id.set(Some(n.id));
                    reload();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let delete_note = move |id: String| {
        spawn_local(async move {
            match ipc::call_unit(cmd::PUBLISH_NOTE_DELETE, &serde_json::json!({ "id": id })).await {
                Ok(()) => {
                    if note_id.get_untracked().as_deref() == Some(id.as_str()) {
                        note_id.set(None);
                    }
                    reload();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    // 导入实拍图:「导入实拍图」按钮选的(可以多选),或者直接拖进窗口的
    let import_photos = move |paths: Vec<String>| {
        let Some(project_id) = current.get_untracked() else { return };
        let into = tab.get_untracked();
        spawn_local(async move {
            for path in paths {
                match ipc::call::<_, PublishImage>(cmd::PUBLISH_IMPORT_PHOTO, &serde_json::json!({ "project_id": project_id, "path": path })).await {
                    Ok(img) => {
                        // 新导入的实拍图直接放进正在编辑的那一份里:商品放最前(首图必须是实拍),笔记放最后
                        match into {
                            Tab::Listing => l_images.update(|l| l.insert(0, img.asset_id.clone())),
                            Tab::Notes => n_images.update(|l| l.push(img.asset_id.clone())),
                        }
                        overview.update(|o| {
                            if let Some(o) = o {
                                o.images.insert(0, img);
                            }
                        });
                    }
                    Err(e) => state.notify_error(e),
                }
            }
        });
    };
    state.accept_file_drops(move |dropped: DroppedFiles| {
        let pictures: Vec<String> = dropped.paths.into_iter().filter(|p| is_importable_image(p)).collect();
        if pictures.is_empty() {
            state.notify_info(td_string!(current_locale(), imagery.drop_reject));
        } else if current.with_untracked(Option::is_none) {
            state.notify_info(td_string!(current_locale(), publish.drop_no_project));
        } else {
            import_photos(pictures);
        }
    });
    let import_photo = move || {
        spawn_local(async move {
            import_photos(ipc::pick_files("Photo", &IMPORT_EXTENSIONS).await);
        });
    };
    let build = move |reason: Option<String>| {
        let (kind, draft_id) = match tab.get_untracked() {
            Tab::Notes => (DraftKind::Note, note_id.get_untracked()),
            Tab::Listing => (DraftKind::Listing, overview.with_untracked(|o| o.as_ref().map(|o| o.listing.id.clone()).filter(|id| !id.is_empty()))),
        };
        let Some(draft_id) = draft_id else {
            // 商品草稿还没存过(什么都没填):先存一次再出包
            save_now();
            state.notify_info(td_string!(current_locale(), publish.save_first));
            return;
        };
        stop_share();
        busy.set(true);
        spawn_local(async move {
            // 出包用的是库里的那一份:先把编辑器里的落盘
            let saved = match kind {
                DraftKind::Note => match note_now(false) {
                    Some(n) => ipc::call::<_, NoteDraft>(cmd::PUBLISH_NOTE_SAVE, &serde_json::json!({ "draft": n })).await.map(|_| ()),
                    None => Ok(()),
                },
                DraftKind::Listing => match listing_now(false) {
                    Some(l) => ipc::call::<_, ListingDraft>(cmd::PUBLISH_LISTING_SAVE, &serde_json::json!({ "draft": l })).await.map(|_| ()),
                    None => Ok(()),
                },
            };
            let built = match saved {
                Ok(()) => {
                    let args = serde_json::json!({ "kind": kind, "draft_id": draft_id, "fit": fit.get_untracked(), "override_reason": reason });
                    ipc::call::<_, PublishPack>(cmd::PUBLISH_PACK_BUILD, &args).await
                }
                Err(e) => Err(e),
            };
            busy.set(false);
            match built {
                Ok(p) => {
                    copied.set(0);
                    link.set(String::new());
                    pack.set(Some(p));
                    override_dialog.set(false);
                    state.reload_project_facts();
                    reload();
                }
                // 检查没过:问一句「为什么要跳过」
                Err(e) if errcode::split(&e).is_some_and(|(code, _)| code == errcode::LINT_BLOCKED) => override_dialog.set(true),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let pack_call = move |command: &'static str| {
        let Some(id) = pack.with_untracked(|p| p.as_ref().map(|p| p.id.clone())) else { return };
        spawn_local(async move {
            if let Err(e) = ipc::call_unit(command, &serde_json::json!({ "pack_id": id })).await {
                state.notify_error(e);
            }
        });
    };
    let start_share = move || {
        let Some(id) = pack.with_untracked(|p| p.as_ref().map(|p| p.id.clone())) else { return };
        let lang = if current_locale() == Locale::en { "en" } else { "zh" };
        spawn_local(async move {
            match ipc::call::<_, ShareInfo>(cmd::PUBLISH_SHARE_START, &serde_json::json!({ "pack_id": id, "lan": true, "lang": lang })).await {
                Ok(info) => share.set(Some(info)),
                Err(e) => state.notify_error(e),
            }
        });
    };
    let copy_step = move |step: usize, text: String| {
        crate::ui::context_menu::copy_text(state, text);
        copied.set(step + 1);
    };
    let mark_published = move || {
        let Some(id) = pack.with_untracked(|p| p.as_ref().map(|p| p.id.clone())) else { return };
        let url = link.get_untracked();
        spawn_local(async move {
            match ipc::call::<_, PublishMarked>(cmd::PUBLISH_MARK_PUBLISHED, &serde_json::json!({ "pack_id": id, "url": url })).await {
                Ok(done) => {
                    pack.set(Some(done.pack));
                    state.reload_project_facts();
                    if done.advanced {
                        crate::controller::ProjectController::new(state).load();
                        state.notify_info(td_string!(current_locale(), publish.advanced));
                    } else {
                        state.notify_info(td_string!(current_locale(), publish.marked));
                    }
                    reload();
                }
                Err(e) => state.notify_error(e),
            }
        });
    };
    let save_voice = move || {
        let v = voice.get_untracked();
        voice_dialog.set(false);
        spawn_local(async move {
            if let Err(e) = ipc::call_unit(cmd::PUBLISH_VOICE_SET, &serde_json::json!({ "voice": v })).await {
                state.notify_error(e);
            }
        });
    };

    let spec_now = move || spec(Channel::Xhs, if tab.get() == Tab::Notes { DraftKind::Note } else { DraftKind::Listing });
    let images_now = move || if tab.get() == Tab::Notes { n_images } else { l_images };
    let has_draft = move || match tab.get() {
        Tab::Notes => note_id.with(Option::is_some),
        Tab::Listing => current.with(Option::is_some),
    };

    view! {
        <div class="h-full flex flex-col">
            <header class="shrink-0 h-14 px-5 flex items-center justify-between gap-4 border-b border-gray-200 dark:border-gray-700">
                <div class="min-w-0">
                    <h1 class="text-base font-semibold leading-tight">{move || t_string!(i18n, publish.title)}</h1>
                    <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{move || t_string!(i18n, publish.subtitle)}</p>
                </div>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Pencil on_click=move || voice_dialog.set(true)>{move || t_string!(i18n, publish.brand_voice)}</Button>
            </header>

            <div class="relative flex-1 min-h-0 flex">
                // 正拖着图片悬在窗口上:松手 = 导入为这个项目的实拍图
                <FileDropHint
                    state=state
                    accepts=is_importable_image
                    icon=IconKind::Upload
                    text=Signal::derive(move || if current.with(Option::is_some) {
                        t_string!(i18n, publish.drop_photo).to_string()
                    } else {
                        t_string!(i18n, publish.drop_no_project).to_string()
                    })
                    reject=Signal::derive(move || t_string!(i18n, imagery.drop_reject).to_string())
                />
                // ---- 左:项目 ----
                <aside class="w-52 shrink-0 h-full overflow-y-auto p-2 space-y-1 border-r border-gray-200 dark:border-gray-700">
                    <Show when=move || state.projects.with(Vec::is_empty)>
                        <p class="p-3 text-xs leading-relaxed text-gray-400">{move || t_string!(i18n, publish.no_projects)}</p>
                    </Show>
                    <For each=move || state.projects.get() key=|p| (p.id.clone(), p.updated_at, p.stage_entered_at) let:p>
                        <ProjectRow state=state project=p current=current on_open=open/>
                    </For>
                </aside>

                <Show
                    when=move || current.with(Option::is_some) && overview.with(Option::is_some)
                    fallback=move || view! {
                        <div class="flex-1"><EmptyState icon=IconKind::Upload title=move || t_string!(i18n, publish.empty_title) hint=move || t_string!(i18n, publish.empty_hint)/></div>
                    }
                >
                    // ---- 中:草稿 ----
                    <section class="flex-1 min-w-0 h-full overflow-y-auto p-4 space-y-3">
                        <div class="flex items-center gap-3">
                            <Segmented
                                value=Signal::derive(move || tab.get())
                                options=vec![(Tab::Notes, label(|l| td_string!(l, publish.tab_notes))), (Tab::Listing, label(|l| td_string!(l, publish.tab_listing)))]
                                on_change=move |t: Tab| {
                                    stop_share();
                                    pack.set(None);
                                    tab.set(t);
                                }
                            />
                            <div class="flex-1"></div>
                            <Show when=move || busy.get()>
                                <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Ban on_click=stop>{move || t_string!(i18n, studio.stop)}</Button>
                            </Show>
                            <Button small=true icon=IconKind::Sparkles disabled=Signal::derive(move || busy.get()) on_click=draft>
                                {move || if busy.get() { t_string!(i18n, publish.drafting) } else { t_string!(i18n, publish.ai_draft) }}
                            </Button>
                        </div>

                        <Show when=move || tab.get() == Tab::Notes>
                            // 起草哪些角度
                            <div class="flex flex-wrap items-center gap-1.5 text-xs">
                                <span class="text-gray-500 dark:text-gray-400">{move || t_string!(i18n, publish.angles)}</span>
                                {NoteAngle::ALL.into_iter().map(|a| {
                                    let on = move || angles.with(|l| l.contains(&a));
                                    view! {
                                        <button
                                            type="button"
                                            class="h-6 px-2 rounded-full border transition-colors"
                                            class=("border-brand", on)
                                            class=("bg-brand-soft", on)
                                            class=("dark:bg-indigo-500/15", on)
                                            class=("text-brand", on)
                                            class=("border-gray-200", move || !on())
                                            class=("dark:border-gray-600", move || !on())
                                            class=("text-gray-500", move || !on())
                                            on:click=move |_| angles.update(|l| if l.contains(&a) { l.retain(|x| *x != a) } else { l.push(a) })
                                        >
                                            {move || angle_name(i18n.get_locale(), a)}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                            // 这个项目的笔记草稿
                            <div class="flex flex-wrap items-center gap-1.5">
                                <For each=move || overview.with(|o| o.as_ref().map(|o| o.notes.clone()).unwrap_or_default()) key=|n| (n.id.clone(), n.title.clone(), n.status, n.angle) let:n>
                                    <NoteChip state=state note=n selected=note_id on_pick=Callback::new(move |(n,): (NoteDraft,)| { stop_share(); pack.set(None); load_note(&n); }) on_delete=delete_note/>
                                </For>
                                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Plus on_click=new_note>{move || t_string!(i18n, publish.new_note)}</Button>
                            </div>
                        </Show>

                        <Show
                            when=has_draft
                            fallback=move || view! { <p class="py-10 text-center text-xs text-gray-400">{move || t_string!(i18n, publish.no_note)}</p> }
                        >
                            <Card class="p-4 space-y-3">
                                <Counted label=label(|l| td_string!(l, publish.f_title)) len=Signal::derive(move || if tab.get() == Tab::Notes { char_len(&n_title.get()) } else { char_len(&l_title.get()) }) max=Signal::derive(move || spec_now().title_max)>
                                    {move || if tab.get() == Tab::Notes { view! { <TextInput value=n_title/> }.into_any() } else { view! { <TextInput value=l_title/> }.into_any() }}
                                </Counted>
                                <Show when=move || tab.get() == Tab::Listing>
                                    <Counted label=label(|l| td_string!(l, publish.f_points)) len=Signal::derive(move || l_points.get().lines().filter(|p| !p.trim().is_empty()).count()) max=Signal::derive(|| 5)>
                                        <TextArea value=l_points rows=4 placeholder=move || t_string!(i18n, publish.points_placeholder)/>
                                    </Counted>
                                </Show>
                                <Counted label=label(|l| td_string!(l, publish.f_body)) len=Signal::derive(move || if tab.get() == Tab::Notes { char_len(&n_body.get()) } else { char_len(&l_body.get()) }) max=Signal::derive(move || spec_now().body_max)>
                                    {move || if tab.get() == Tab::Notes { view! { <TextArea value=n_body rows=11/> }.into_any() } else { view! { <TextArea value=l_body rows=8/> }.into_any() }}
                                </Counted>
                                <Show when=move || tab.get() == Tab::Notes>
                                    <Counted label=label(|l| td_string!(l, publish.f_tags)) len=Signal::derive(move || parse_tags(&n_tags.get()).len()) max=Signal::derive(move || spec_now().tags_max)>
                                        <TextInput value=n_tags placeholder=move || t_string!(i18n, publish.tags_placeholder)/>
                                    </Counted>
                                </Show>
                                <Show when=move || tab.get() == Tab::Listing>
                                    <div class="grid grid-cols-3 gap-3">
                                        <div class="space-y-1">
                                            <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.f_price)}</div>
                                            <NumInput value=l_price unit="¥"/>
                                            <div class="text-[11px] text-gray-400">
                                                {move || overview.with(|o| o.as_ref().and_then(|o| o.unit_cost).map(|c| t_string!(i18n, publish.unit_cost, yuan = format!("{c:.2}")).to_string()).unwrap_or_default())}
                                            </div>
                                        </div>
                                        <div class="space-y-1">
                                            <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.presale_days)}</div>
                                            <NumInput value=l_presale decimals=0 unit="d"/>
                                            <div class="text-[11px] text-gray-400">{move || t_string!(i18n, publish.presale_hint)}</div>
                                        </div>
                                        <div class="space-y-1">
                                            <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.f_license)}</div>
                                            <select
                                                class="w-full h-8 px-2 rounded-md border border-gray-200 dark:border-gray-600 bg-white dark:bg-gray-900 text-xs"
                                                on:change=move |e| l_license.set(match event_target_value(&e).as_str() {
                                                    "original" => Some(ModelLicense::Original),
                                                    "licensed" => Some(ModelLicense::Licensed),
                                                    "non_commercial" => Some(ModelLicense::NonCommercial),
                                                    _ => None,
                                                })
                                            >
                                                <option value="" selected=move || l_license.get().is_none()>{move || t_string!(i18n, publish.license_pick)}</option>
                                                <option value="original" selected=move || l_license.get() == Some(ModelLicense::Original)>{move || t_string!(i18n, publish.license_original)}</option>
                                                <option value="licensed" selected=move || l_license.get() == Some(ModelLicense::Licensed)>{move || t_string!(i18n, publish.license_licensed)}</option>
                                                <option value="non_commercial" selected=move || l_license.get() == Some(ModelLicense::NonCommercial)>{move || t_string!(i18n, publish.license_nc)}</option>
                                            </select>
                                        </div>
                                    </div>
                                </Show>
                            </Card>

                            // ---- 配图:上面是选中的(顺序 = 发布顺序),下面是这个项目里能用的 ----
                            <Card class="p-4 space-y-3">
                                <div class="flex items-center justify-between gap-2">
                                    <SectionTitle title=move || t_string!(i18n, publish.f_images) hint=move || {
                                        let s = spec_now();
                                        t_string!(i18n, publish.images_hint, min = s.images_min, max = s.images_max).to_string()
                                    }/>
                                    <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Upload on_click=import_photo>{move || t_string!(i18n, publish.import_photo)}</Button>
                                </div>
                                <div class="flex flex-wrap gap-2 min-h-[5rem]">
                                    {move || {
                                        let chosen = images_now();
                                        let list = chosen.get();
                                        let last = list.len().saturating_sub(1);
                                        let images = pool();
                                        list.into_iter().enumerate().map(|(i, id)| {
                                            let ai = images.iter().any(|p| p.asset_id == id && p.ai_generated);
                                            view! { <PickedImage state=state asset_id=id index=i last=last ai=ai chosen=chosen/> }
                                        }).collect_view()
                                    }}
                                    <Show when=move || images_now().with(Vec::is_empty)>
                                        <p class="self-center text-xs text-gray-400">{move || t_string!(i18n, publish.no_images_picked)}</p>
                                    </Show>
                                </div>
                                <div class="pt-2 border-t border-gray-100 dark:border-gray-700 space-y-1.5">
                                    <div class="text-[11px] text-gray-500 dark:text-gray-400">{move || t_string!(i18n, publish.pool_hint)}</div>
                                    <div class="flex flex-wrap gap-1.5">
                                        {move || {
                                            let chosen = images_now();
                                            pool().into_iter().map(|img| {
                                                let id = StoredValue::new(img.asset_id.clone());
                                                let picked = move || chosen.with(|l| id.with_value(|id| l.contains(id)));
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="relative w-14 h-14 rounded-md overflow-hidden border-2 transition-colors"
                                                        class=("border-brand", picked)
                                                        class=("opacity-40", picked)
                                                        class=("border-transparent", move || !picked())
                                                        on:click=move |_| chosen.update(|l| {
                                                            let id = id.get_value();
                                                            if l.contains(&id) { l.retain(|x| x != &id) } else { l.push(id) }
                                                        })
                                                    >
                                                        <img src=ipc::asset_url(&img.asset_id) class="w-full h-full object-cover" draggable="false"/>
                                                        {img.ai_generated.then(|| view! { <span class="absolute left-0 bottom-0 px-1 text-[9px] leading-tight bg-amber-500 text-white">"AI"</span> })}
                                                    </button>
                                                }
                                            }).collect_view()
                                        }}
                                        <Show when=move || pool().is_empty()>
                                            <p class="text-xs text-gray-400">{move || t_string!(i18n, publish.pool_empty)}</p>
                                        </Show>
                                    </div>
                                </div>
                            </Card>
                        </Show>
                    </section>

                    // ---- 右:检查 + 发布包 ----
                    <aside class="w-[22rem] shrink-0 h-full overflow-y-auto p-4 space-y-3 border-l border-gray-200 dark:border-gray-700">
                        <Card class="p-4 space-y-2">
                            <SectionTitle title=move || t_string!(i18n, publish.check_title)/>
                            <Show
                                when=move || !issues.with(Vec::is_empty)
                                fallback=move || view! {
                                    <div class="flex items-center gap-1.5 text-xs text-green-600 dark:text-green-400">
                                        <Icon kind=IconKind::Check class="w-3.5 h-3.5"/>{move || t_string!(i18n, publish.check_ok)}
                                    </div>
                                }
                            >
                                <ul class="space-y-1.5 text-xs leading-relaxed">
                                    <For each=move || issues.get() key=|i| format!("{:?}{:?}{}", i.code, i.field, i.hit) let:i>
                                        {
                                            let block = i.severity == Severity::Block;
                                            view! {
                                                <li class="flex items-start gap-1.5" class=("text-red-600", block) class=("dark:text-red-400", block) class=("text-amber-700", !block) class=("dark:text-amber-300", !block)>
                                                    <Icon kind=if block { IconKind::Ban } else { IconKind::Alert } class="w-3.5 h-3.5 mt-0.5 shrink-0"/>
                                                    <span class="break-words selectable">{move || issue_text(i18n.get_locale(), &i)}</span>
                                                </li>
                                            }
                                        }
                                    </For>
                                </ul>
                            </Show>
                        </Card>

                        <Card class="p-4 space-y-3">
                            <SectionTitle title=move || t_string!(i18n, publish.pack_title) hint=move || t_string!(i18n, publish.pack_hint)/>
                            <div class="flex items-center gap-2">
                                <Segmented
                                    value=Signal::derive(move || fit.get())
                                    options=vec![(ImageFit::Cover, label(|l| td_string!(l, publish.fit_cover))), (ImageFit::Contain, label(|l| td_string!(l, publish.fit_contain)))]
                                    on_change=move |f: ImageFit| fit.set(f)
                                />
                                <div class="flex-1"></div>
                                <Button small=true icon=IconKind::Package disabled=Signal::derive(move || busy.get() || !has_draft()) on_click=move || build(None)>
                                    {move || t_string!(i18n, publish.build_pack)}
                                </Button>
                            </div>
                            <p class="text-[11px] leading-relaxed text-gray-400">{move || t_string!(i18n, publish.why_manual)}</p>
                        </Card>

                        {move || pack.get().map(|p| {
                            let steps: Vec<(usize, &'static str, String)> = [(0usize, "title", p.title.clone()), (1, "body", p.body.clone()), (2, "tags", tags_line(&p.tags))]
                                .into_iter()
                                .filter(|(_, _, text)| !text.trim().is_empty())
                                .collect();
                            let published = p.external_url.clone();
                            let forced = p.override_reason.clone();
                            view! {
                                <Card class="p-4 space-y-3">
                                    <div class="flex items-center justify-between gap-2">
                                        <div class="text-sm font-semibold">{move || t_string!(i18n, publish.pack_ready, n = p.images.len()).to_string()}</div>
                                        {published.is_some().then(|| view! { <Badge tone=Tone::Green>{move || t_string!(i18n, publish.st_published)}</Badge> })}
                                    </div>
                                    {(!forced.is_empty()).then(|| view! { <p class="text-[11px] text-amber-700 dark:text-amber-300 break-words">{move || t_string!(i18n, publish.forced_note, reason = forced.clone()).to_string()}</p> })}

                                    // 方式 A:在电脑上发布
                                    <div class="space-y-2">
                                        <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.way_a)}</div>
                                        <div class="flex flex-wrap gap-1.5">
                                            <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Folder on_click=move || pack_call(cmd::PUBLISH_PACK_OPEN_FOLDER)>{move || t_string!(i18n, publish.open_folder)}</Button>
                                            <Button small=true variant=ButtonVariant::Secondary icon=IconKind::External on_click=move || pack_call(cmd::PUBLISH_OPEN_SITE)>{move || t_string!(i18n, publish.open_site)}</Button>
                                        </div>
                                        <div class="flex flex-wrap gap-1.5">
                                            {steps.into_iter().enumerate().map(|(order, (_, key, text))| {
                                                let text = StoredValue::new(text);
                                                let next = move || copied.get() == order;
                                                let done = move || copied.get() > order;
                                                view! {
                                                    <Button
                                                        small=true
                                                        variant=ButtonVariant::Secondary
                                                        icon=IconKind::Copy
                                                        on_click=move || copy_step(order, text.get_value())
                                                    >
                                                        <span class=("font-semibold", next) class=("text-brand", next) class=("line-through", done) class=("opacity-60", done)>
                                                            {move || match key {
                                                                "title" => t_string!(i18n, publish.copy_title),
                                                                "body" => t_string!(i18n, publish.copy_body),
                                                                _ => t_string!(i18n, publish.copy_tags),
                                                            }}
                                                        </span>
                                                    </Button>
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>

                                    // 方式 B:用手机发布
                                    <div class="space-y-2 pt-2 border-t border-gray-100 dark:border-gray-700">
                                        <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.way_b)}</div>
                                        <Show
                                            when=move || share.with(Option::is_some)
                                            fallback=move || view! {
                                                <div class="space-y-1.5">
                                                    <Button small=true variant=ButtonVariant::Secondary icon=IconKind::Crosshair on_click=start_share>{move || t_string!(i18n, publish.share_start)}</Button>
                                                    <p class="text-[11px] leading-relaxed text-gray-400">{move || t_string!(i18n, publish.share_hint)}</p>
                                                </div>
                                            }
                                        >
                                            <div class="flex items-start gap-3">
                                                <div class="w-36 h-36 shrink-0 p-1.5 rounded-lg bg-white [&>svg]:w-full [&>svg]:h-full" inner_html=move || share.with(|s| s.as_ref().map(|s| s.qr_svg.clone()).unwrap_or_default())></div>
                                                <div class="min-w-0 space-y-1.5">
                                                    <p class="text-[11px] leading-relaxed text-gray-500 dark:text-gray-400">
                                                        {move || if share.with(|s| s.as_ref().is_some_and(|s| s.lan)) { t_string!(i18n, publish.share_scan) } else { t_string!(i18n, publish.share_local_only) }}
                                                    </p>
                                                    <p class="text-[10px] break-all text-gray-400 selectable">{move || share.with(|s| s.as_ref().map(|s| s.url.clone()).unwrap_or_default())}</p>
                                                    <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Close on_click=stop_share>{move || t_string!(i18n, publish.share_stop)}</Button>
                                                </div>
                                            </div>
                                        </Show>
                                    </div>

                                    // 回填链接
                                    <div class="space-y-1.5 pt-2 border-t border-gray-100 dark:border-gray-700">
                                        <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.backfill)}</div>
                                        {published.clone().map(|u| view! { <p class="text-[11px] break-all text-green-700 dark:text-green-400 selectable">{u}</p> })}
                                        <div class="flex items-center gap-1.5">
                                            <div class="flex-1"><TextInput value=link placeholder="https://www.xiaohongshu.com/…" on_enter=mark_published/></div>
                                            <Button small=true icon=IconKind::Check disabled=Signal::derive(move || link.with(|l| l.trim().is_empty())) on_click=mark_published>{move || t_string!(i18n, publish.mark_published)}</Button>
                                        </div>
                                    </div>
                                </Card>
                            }
                        })}

                        // 出过的包
                        <Show when=move || overview.with(|o| o.as_ref().is_some_and(|o| !o.packs.is_empty()))>
                            <Card class="p-4 space-y-2">
                                <SectionTitle title=move || t_string!(i18n, publish.history)/>
                                <For each=move || overview.with(|o| o.as_ref().map(|o| o.packs.clone()).unwrap_or_default()) key=|p| (p.id.clone(), p.external_url.clone()) let:p>
                                    {
                                        let row = StoredValue::new(p.clone());
                                        let active = move || pack.with(|x| x.as_ref().is_some_and(|x| row.with_value(|r| r.id == x.id)));
                                        view! {
                                            <button
                                                type="button"
                                                class="w-full flex items-center gap-2 px-2 py-1.5 rounded-lg text-left text-xs transition-colors"
                                                class=("bg-brand-soft", active)
                                                class=("dark:bg-indigo-500/15", active)
                                                class=("hover:bg-gray-100", move || !active())
                                                class=("dark:hover:bg-gray-700/60", move || !active())
                                                on:click=move |_| { stop_share(); copied.set(0); link.set(String::new()); pack.set(Some(row.get_value())); }
                                            >
                                                <Icon kind=if p.kind == "listing" { IconKind::Package } else { IconKind::Image } class="w-3.5 h-3.5 shrink-0 text-gray-400"/>
                                                <span class="flex-1 min-w-0 truncate">{if p.title.trim().is_empty() { "—".to_string() } else { p.title.clone() }}</span>
                                                {p.external_url.is_some().then(|| view! { <Icon kind=IconKind::Check class="w-3.5 h-3.5 shrink-0 text-green-500"/> })}
                                            </button>
                                        }
                                    }
                                </For>
                            </Card>
                        </Show>
                    </aside>
                </Show>
            </div>

            <Dialog open=override_dialog title=move || t_string!(i18n, publish.override_title)>
                <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.override_hint)}</p>
                <TextArea value=override_reason rows=3 placeholder=move || t_string!(i18n, publish.override_placeholder)/>
                <DialogFooter>
                    <Button variant=ButtonVariant::Secondary on_click=move || override_dialog.set(false)>{move || t_string!(i18n, common.cancel)}</Button>
                    <Button variant=ButtonVariant::Danger disabled=Signal::derive(move || override_reason.with(|r| r.trim().is_empty())) on_click=move || build(Some(override_reason.get_untracked()))>
                        {move || t_string!(i18n, publish.override_go)}
                    </Button>
                </DialogFooter>
            </Dialog>
            <Dialog open=voice_dialog title=move || t_string!(i18n, publish.brand_voice)>
                <p class="text-sm leading-relaxed text-gray-600 dark:text-gray-300">{move || t_string!(i18n, publish.voice_hint)}</p>
                <TextArea value=voice rows=4 placeholder=move || t_string!(i18n, publish.voice_placeholder)/>
                <DialogFooter>
                    <Button variant=ButtonVariant::Secondary on_click=move || voice_dialog.set(false)>{move || t_string!(i18n, common.cancel)}</Button>
                    <Button on_click=save_voice>{move || t_string!(i18n, common.save)}</Button>
                </DialogFooter>
            </Dialog>
        </div>
    }
}

/// 一篇笔记里会被保存的那几样,拼成一个可以比较的串。
fn note_snapshot(title: &str, body: &str, tags: &[String], images: &[String], angle: NoteAngle) -> String {
    serde_json::to_string(&(title, body, tags, images, angle)).unwrap_or_default()
}

/// 带字数的一栏:超了变红。
#[component]
fn Counted(#[prop(into)] label: Signal<String>, #[prop(into)] len: Signal<usize>, #[prop(into)] max: Signal<usize>, children: Children) -> impl IntoView {
    let over = move || max.get() > 0 && len.get() > max.get();
    view! {
        <div class="space-y-1">
            <div class="flex items-center justify-between text-xs">
                <span class="font-medium text-gray-600 dark:text-gray-300">{move || label.get()}</span>
                <span class="tabular-nums" class=("text-red-600", over) class=("dark:text-red-400", over) class=("text-gray-400", move || !over())>
                    {move || format!("{}/{}", len.get(), max.get())}
                </span>
            </div>
            {children()}
        </div>
    }
}

#[component]
fn ProjectRow(state: AppState, project: Project, current: RwSignal<Option<String>>, #[prop(into)] on_open: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(project.id.clone());
    let active = move || current.with(|c| id.with_value(|id| c.as_deref() == Some(id.as_str())));
    let stage = project.stage;
    let published = move || id.with_value(|id| state.facts_of(id).published > 0);
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
                state.open_menu(&ev, vec![
                    item(td_string!(l, menu.open), IconKind::Upload, move || on_open.run((id.get_value(),))),
                    item(td_string!(l, board.menu_open), IconKind::Kanban, move || state.open_project.set(Some(id.get_value()))),
                ]);
            }
        >
            <div class="flex items-center justify-between gap-2">
                <span class="text-[11px] font-mono text-gray-400">{project.code.clone()}</span>
                <Show when=published>
                    <span class="text-[11px] text-green-600 dark:text-green-400">{move || t_string!(i18n, publish.st_published)}</span>
                </Show>
            </div>
            <div class="text-xs font-medium truncate text-gray-800 dark:text-gray-100">{project.title.clone()}</div>
            <div class="text-[11px] text-gray-400">{move || stage_name(i18n.get_locale(), stage)}</div>
        </div>
    }
}

#[component]
fn NoteChip(state: AppState, note: NoteDraft, selected: RwSignal<Option<String>>, on_pick: Callback<(NoteDraft,)>, #[prop(into)] on_delete: Callback<(String,)>) -> impl IntoView {
    let i18n = use_i18n();
    let n = StoredValue::new(note.clone());
    let active = move || selected.with(|s| n.with_value(|n| s.as_deref() == Some(n.id.as_str())));
    let angle = note.angle;
    let status = note.status;
    let title = if note.title.trim().is_empty() { "—".to_string() } else { note.title.chars().take(10).collect() };
    view! {
        <button
            type="button"
            class="h-7 max-w-[14rem] px-2.5 inline-flex items-center gap-1.5 rounded-full border text-xs transition-colors"
            class=("border-brand", active)
            class=("bg-brand-soft", active)
            class=("dark:bg-indigo-500/15", active)
            class=("border-gray-200", move || !active())
            class=("dark:border-gray-600", move || !active())
            on:click=move |_| on_pick.run((n.get_value(),))
            on:contextmenu=move |ev| {
                let l = current_locale();
                let note = n.get_value();
                let id = note.id.clone();
                state.open_menu(&ev, vec![
                    copy_entry(state, td_string!(l, publish.copy_title), note.title.clone()),
                    copy_entry(state, td_string!(l, publish.copy_body), note.body.clone()),
                    separator(),
                    item(td_string!(l, menu.delete), IconKind::Trash, move || on_delete.run((id.clone(),))).danger(),
                ]);
            }
        >
            <span class="text-gray-500 dark:text-gray-400">{move || angle_name(i18n.get_locale(), angle)}</span>
            <span class="truncate">{title}</span>
            {(status == DraftStatus::Published).then(|| view! { <Icon kind=IconKind::Check class="w-3 h-3 shrink-0 text-green-500"/> })}
        </button>
    }
}

/// 选中的一张配图:左右挪顺序、拿掉。第一张是封面 / 首图。
#[component]
fn PickedImage(state: AppState, asset_id: String, index: usize, last: usize, ai: bool, chosen: RwSignal<Vec<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(asset_id.clone());
    let shift = move |delta: isize| {
        chosen.update(|l| {
            let to = index as isize + delta;
            if to >= 0 && (to as usize) < l.len() {
                l.swap(index, to as usize);
            }
        })
    };
    view! {
        <div
            class="group relative w-20 h-[6.6rem] rounded-lg overflow-hidden border border-gray-200 dark:border-gray-700 bg-gray-100 dark:bg-gray-900"
            on:contextmenu=move |ev| {
                let l = current_locale();
                let mut entries = image_entries(state, id.get_value());
                entries.push(separator());
                entries.push(item(td_string!(l, publish.img_first), IconKind::Star, move || chosen.update(|l| { let x = l.remove(index); l.insert(0, x); })).disabled_if(index == 0));
                entries.push(item(td_string!(l, publish.img_remove), IconKind::Close, move || chosen.update(|l| { l.remove(index); })).danger());
                state.open_menu(&ev, entries);
            }
        >
            <img src=ipc::asset_url(&asset_id) class="w-full h-full object-cover" draggable="false"/>
            <span class="absolute left-1 top-1 px-1 rounded text-[10px] leading-tight bg-black/60 text-white">{if index == 0 { "★ 1".to_string() } else { (index + 1).to_string() }}</span>
            {ai.then(|| view! { <span class="absolute right-1 top-1 px-1 rounded text-[9px] leading-tight bg-amber-500 text-white">"AI"</span> })}
            <div class="absolute inset-x-0 bottom-0 flex items-center justify-between opacity-0 group-hover:opacity-100 transition-opacity bg-white/90 dark:bg-gray-800/90">
                <IconButton icon=IconKind::ArrowRight label=move || t_string!(i18n, publish.img_left) disabled=Signal::derive(move || index == 0) on_click=move || shift(-1)/>
                <IconButton icon=IconKind::Close label=move || t_string!(i18n, publish.img_remove) on_click=move || chosen.update(|l| { l.remove(index); })/>
                <IconButton icon=IconKind::ArrowRight label=move || t_string!(i18n, publish.img_right) disabled=Signal::derive(move || index >= last) on_click=move || shift(1)/>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lint_code_has_a_sentence_in_every_locale() {
        let codes = [
            LintCode::TitleEmpty, LintCode::TitleTooLong, LintCode::BodyEmpty, LintCode::BodyTooLong, LintCode::TooManyTags, LintCode::TooFewImages, LintCode::TooManyImages,
            LintCode::FirstImageNotPhoto, LintCode::AiImages, LintCode::Extreme, LintCode::Medical, LintCode::IpName, LintCode::Diversion, LintCode::FakeReview,
            LintCode::CategoryKids, LintCode::CategoryElectric, LintCode::CategoryFood, LintCode::CategoryWeapon, LintCode::PriceMissing, LintCode::PriceBelowCost,
            LintCode::LicenseMissing, LintCode::LicenseNonCommercial,
        ];
        for l in [Locale::zh, Locale::en] {
            for code in codes {
                let text = issue_text(l, &LintIssue { code, severity: Severity::Block, field: LintField::Title, hit: "顶级".into() });
                assert!(!text.is_empty(), "{code:?}");
            }
            for a in NoteAngle::ALL {
                assert!(!angle_name(l, a).is_empty());
            }
        }
        let zh = issue_text(Locale::zh, &LintIssue { code: LintCode::Extreme, severity: Severity::Block, field: LintField::Body, hit: "顶级".into() });
        assert!(zh.contains("顶级") && zh.contains("正文"), "命中的词和所在的栏都要说出来:{zh}");
        assert!(issue_text(Locale::zh, &LintIssue { code: LintCode::TitleTooLong, severity: Severity::Block, field: LintField::Title, hit: "23/20".into() }).contains("23/20"));
    }
}
