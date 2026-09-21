//! 上架与发布(M5,docs/adr/0009-publish-pack.md):草稿、AI 起草、实拍图导入、发布包、手机页、回填链接。
//!
//! 红线(ADR-0002):这里**不碰任何平台**。「在电脑上发布」只是用默认浏览器打开官方的发布页;
//! 「用手机发布」只是一个只读的局域网页面。点「发布」的永远是人。

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pp_agent::publish::{draft_listing, draft_notes, PublishBrief, PublishConfig};
use pp_common::publish::{
    has_blockers, lint_listing, lint_note, spec, Channel, DraftKind, DraftedNotes, ImageFit, LintIssue, ListingDraft, NoteAngle, NoteDraft, PublishImage, PublishMarked,
    PublishOverview, PublishPack, ShareInfo, ShipMode,
};
use pp_common::{cost, errcode, AssetKind, Stage};
use pp_db::NewAsset;
use serde_json::json;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use super::cad::{agent_err, fs_err, folder_for, invalid, llm_or_demo};
use super::join_err;
use crate::share::{self, SharePayload};
use crate::AppCtx;

/// 实拍图入库时缩到的最长边:发布包里的图是 1080 × 1440,留一倍余量给裁切
const PHOTO_MAX_SIDE: u32 = 2400;

fn db<T>(r: pp_db::DbResult<T>) -> Result<T, String> {
    r.map_err(|e| e.to_wire())
}

fn meta_size(meta: Option<&str>) -> (u32, u32) {
    let v: serde_json::Value = meta.and_then(|m| serde_json::from_str(m).ok()).unwrap_or_default();
    (v["width"].as_u64().unwrap_or(0) as u32, v["height"].as_u64().unwrap_or(0) as u32)
}

/// 这个项目里可以拿来配图的图:导入的实拍在前,然后是图片工作台里的图(采用的在前)。
fn image_pool(ctx: &AppCtx, project_id: &str) -> Result<Vec<PublishImage>, String> {
    let mut pool: Vec<PublishImage> = Vec::new();
    for a in db(ctx.db.list_assets(Some(project_id), Some(AssetKind::Photo)))? {
        let (width, height) = meta_size(a.meta_json.as_deref());
        pool.push(PublishImage {
            asset_id: a.id,
            ai_generated: false,
            source: "photo".into(),
            width,
            height,
        });
    }
    for img in db(ctx.db.project_overview(project_id))?.images {
        if pool.iter().any(|p| p.asset_id == img.asset_id) {
            continue;
        }
        let Ok(a) = ctx.db.get_asset(&img.asset_id) else { continue };
        let (width, height) = meta_size(a.meta_json.as_deref());
        pool.push(PublishImage {
            asset_id: a.id,
            ai_generated: a.ai_generated,
            source: "board".into(),
            width,
            height,
        });
    }
    Ok(pool)
}

fn pricing_of(ctx: &AppCtx, project_id: &str) -> Result<(Option<f64>, Option<f64>), String> {
    let model = db(ctx.db.cost_model(project_id))?;
    let unit_cost = model.saved.then(|| cost::breakdown(&model.params).unit_cost).filter(|c| c.is_finite() && *c > 0.0);
    Ok((model.chosen_price.filter(|p| *p > 0.0), unit_cost))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn publish_overview(ctx: State<'_, Arc<AppCtx>>, project_id: String) -> Result<PublishOverview, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (chosen_price, unit_cost) = pricing_of(&ctx, &project_id)?;
        let mut listing = db(ctx.db.listing_of(&project_id, Channel::Xhs))?;
        // 还没存过的商品草稿:售价先带上定价器里定的那个
        if listing.id.is_empty() && listing.price_yuan.is_none() {
            listing.price_yuan = chosen_price;
        }
        Ok(PublishOverview {
            notes: db(ctx.db.list_notes(&project_id))?,
            listing,
            packs: db(ctx.db.list_packs(&project_id))?,
            images: image_pool(&ctx, &project_id)?,
            chosen_price,
            unit_cost,
            brand_voice: db(ctx.db.brand_voice())?,
        })
    })
    .await
    .map_err(join_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn publish_note_save(ctx: State<'_, Arc<AppCtx>>, draft: NoteDraft) -> Result<NoteDraft, String> {
    db(ctx.db.save_note(&draft))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn publish_note_delete(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    db(ctx.db.delete_note(&id))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn publish_listing_save(ctx: State<'_, Arc<AppCtx>>, draft: ListingDraft) -> Result<ListingDraft, String> {
    db(ctx.db.save_listing(&draft))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn publish_voice_set(ctx: State<'_, Arc<AppCtx>>, voice: String) -> Result<(), String> {
    db(ctx.db.set_brand_voice(&voice))
}

// ---------------------------------------------------------------- AI 起草

/// 写文案要的事实:项目自己的信息 + 名下最新一个模型的尺寸 + 定价器里的售价。没有的就不写——提示词要求「没给的不要编」。
fn brief_for(ctx: &AppCtx, project_id: &str) -> Result<PublishBrief, String> {
    let project = db(ctx.db.get_project(project_id))?;
    let mut facts = Vec::new();
    if let Some(design) = db(ctx.db.list_designs_of_project(project_id))?.into_iter().find(|d| d.size.is_some()) {
        if let Some(s) = design.size {
            facts.push(format!("尺寸约 {:.0} × {:.0} × {:.0} mm", s[0], s[1], s[2]));
        }
    }
    facts.push("FDM 3D 打印,自己设计、自己打印".into());
    let (price, _) = pricing_of(ctx, project_id)?;
    if let Some(p) = price {
        facts.push(format!("售价 {p:.1} 元"));
    }
    Ok(PublishBrief {
        product: project.title,
        category: project.category,
        hypothesis: project.hypothesis,
        facts,
        brand_voice: db(ctx.db.brand_voice())?,
    })
}

fn angle_demo(angle: NoteAngle, product: &str) -> serde_json::Value {
    let (title, body) = match angle {
        NoteAngle::Pain => (format!("桌面总是乱?试试{product}"), "每天坐下来先被一团线绊住……\n\n所以自己画了一个小东西,把它们各归各位。\n\n(演示数据:没有调用模型)"),
        NoteAngle::Scene => (format!("{product}|放在桌角刚刚好"), "白色的小件,贴在桌沿,不抢眼。\n\n用了一周,桌面清爽了不少。\n\n(演示数据:没有调用模型)"),
        NoteAngle::Process => (format!("打印一个{product}的过程"), "第一版尺寸小了 1 毫米,线卡不进去。\n\n改到第三版才顺手。\n\n(演示数据:没有调用模型)"),
        NoteAngle::Backstage => (format!("{product}为什么这样设计"), "尺寸是照着自己的桌沿量的,圆角是为了不刮手。\n\n(演示数据:没有调用模型)"),
        NoteAngle::Custom => (format!("{product}可以定制"), "颜色可以换,也可以刻上名字缩写。\n\n(演示数据:没有调用模型)"),
    };
    let title: String = title.chars().take(20).collect();
    json!({ "angle": angle.as_str(), "title": title, "body": body, "tags": ["桌搭", "3D打印", "桌面收纳"] })
}

/// 按角度各起草一篇笔记,存成草稿。可以中途停止(`publish:<项目 id>`);停了就什么都不留。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_draft_notes(ctx: State<'_, Arc<AppCtx>>, project_id: String, angles: Vec<NoteAngle>) -> Result<DraftedNotes, String> {
    let ctx = ctx.inner().clone();
    let angles: Vec<NoteAngle> = if angles.is_empty() { vec![NoteAngle::Pain, NoteAngle::Scene] } else { angles.into_iter().take(5).collect() };
    let brief = brief_for(&ctx, &project_id)?;
    let turn_guard = ctx.turns.begin(&format!("publish:{project_id}"))?;
    let demo = json!({ "notes": angles.iter().map(|a| angle_demo(*a, &brief.product)).collect::<Vec<_>>() }).to_string();
    let llm = llm_or_demo(&ctx, vec![demo])?;
    let (candidates, report) = turn_guard.run(async { draft_notes(llm.as_ref(), &brief, &angles, &PublishConfig::default()).await.map_err(agent_err) }).await?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut notes = Vec::new();
        for c in candidates {
            notes.push(db(ctx.db.save_note(&NoteDraft {
                project_id: project_id.clone(),
                angle: c.angle,
                title: c.title,
                body: c.body,
                tags: c.tags,
                ai_drafted: true,
                ..Default::default()
            }))?);
        }
        db(ctx.db.add_cost(Some(&project_id), "llm", report.cost_fen, "上架 · 笔记起草"))?;
        log::info!("[publish] 起草 {} 篇笔记 · {} 次调用 · ¥{:.3} · {} ms", notes.len(), report.llm_calls, report.cost_fen / 100.0, report.elapsed_ms);
        Ok(DraftedNotes {
            notes,
            cost_fen: report.cost_fen,
            elapsed_ms: report.elapsed_ms,
        })
    })
    .await
    .map_err(join_err)?
}

/// 起草商品标题 / 卖点 / 详情,写进这个项目的商品草稿(价格、规格、图片、授权这些人填的东西不动)。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_draft_listing(ctx: State<'_, Arc<AppCtx>>, project_id: String) -> Result<ListingDraft, String> {
    let ctx = ctx.inner().clone();
    let brief = brief_for(&ctx, &project_id)?;
    let turn_guard = ctx.turns.begin(&format!("publish:{project_id}"))?;
    let demo = json!({
        "title": format!("{} 3D打印 桌面小物", brief.product),
        "selling_points": ["自己设计,尺寸合手", "PLA 打印,轻", "颜色可选"],
        "body": "3D 打印件,表面有细微层纹,每件略有差异;PLA 材质不耐高温。\n\n(演示数据:没有调用模型)"
    })
    .to_string();
    let llm = llm_or_demo(&ctx, vec![demo])?;
    let (c, report) = turn_guard.run(async { draft_listing(llm.as_ref(), &brief, &PublishConfig::default()).await.map_err(agent_err) }).await?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut listing = db(ctx.db.listing_of(&project_id, Channel::Xhs))?;
        if listing.price_yuan.is_none() {
            listing.price_yuan = pricing_of(&ctx, &project_id)?.0;
        }
        listing.title = c.title;
        listing.selling_points = c.selling_points;
        listing.body = c.body;
        db(ctx.db.add_cost(Some(&project_id), "llm", report.cost_fen, "上架 · 商品文案起草"))?;
        db(ctx.db.save_listing(&listing))
    })
    .await
    .map_err(join_err)?
}

// ---------------------------------------------------------------- 图片

/// 按 EXIF 摆正后解码。
fn decode_upright(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    use image::{DynamicImage, ImageDecoder, ImageReader};
    let unreadable = |e: image::ImageError| errcode::err(errcode::IMAGE_UNREADABLE, e);
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?;
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    let orientation = decoder.orientation().map_err(unreadable)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// 透明底铺白(JPEG 没有透明通道,直接丢掉 alpha 会变成一片黑)。
fn flatten(img: &image::DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb = image::RgbImage::new(w, h);
    for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
        let a = src[3] as u32;
        for c in 0..3 {
            dst[c] = ((src[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
    }
    rgb
}

fn encode_jpeg(rgb: &image::RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(rgb).map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?;
    Ok(out)
}

/// 把一张图放进渠道要求的画幅:`Cover` 居中裁切填满,`Contain` 整张放进去、四周留白。
/// 输出 JPEG;超过渠道的单张大小上限就逐级降质量。重新编码同时去掉了 EXIF(手机照片里有 GPS)。
fn fit_image(bytes: &[u8], (w, h): (u32, u32), fit: ImageFit, max_bytes: usize) -> Result<Vec<u8>, String> {
    use image::imageops::FilterType;
    let img = decode_upright(bytes)?;
    let canvas = match fit {
        ImageFit::Cover => flatten(&img.resize_to_fill(w, h, FilterType::Lanczos3)),
        ImageFit::Contain => {
            let inner = flatten(&img.resize(w, h, FilterType::Lanczos3));
            let mut canvas = image::RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
            let (x, y) = ((w - inner.width()) / 2, (h - inner.height()) / 2);
            image::imageops::overlay(&mut canvas, &inner, x as i64, y as i64);
            canvas
        }
    };
    let mut last = Vec::new();
    for quality in [90u8, 84, 76, 66] {
        last = encode_jpeg(&canvas, quality)?;
        if last.len() <= max_bytes {
            break;
        }
    }
    Ok(last)
}

/// 导入一张实拍图(商品首图必须是实拍)。缩到最长边 2400、摆正、去掉 EXIF。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_import_photo(ctx: State<'_, Arc<AppCtx>>, project_id: String, path: String) -> Result<PublishImage, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        db(ctx.db.get_project(&project_id))?;
        let src = Path::new(&path);
        if std::fs::metadata(src).map_err(fs_err)?.len() > 64 * 1024 * 1024 {
            return Err(invalid("image is larger than 64 MB"));
        }
        let mut img = decode_upright(&std::fs::read(src).map_err(fs_err)?)?;
        if img.width().max(img.height()) > PHOTO_MAX_SIDE {
            img = img.resize(PHOTO_MAX_SIDE, PHOTO_MAX_SIDE, image::imageops::FilterType::Lanczos3);
        }
        let rgb = flatten(&img);
        let (width, height) = rgb.dimensions();
        let jpeg = encode_jpeg(&rgb, 92)?;
        let mut new = NewAsset::new(AssetKind::Photo, "product_photo", "", "jpg", jpeg.len() as i64);
        new.rel_path = format!("assets/{}/{}.jpg", folder_for(&ctx, Some(&project_id))?, new.id);
        new.project_id = Some(project_id.clone());
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        new.meta_json = Some(json!({ "width": width, "height": height, "original_name": name }).to_string());
        let dest = ctx.asset_path(&new.rel_path);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(fs_err)?;
        }
        std::fs::write(&dest, &jpeg).map_err(fs_err)?;
        let asset = db(ctx.db.insert_asset(&new))?;
        log::info!("[publish] 实拍图 {name} → {width} × {height} · {:.0} KB(已去除 EXIF)", jpeg.len() as f64 / 1024.0);
        Ok(PublishImage {
            asset_id: asset.id,
            ai_generated: false,
            source: "photo".into(),
            width,
            height,
        })
    })
    .await
    .map_err(join_err)?
}

// ---------------------------------------------------------------- 发布包

fn listing_sheet(l: &ListingDraft) -> String {
    let mut out = format!("【商品标题】\n{}\n", l.title.trim());
    if !l.selling_points.is_empty() {
        out.push_str("\n【卖点】\n");
        for p in &l.selling_points {
            out.push_str(&format!("· {}\n", p.trim()));
        }
    }
    if let Some(p) = l.price_yuan {
        out.push_str(&format!("\n【售价】\n¥{p:.2}\n"));
    }
    if !l.skus.is_empty() {
        out.push_str("\n【规格】\n");
        for s in &l.skus {
            out.push_str(&format!("· {}  ¥{:.2}\n", s.name.trim(), s.price_yuan));
        }
    }
    out.push_str(&format!(
        "\n【发货】\n{}\n",
        match l.ship {
            ShipMode::InStock => "现货,48 小时内发货".to_string(),
            ShipMode::Presale { days } => format!("预售,付款后 {days} 天内发货"),
        }
    ));
    out
}

fn write_text(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    // 记事本认 BOM;没有 BOM 的 UTF-8 在一些旧环境里会被当成 GBK 打开
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
    std::fs::write(dir.join(name), bytes).map_err(fs_err)
}

/// 出一个发布包:再检查一遍(这一次才算数)→ 处理图片 → 写进资料库的 `packs/<项目编号>/` 下面。
/// 检查有没过的硬项、又没写 `override_reason` → `#lint_blocked#`。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_pack_build(ctx: State<'_, Arc<AppCtx>>, kind: DraftKind, draft_id: String, fit: ImageFit, override_reason: Option<String>) -> Result<PublishPack, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let reason = override_reason.unwrap_or_default().trim().to_string();
        let (project_id, channel, title, body, tags, image_ids, issues): (String, Channel, String, String, Vec<String>, Vec<String>, Vec<LintIssue>) = match kind {
            DraftKind::Note => {
                let n = db(ctx.db.get_note(&draft_id))?;
                let issues = lint_note(&n, &image_pool(&ctx, &n.project_id)?);
                (n.project_id, n.channel, n.title, n.body, n.tags, n.images, issues)
            }
            DraftKind::Listing => {
                let l = db(ctx.db.get_listing(&draft_id))?;
                let issues = lint_listing(&l, &image_pool(&ctx, &l.project_id)?, pricing_of(&ctx, &l.project_id)?.1);
                let body = if l.body.trim().is_empty() { listing_sheet(&l) } else { format!("{}\n\n{}", listing_sheet(&l), l.body.trim()) };
                (l.project_id.clone(), l.channel, l.title.clone(), body, Vec::new(), l.images.clone(), issues)
            }
        };
        if has_blockers(&issues) && reason.is_empty() {
            let codes: Vec<String> = issues.iter().filter(|i| i.severity == pp_common::publish::Severity::Block).map(|i| format!("{:?}", i.code)).collect();
            return Err(errcode::err(errcode::LINT_BLOCKED, codes.join(", ")));
        }

        let spec = spec(channel, kind);
        let pack_id = pp_db::new_id();
        // ID 是 UUIDv7:前缀是时间戳,一分钟内出的几个包前 8 位一样——文件夹名要取随机的尾段,否则会互相覆盖
        let dir_rel = format!("packs/{}/{}-{}", folder_for(&ctx, Some(&project_id))?, kind.as_str(), &pack_id[pack_id.len() - 12..]);
        let dir: PathBuf = dir_rel.split('/').fold(ctx.library_dir.clone(), |d, seg| d.join(seg));
        std::fs::create_dir_all(&dir).map_err(fs_err)?;
        let mut images = Vec::new();
        for (i, asset_id) in image_ids.iter().enumerate() {
            let asset = db(ctx.db.get_asset(asset_id))?;
            let bytes = std::fs::read(ctx.asset_path(&asset.rel_path)).map_err(fs_err)?;
            let name = format!("{:02}.jpg", i + 1);
            std::fs::write(dir.join(&name), fit_image(&bytes, spec.image_px, fit, spec.image_max_bytes)?).map_err(fs_err)?;
            images.push(name);
        }
        write_text(&dir, if kind == DraftKind::Listing { "商品信息.txt" } else { "正文.txt" }, &body)?;
        write_text(&dir, "标题.txt", &title)?;
        write_text(&dir, "话题.txt", &pp_common::publish::tags_line(&tags))?;

        let pack = db(ctx.db.insert_pack(&PublishPack {
            id: pack_id,
            project_id,
            kind: kind.as_str().into(),
            draft_id,
            channel,
            dir: dir_rel,
            images,
            title,
            body,
            tags,
            issues: if reason.is_empty() { Vec::new() } else { issues.into_iter().filter(|i| i.severity == pp_common::publish::Severity::Block).collect() },
            override_reason: reason,
            ..Default::default()
        }))?;
        log::info!("[publish] 发布包 {} · {} 张图 · {}", pack.dir, pack.images.len(), if pack.override_reason.is_empty() { "检查通过" } else { "强制跳过检查" });
        Ok(pack)
    })
    .await
    .map_err(join_err)?
}

fn pack_dir(ctx: &AppCtx, pack: &PublishPack) -> PathBuf {
    pack.dir.split('/').fold(ctx.library_dir.clone(), |d, seg| d.join(seg))
}

/// 方式 A 第一步:打开素材文件夹(把图拖进官方的发布页)。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_pack_open_folder(app: AppHandle, ctx: State<'_, Arc<AppCtx>>, pack_id: String) -> Result<(), String> {
    let pack = db(ctx.db.get_pack(&pack_id))?;
    app.opener().open_path(pack_dir(&ctx, &pack).display().to_string(), None::<&str>).map_err(|e| errcode::err(errcode::IO_FAILED, e))
}

/// 方式 A 第二步:用默认浏览器打开渠道**官方的**发布入口。之后的每一下都是人点的。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_open_site(app: AppHandle, ctx: State<'_, Arc<AppCtx>>, pack_id: String) -> Result<(), String> {
    let pack = db(ctx.db.get_pack(&pack_id))?;
    let url = spec(pack.channel, DraftKind::parse(&pack.kind)).publish_url;
    app.opener().open_url(url, None::<&str>).map_err(|e| errcode::err(errcode::IO_FAILED, e))
}

/// 方式 B:开一个临时的局域网页面给手机扫码。同一时间只有一个;再开一个会先停掉上一个。
/// `lan = false` 只监听本机(自动化验证用:不会触发系统防火墙的授权提示)。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_share_start(ctx: State<'_, Arc<AppCtx>>, pack_id: String, lan: bool, lang: String) -> Result<ShareInfo, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let pack = db(ctx.db.get_pack(&pack_id))?;
        let dir = pack_dir(&ctx, &pack);
        let payload = SharePayload {
            title: pack.title.clone(),
            body: pack.body.clone(),
            tags_line: pack.tags_line(),
            images: pack.images.iter().map(|n| dir.join(n)).collect(),
            lang,
        };
        let mut slot = ctx.share.lock().unwrap_or_else(|e| e.into_inner());
        *slot = None; // 先停掉上一个
        let handle = share::start(payload, lan).map_err(fs_err)?;
        let qr = qrcode::QrCode::new(handle.url.as_bytes()).map_err(|e| errcode::err(errcode::IO_FAILED, e))?;
        let qr_svg = qr.render::<qrcode::render::svg::Color<'_>>().min_dimensions(220, 220).quiet_zone(true).build();
        let info = ShareInfo {
            url: handle.url.clone(),
            qr_svg,
            lan: lan && share::lan_ip().is_some(),
        };
        *slot = Some(handle);
        Ok(info)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn publish_share_stop(ctx: State<'_, Arc<AppCtx>>) -> Result<(), String> {
    let ctx = ctx.inner().clone();
    // 停止要等服务线程退出(最多两三秒):放到阻塞线程池里,不占命令线程
    tauri::async_runtime::spawn_blocking(move || {
        *ctx.share.lock().unwrap_or_else(|e| e.into_inner()) = None;
    })
    .await
    .map_err(join_err)
}

/// 回填发布后的链接。项目正停在「上架」阶段、而且这一下让清单齐了 → 自动进入「运营」(PRD M5 的验收项)。
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_mark_published(ctx: State<'_, Arc<AppCtx>>, pack_id: String, url: String) -> Result<PublishMarked, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let pack = db(ctx.db.mark_published(&pack_id, &url))?;
        let project = db(ctx.db.get_project(&pack.project_id))?;
        let facts = db(ctx.db.project_facts(&pack.project_id))?;
        let advanced = project.stage == Stage::Listing && pp_common::gate::gate_passed(Stage::Listing, &facts);
        if advanced {
            db(ctx.db.move_project_stage(&pack.project_id, Stage::Operating, "system", false, "回填了发布链接"))?;
            log::info!("[publish] {} 已发布,项目进入「运营」", project.code);
        }
        Ok(PublishMarked { pack, advanced })
    })
    .await
    .map_err(join_err)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, transparent: bool) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, _| if transparent && x < w / 2 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([200, 30, 30, 255]) });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn images_are_fitted_to_the_channel_canvas() {
        // 横图裁成 3:4:居中裁切,不变形
        let cover = fit_image(&png(1600, 900, false), (1080, 1440), ImageFit::Cover, 3 << 20).unwrap();
        let img = image::load_from_memory(&cover).unwrap();
        assert_eq!((img.width(), img.height()), (1080, 1440));
        assert!(cover.starts_with(&[0xFF, 0xD8]), "输出是 JPEG");

        // 留白:整张都在,四周是白的;透明的部分也铺成白(不是黑)
        let contain = fit_image(&png(1600, 900, true), (1080, 1440), ImageFit::Contain, 3 << 20).unwrap();
        let img = image::load_from_memory(&contain).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (1080, 1440));
        let white = |p: &image::Rgb<u8>| p.0.iter().all(|c| *c > 245);
        assert!(white(img.get_pixel(540, 10)) && white(img.get_pixel(540, 1430)), "上下留白");
        assert!(white(img.get_pixel(100, 720)), "透明底铺白");
        assert!(img.get_pixel(1000, 720).0[0] > 150 && img.get_pixel(1000, 720).0[1] < 90, "右半边还是原来的红色");
    }

    #[test]
    fn oversized_output_is_recompressed_until_it_fits() {
        let noisy = image::RgbImage::from_fn(1200, 1600, |x, y| image::Rgb([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x ^ y) % 256) as u8]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(noisy).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let loose = fit_image(&bytes, (1080, 1440), ImageFit::Cover, usize::MAX).unwrap();
        let tight = fit_image(&bytes, (1080, 1440), ImageFit::Cover, loose.len() / 2).unwrap();
        assert!(tight.len() < loose.len(), "超过上限就降质量:{} → {}", loose.len(), tight.len());
    }

    #[test]
    fn the_listing_sheet_says_what_a_seller_has_to_type_into_the_shop_backend() {
        let sheet = listing_sheet(&ListingDraft {
            title: "磁吸线缆夹".into(),
            selling_points: vec!["三道线槽".into()],
            price_yuan: Some(29.9),
            skus: vec![pp_common::publish::Sku { name: "刻字".into(), price_yuan: 39.9 }],
            ship: ShipMode::Presale { days: 7 },
            ..Default::default()
        });
        for needle in ["【商品标题】\n磁吸线缆夹", "· 三道线槽", "¥29.90", "· 刻字  ¥39.90", "预售,付款后 7 天内发货"] {
            assert!(sheet.contains(needle), "{needle}\n{sheet}");
        }
    }
}
