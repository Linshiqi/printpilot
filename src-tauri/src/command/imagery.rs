//! 图片工作台的命令:画板、和 AI 的持续对话出图 / 改图、导入自己的图、采用、送去建模、导出。
//!
//! 一轮对话 = 规划(DeepSeek 看着当前图写提示词、判断是重新生成 / 改这张 / 只回答)→ 调图片模型 → **立刻落盘** → 入库。
//! 和建模工作室同一条约定:**一轮要么整轮入库,要么什么都不留**(规划或出图失败时不写半轮消息)。
//!
//! 供应商怎么选:生成用最便宜的(MiniMax),编辑用能编辑的(通义千问图像);只配了一家就都用它;
//! 一家能编辑的都没有时,「改一下」只能改写提示词重新生成——规划那一步知道这一点,会如实告诉用户主体会变。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use pp_agent::{plan_image_turn, ChatLine, ImageAction, ImagePlanConfig, ImageTurn};
use pp_common::design::CadDesign;
use pp_common::imagery::{
    is_importable_image, ImageAspect, ImageBoard, ImageBoardDetail, ImageBoardSummary, ImageImportResult, ImageMsgExtra, ImageMsgKind,
    ImageProviderInfo, ImagePurpose, ImageSettings, ImageTurnReport, ImageTurnResult, ImageVersion, ImportFailure, IMPORT_MAX_FILES,
};
use pp_common::provider::ProviderId;
use pp_common::{errcode, Asset, AssetKind};
use pp_db::{NewAsset, NewImageMessage, NewImageVersion};
use pp_providers::image::{ImageBytes, ImageProvider, ImageRequest, MiniMaxImage, QwenImage};
use pp_providers::llm::{image_data_url, LlmPricing};
use pp_providers::ProviderError;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

use super::cad::{
    agent_err, decode_upright, folder_for, fs_err, invalid, llm_or_demo, prepare_reference, reference_jpeg, MAX_REFERENCE_IMAGES, REFERENCE_MAX_SIDE,
};
use super::join_err;
use super::provider::{key_of, provider_err};
use crate::AppCtx;

const DEFAULT_NAME: &str = "未命名画板";
/// 进度事件名(前端 listen 这个)。载荷:`{ board_id, phase: planning|rendering|saving, provider?, count? }`
const PROGRESS_EVENT: &str = "image-progress";

fn emit_progress(app: &AppHandle, board_id: &str, phase: &str, provider: &str, count: u32) {
    let _ = app.emit(PROGRESS_EVENT, json!({ "board_id": board_id, "phase": phase, "provider": provider, "count": count }));
}

fn db<T>(r: pp_db::DbResult<T>) -> Result<T, String> {
    r.map_err(|e| e.to_wire())
}

// ---------------------------------------------------------------- 供应商

/// 演示模式的「假画师」:不联网,在本机画一张示意图;「编辑」= 只换背景色、主体像素不动——
/// 正好演示「按指令改图、主体保持不变」是什么意思。
struct DemoPainter;

fn seed_of(text: &str, salt: u32) -> u32 {
    text.bytes().fold(2166136261u32 ^ salt, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

fn pastel(seed: u32) -> [u8; 3] {
    [200 + (seed % 48) as u8, 200 + ((seed >> 8) % 48) as u8, 200 + ((seed >> 16) % 48) as u8]
}

fn encode_jpeg(img: &image::RgbImage) -> Result<Vec<u8>, ProviderError> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
        .encode_image(img)
        .map_err(|e| ProviderError::BadResponse(e.to_string()))?;
    Ok(out)
}

impl DemoPainter {
    fn canvas(aspect: ImageAspect) -> (u32, u32) {
        match aspect {
            ImageAspect::Square => (768, 768),
            ImageAspect::Portrait => (720, 960),
            ImageAspect::Landscape => (960, 720),
            ImageAspect::Tall => (648, 1152),
            ImageAspect::Wide => (1152, 648),
        }
    }

    /// 一个示意的「三槽线缆夹」侧视图:底座 + 几道槽,每张的槽数和颜色略有不同。
    fn draw(prompt: &str, aspect: ImageAspect, index: u32) -> image::RgbImage {
        let (w, h) = Self::canvas(aspect);
        let seed = seed_of(prompt, index);
        let bg = if prompt.contains('白') { [250, 250, 250] } else { pastel(seed) };
        let body = [70 + (seed % 60) as u8, 80 + ((seed >> 5) % 60) as u8, 200 + ((seed >> 11) % 50) as u8];
        let slots = 2 + (seed >> 3) % 3;
        let (bw, bh) = (w * 6 / 10, h / 4);
        let (x0, y0) = ((w - bw) / 2, h / 2 - bh / 3);
        let mut img = image::RgbImage::from_pixel(w, h, image::Rgb(bg));
        for y in y0..y0 + bh {
            for x in x0..x0 + bw {
                let in_slot = (0..slots).any(|i| {
                    let cx = x0 + bw * (i + 1) / (slots + 1);
                    x.abs_diff(cx) < bw / 22 && y < y0 + bh * 6 / 10
                });
                if !in_slot {
                    // 顶部一条亮边,像是受光面
                    let shade = if y < y0 + 6 { 30 } else { 0 };
                    img.put_pixel(x, y, image::Rgb([body[0].saturating_add(shade), body[1].saturating_add(shade), body[2].saturating_add(shade)]));
                }
            }
        }
        // 接触阴影
        for x in x0..x0 + bw {
            for dy in 0..8u32 {
                let y = y0 + bh + dy;
                if y < h {
                    let p = img.get_pixel_mut(x, y);
                    for c in 0..3 {
                        p[c] = p[c].saturating_sub(24 - dy as u8 * 3);
                    }
                }
            }
        }
        img
    }

    /// 只换背景:把「接近左上角那个颜色」的像素换成新颜色,其余像素原样保留。
    fn recolor_background(input: &str, instruction: &str, index: u32) -> Result<image::RgbImage, ProviderError> {
        use base64::Engine as _;
        let b64 = input.split_once(',').map(|(_, b)| b).unwrap_or(input);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| ProviderError::BadResponse(format!("demo: bad input image: {e}")))?;
        let mut img = image::load_from_memory(&bytes).map_err(|e| ProviderError::BadResponse(e.to_string()))?.to_rgb8();
        let old = *img.get_pixel(2, 2);
        let seed = seed_of(instruction, index + 7);
        let new = if instruction.contains('木') { [222, 196, 160] } else { pastel(seed) };
        for p in img.pixels_mut() {
            let near = (0..3).all(|c| p[c].abs_diff(old[c]) < 26);
            if near {
                *p = image::Rgb(new);
            }
        }
        Ok(img)
    }
}

#[async_trait]
impl ImageProvider for DemoPainter {
    fn name(&self) -> &'static str {
        "demo"
    }
    fn model(&self) -> String {
        "demo-painter".into()
    }
    fn can_edit(&self) -> bool {
        true
    }
    fn max_count(&self) -> u32 {
        4
    }
    fn price_fen(&self) -> f64 {
        0.0
    }
    async fn generate(&self, req: &ImageRequest) -> Result<Vec<ImageBytes>, ProviderError> {
        let mut out = Vec::new();
        for i in 0..req.count.clamp(1, self.max_count()) {
            let img = match req.images.first() {
                Some(input) => Self::recolor_background(input, &req.prompt, i)?,
                None => Self::draw(&req.prompt, req.aspect, i),
            };
            out.push(ImageBytes {
                bytes: encode_jpeg(&img)?,
                ext: "jpg".into(),
            });
        }
        Ok(out)
    }
}

/// 现在能用的出图供应商,按「生成时优先用谁」排好序。
fn providers(ctx: &AppCtx) -> Result<Vec<Box<dyn ImageProvider>>, String> {
    let cfg = ctx.config();
    if cfg.demo_mode {
        return Ok(vec![Box::new(DemoPainter)]);
    }
    let client = pp_providers::http::build_client(None).map_err(provider_err)?;
    let mut list: Vec<Box<dyn ImageProvider>> = Vec::new();
    if let Some(key) = key_of(ctx, ProviderId::Minimax)? {
        let base = cfg.minimax_base_url.clone().unwrap_or_else(|| MiniMaxImage::DEFAULT_BASE.to_string());
        list.push(Box::new(MiniMaxImage::new(client.clone(), &base, &key).map_err(provider_err)?));
    }
    if let Some(key) = key_of(ctx, ProviderId::QwenImage)? {
        let base = cfg.qwen_base_url.clone().unwrap_or_else(|| QwenImage::DEFAULT_BASE.to_string());
        list.push(Box::new(QwenImage::new(client, &base, &key).map_err(provider_err)?));
    }
    // 用户指定了偏好的那家排最前(生成时用它);没指定 = 便宜的在前
    let preferred = cfg.image_provider.as_str();
    list.sort_by_key(|p| (p.name() != preferred, p.price_fen() as i64));
    Ok(list)
}

fn pick<'a>(list: &'a [Box<dyn ImageProvider>], need_edit: bool) -> Result<&'a dyn ImageProvider, String> {
    list.iter()
        .map(|p| p.as_ref())
        .find(|p| !need_edit || p.can_edit())
        .ok_or_else(|| provider_err(ProviderError::MissingKey))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn image_provider_info(ctx: State<'_, Arc<AppCtx>>) -> Result<ImageProviderInfo, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let list = providers(&ctx)?;
        let planner_ready = ctx.config().demo_mode || key_of(&ctx, ProviderId::Deepseek)?.is_some();
        Ok(match list.first() {
            None => ImageProviderInfo {
                planner_ready,
                ..Default::default()
            },
            Some(first) => ImageProviderInfo {
                available: true,
                name: list.iter().map(|p| p.name()).collect::<Vec<_>>().join(" + "),
                model: first.model(),
                can_edit: list.iter().any(|p| p.can_edit()),
                price_fen: first.price_fen(),
                planner_ready,
            },
        })
    })
    .await
    .map_err(join_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn image_settings_get(ctx: State<'_, Arc<AppCtx>>) -> Result<ImageSettings, String> {
    let cfg = ctx.config();
    Ok(ImageSettings {
        provider: if cfg.image_provider.is_empty() { "auto".into() } else { cfg.image_provider },
        minimax_base_url: cfg.minimax_base_url.unwrap_or_else(|| MiniMaxImage::DEFAULT_BASE.to_string()),
        qwen_base_url: cfg.qwen_base_url.unwrap_or_else(|| QwenImage::DEFAULT_BASE.to_string()),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn image_settings_set(ctx: State<'_, Arc<AppCtx>>, settings: ImageSettings) -> Result<(), String> {
    let clean = |url: &str, default: &str| -> Result<Option<String>, String> {
        let url = url.trim().trim_end_matches('/');
        if url.is_empty() || url == default {
            return Ok(None);
        }
        // 这是用户填错了地址,不是接口出了问题:归到「输入不合法」,话也说成给用户看的
        pp_providers::http::check_base_url(url).map_err(|_| invalid(format!("endpoint must start with https:// — {url}")))?;
        Ok(Some(url.to_string()))
    };
    let minimax = clean(&settings.minimax_base_url, MiniMaxImage::DEFAULT_BASE)?;
    let qwen = clean(&settings.qwen_base_url, QwenImage::DEFAULT_BASE)?;
    let provider = match settings.provider.as_str() {
        "minimax" | "qwen" => settings.provider.clone(),
        _ => String::new(),
    };
    ctx.update_config(|c| {
        c.image_provider = provider;
        c.minimax_base_url = minimax;
        c.qwen_base_url = qwen;
    })
    .map_err(|e| errcode::err(errcode::IO_FAILED, e))?;
    Ok(())
}

// ---------------------------------------------------------------- 画板

#[tauri::command(rename_all = "snake_case")]
pub async fn board_list(ctx: State<'_, Arc<AppCtx>>) -> Result<Vec<ImageBoardSummary>, String> {
    db(ctx.db.list_boards())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn board_create(
    ctx: State<'_, Arc<AppCtx>>,
    purpose: ImagePurpose,
    project_id: Option<String>,
    name: Option<String>,
) -> Result<ImageBoard, String> {
    // 从项目里发起的画板直接用项目的名字;没给名字的,第一句话之后自动起名
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| DEFAULT_NAME.to_string());
    db(ctx.db.create_board(&name, purpose, project_id.as_deref()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn board_get(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<ImageBoardDetail, String> {
    Ok(ImageBoardDetail {
        board: db(ctx.db.get_board(&id))?,
        messages: db(ctx.db.list_image_messages(&id))?,
        versions: db(ctx.db.list_image_versions(&id))?,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn board_rename(ctx: State<'_, Arc<AppCtx>>, id: String, name: String) -> Result<ImageBoard, String> {
    db(ctx.db.rename_board(&id, &name))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn board_delete(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    db(ctx.db.delete_board(&id))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn board_set_options(ctx: State<'_, Arc<AppCtx>>, id: String, purpose: ImagePurpose, aspect: ImageAspect) -> Result<ImageBoard, String> {
    db(ctx.db.set_board_options(&id, purpose, aspect))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn board_link_project(ctx: State<'_, Arc<AppCtx>>, id: String, project_id: Option<String>) -> Result<ImageBoard, String> {
    if let Some(pid) = &project_id {
        db(ctx.db.get_project(pid))?;
    }
    db(ctx.db.link_board_project(&id, project_id.as_deref()))
}

/// 选中一张图:之后说的「改一下」改的就是它。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_select(ctx: State<'_, Arc<AppCtx>>, id: String, version_id: String) -> Result<ImageBoard, String> {
    db(ctx.db.set_board_current(&id, Some(&version_id)))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn board_adopt(ctx: State<'_, Arc<AppCtx>>, version_id: String, adopted: bool) -> Result<ImageVersion, String> {
    db(ctx.db.set_image_adopted(&version_id, adopted))
}

/// 从画板上拿掉一张图(图片文件仍留在资料库里)。返回更新后的画板——当前选中的图可能因此换了一张。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_remove_image(ctx: State<'_, Arc<AppCtx>>, version_id: String) -> Result<ImageBoard, String> {
    db(ctx.db.remove_image_version(&version_id))
}

/// 贴一张图(缩小、去 EXIF——它会被发给第三方模型)。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_add_image(ctx: State<'_, Arc<AppCtx>>, id: String, path: String) -> Result<Asset, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let board = db(ctx.db.get_board(&id))?;
        let src = Path::new(&path);
        if std::fs::metadata(src).map_err(fs_err)?.len() > 64 * 1024 * 1024 {
            return Err(invalid("image is larger than 64 MB"));
        }
        let (jpeg, w, h) = prepare_reference(&std::fs::read(src).map_err(fs_err)?)?;
        store_image(&ctx, &board, NewImage::reference(&jpeg, (w, h)))
    })
    .await
    .map_err(join_err)?
}

/// 要存进资料库的一张图。
struct NewImage<'a> {
    bytes: &'a [u8],
    ext: &'a str,
    role: &'a str,
    ai_generated: bool,
    parent_asset: Option<&'a str>,
    /// 不给就从字节里读
    size: Option<(u32, u32)>,
    /// 导入的图:原来叫什么(记一笔,方便以后认出来)
    original_name: Option<&'a str>,
}

impl<'a> NewImage<'a> {
    /// 贴在一句话上的参考图(已经缩小、去过 EXIF 的 JPEG)
    fn reference(jpeg: &'a [u8], size: (u32, u32)) -> Self {
        Self {
            bytes: jpeg,
            ext: "jpg",
            role: "image_reference",
            ai_generated: false,
            parent_asset: None,
            size: Some(size),
            original_name: None,
        }
    }
}

fn store_image(ctx: &AppCtx, board: &ImageBoard, img: NewImage<'_>) -> Result<Asset, String> {
    let ext = img.ext;
    let mut new = NewAsset::new(AssetKind::Image, img.role, "", ext, img.bytes.len() as i64);
    new.rel_path = format!("assets/{}/{}.{ext}", folder_for(ctx, board.project_id.as_deref())?, new.id);
    new.project_id = board.project_id.clone();
    new.parent_asset_id = img.parent_asset.map(str::to_string);
    new.ai_generated = img.ai_generated;
    let size = img.size.or_else(|| {
        image::ImageReader::new(std::io::Cursor::new(img.bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok())
    });
    let mut meta = serde_json::Map::new();
    if let Some((w, h)) = size {
        meta.insert("width".into(), json!(w));
        meta.insert("height".into(), json!(h));
    }
    if let Some(name) = img.original_name.filter(|n| !n.is_empty()) {
        meta.insert("original_name".into(), json!(name));
    }
    new.meta_json = (!meta.is_empty()).then(|| serde_json::Value::Object(meta).to_string());
    let dest = ctx.asset_path(&new.rel_path);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(fs_err)?;
    }
    std::fs::write(&dest, img.bytes).map_err(fs_err)?;
    db(ctx.db.insert_asset(&new))
}

// ---------------------------------------------------------------- 导入自己的图

/// 导入的图入库时缩到的最长边:够发布图(1080 × 1440)裁切放大用,又不至于把几十 MB 的原片原样塞进资料库
const IMPORT_MAX_SIDE: u32 = 4096;
const IMPORT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// 一张要导入的图是从哪来的。
enum ImportSource {
    File(PathBuf),
    /// 剪贴板里的位图(截图、浏览器里「复制图片」):没有文件,只有像素
    Pixels(image::DynamicImage),
}

impl ImportSource {
    fn name(&self) -> String {
        match self {
            ImportSource::File(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string()),
            ImportSource::Pixels(_) => String::new(),
        }
    }

    fn load(self) -> Result<image::DynamicImage, String> {
        let path = match self {
            ImportSource::Pixels(img) => return Ok(opaque_if_blank_alpha(img)),
            ImportSource::File(path) => path,
        };
        let meta = std::fs::metadata(&path).map_err(fs_err)?;
        if meta.is_dir() {
            return Err(errcode::err(errcode::IMAGE_UNSUPPORTED, "a folder"));
        }
        if !is_importable_image(&path.to_string_lossy()) {
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            return Err(errcode::err(errcode::IMAGE_UNSUPPORTED, format!(".{ext}")));
        }
        if meta.len() > IMPORT_MAX_BYTES {
            return Err(invalid("image is larger than 64 MB"));
        }
        decode_upright(&std::fs::read(&path).map_err(fs_err)?)
    }
}

/// 剪贴板里的位图经常不填 alpha(整张都是 0):那不是「全透明」,是来源根本没有透明这回事。
fn opaque_if_blank_alpha(img: image::DynamicImage) -> image::DynamicImage {
    if !img.color().has_alpha() {
        return img;
    }
    let mut rgba = img.into_rgba8();
    if rgba.pixels().all(|p| p[3] == 0) {
        rgba.pixels_mut().for_each(|p| p[3] = 255);
    }
    image::DynamicImage::ImageRgba8(rgba)
}

/// 导入的图入库前的处理 → `(字节, 扩展名, 宽, 高)`。
/// 已经按 EXIF 摆正;重新编码一遍 = 去掉元数据(手机照片里有拍摄地点和机型,这张图之后会发给模型、会发到平台上);太大就缩。
/// 真有透明的存 PNG(抠好的图不能被铺白之后再也抠不回来),其余存 JPEG。
fn prepare_import(mut img: image::DynamicImage) -> Result<(Vec<u8>, &'static str, u32, u32), String> {
    if img.width() == 0 || img.height() == 0 {
        return Err(errcode::err(errcode::IMAGE_UNREADABLE, "empty image"));
    }
    if img.width().max(img.height()) > IMPORT_MAX_SIDE {
        img = img.resize(IMPORT_MAX_SIDE, IMPORT_MAX_SIDE, image::imageops::FilterType::Lanczos3);
    }
    let (w, h) = (img.width(), img.height());
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        if rgba.pixels().any(|p| p[3] < 255) {
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(rgba)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?;
            return Ok((png, "png", w, h));
        }
    }
    Ok((flatten_to_jpeg(&img, 92)?, "jpg", w, h))
}

/// 把这些图导入到画板上:每张都成为画板上的一张图,和生成的图同等地位(能选中、采用、导出、送去建模、让 AI 接着改)。
/// 能导入的照常导入,导不进来的逐个说明;第一张成为当前选中的图;时间线上记一条「导入了这几张」。
fn import_into_board(ctx: &AppCtx, id: &str, sources: Vec<ImportSource>) -> Result<ImageImportResult, String> {
    let board = db(ctx.db.get_board(id))?;
    if sources.is_empty() {
        return Err(invalid("nothing to import"));
    }
    let mut failed = Vec::new();
    let mut ready = Vec::new();
    for (i, source) in sources.into_iter().enumerate() {
        let name = source.name();
        if i >= IMPORT_MAX_FILES {
            failed.push(ImportFailure {
                name,
                reason: invalid(format!("at most {IMPORT_MAX_FILES} pictures at a time")),
            });
            continue;
        }
        match source.load().and_then(prepare_import) {
            Ok(prepared) => ready.push((name, prepared)),
            Err(reason) => failed.push(ImportFailure { name, reason }),
        }
    }

    // 倒着入库:胶片条按时间倒序排,这样第一个文件排在最前面
    let mut versions = Vec::new();
    for (name, (bytes, ext, w, h)) in ready.iter().rev() {
        let asset = store_image(
            ctx,
            &board,
            NewImage {
                bytes,
                ext,
                role: "studio_import",
                ai_generated: false,
                parent_asset: None,
                size: Some((*w, *h)),
                original_name: Some(name),
            },
        )?;
        versions.push(db(ctx.db.insert_image_version(&NewImageVersion {
            board_id: id.to_string(),
            parent_id: None,
            asset_id: asset.id,
            prompt: String::new(),
            mode: "import".into(),
            provider: String::new(),
            model: String::new(),
            aspect: ImageAspect::nearest(*w, *h),
        }))?);
    }
    versions.reverse();

    let mut message = None;
    if let Some(first) = versions.first() {
        db(ctx.db.set_board_current(id, Some(&first.id)))?;
        message = Some(db(ctx.db.insert_image_message(&NewImageMessage {
            board_id: id.to_string(),
            from_user: true,
            kind: ImageMsgKind::Images,
            content: String::new(),
            extra: ImageMsgExtra {
                mode: Some("import".into()),
                version_ids: versions.iter().map(|v| v.id.clone()).collect(),
                ..Default::default()
            },
            image_asset_ids: Vec::new(),
        }))?);
    }
    log::info!("[imagery] 「{}」导入 {} 张(没导入成 {} 张)", board.name, versions.len(), failed.len());
    Ok(ImageImportResult {
        board: db(ctx.db.get_board(id))?,
        message,
        versions,
        failed,
    })
}

/// 剪贴板里能导入的东西:复制的图片文件优先(有文件名,也没被剪贴板转成位图),其次是位图。
fn clipboard_sources() -> Result<Vec<ImportSource>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| errcode::err(errcode::IO_FAILED, e))?;
    if let Ok(files) = clipboard.get().file_list() {
        let pictures: Vec<PathBuf> = files.into_iter().filter(|p| is_importable_image(&p.to_string_lossy())).collect();
        if !pictures.is_empty() {
            return Ok(pictures.into_iter().map(ImportSource::File).collect());
        }
    }
    let data = clipboard.get_image().map_err(|_| errcode::err(errcode::CLIPBOARD_NO_IMAGE, ""))?;
    let rgba = image::RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
        .ok_or_else(|| errcode::err(errcode::IMAGE_UNREADABLE, "clipboard image has an inconsistent size"))?;
    Ok(vec![ImportSource::Pixels(image::DynamicImage::ImageRgba8(rgba))])
}

/// 导入本地图片(「导入图片」按钮选的、或者拖进窗口的)。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_import_images(ctx: State<'_, Arc<AppCtx>>, id: String, paths: Vec<String>) -> Result<ImageImportResult, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let sources = paths.into_iter().map(|p| ImportSource::File(PathBuf::from(p))).collect();
        import_into_board(&ctx, &id, sources)
    })
    .await
    .map_err(join_err)?
}

/// 把剪贴板里的图导入到画板上(截图、浏览器里「复制图片」、资源管理器里复制的图片文件)。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_paste_images(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<ImageImportResult, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || import_into_board(&ctx, &id, clipboard_sources()?))
        .await
        .map_err(join_err)?
}

/// 把剪贴板里的图贴到这句话上当参考(在输入框里按 Ctrl+V)。和「贴一张图」一样缩小、去 EXIF;最多要 `max` 张。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_paste_reference(ctx: State<'_, Arc<AppCtx>>, id: String, max: u32) -> Result<Vec<Asset>, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let board = db(ctx.db.get_board(&id))?;
        let mut out = Vec::new();
        for source in clipboard_sources()?.into_iter().take(max.min(MAX_TURN_IMAGES as u32) as usize) {
            let (jpeg, w, h) = reference_jpeg(source.load()?)?;
            out.push(store_image(&ctx, &board, NewImage::reference(&jpeg, (w, h)))?);
        }
        Ok(out)
    })
    .await
    .map_err(join_err)?
}

// ---------------------------------------------------------------- 一轮里用到的图

/// 一轮最多涉及几张图(当前选中的 + 这句话里贴的)。通义千问的编辑接口一次最多收 3 张。
const MAX_TURN_IMAGES: usize = 3;
/// 给规划模型看的图缩到多大
const PLANNER_MAX_SIDE: u32 = 1024;
/// 给图片模型改的原图超过这么大才缩(接口对请求体有上限,而且 base64 还要再涨三分之一)
const EDIT_INPUT_MAX_BYTES: usize = 7 * 1024 * 1024;
const EDIT_INPUT_MAX_SIDE: u32 = 2048;

fn flatten_to_jpeg(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    // JPEG 没有透明通道:直接丢掉 alpha 会让透明底变成一片黑
    let rgba = img.to_rgba8();
    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
        let a = src[3] as u32;
        for c in 0..3 {
            dst[c] = ((src[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&rgb)
        .map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?;
    Ok(out)
}

/// 给规划模型「看」的图:缩到最长边 1024 的 JPEG。它只需要看清画面,不需要像素;
/// 图片模型出的原图动辄几 MB,每一轮都原样发过去既慢又贵。解不开的图原样给(让模型自己去报错)。
fn planner_view(bytes: &[u8], ext: &str) -> String {
    let small = image::load_from_memory(bytes).ok().and_then(|img| {
        let img = if img.width().max(img.height()) > PLANNER_MAX_SIDE {
            img.resize(PLANNER_MAX_SIDE, PLANNER_MAX_SIDE, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        flatten_to_jpeg(&img, 85).ok()
    });
    match small {
        Some(jpeg) => image_data_url(&jpeg, "jpg"),
        None => image_data_url(bytes, ext),
    }
}

/// 给图片模型「改」的图:**尽量原样**——每改一轮都重压一遍 JPEG 的话,图会越改越糊。只有太大才缩一下。
fn edit_input(bytes: &[u8], ext: &str) -> Result<String, String> {
    if bytes.len() <= EDIT_INPUT_MAX_BYTES {
        return Ok(image_data_url(bytes, ext));
    }
    let img = image::load_from_memory(bytes).map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?;
    let img = img.resize(EDIT_INPUT_MAX_SIDE, EDIT_INPUT_MAX_SIDE, image::imageops::FilterType::Lanczos3);
    Ok(image_data_url(&flatten_to_jpeg(&img, 92)?, "jpg"))
}

/// 这一轮涉及的图 → (给规划模型看的, 给图片模型改的),两份一一对应。
fn turn_images(ctx: &AppCtx, asset_ids: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    let (mut looks, mut inputs) = (Vec::new(), Vec::new());
    for id in asset_ids {
        let asset = db(ctx.db.get_asset(id))?;
        if !matches!(asset.kind, AssetKind::Image | AssetKind::Photo) {
            return Err(invalid(format!("asset {id} is not an image")));
        }
        let bytes = std::fs::read(ctx.asset_path(&asset.rel_path)).map_err(fs_err)?;
        looks.push(planner_view(&bytes, &asset.ext));
        inputs.push(edit_input(&bytes, &asset.ext)?);
    }
    Ok((looks, inputs))
}

/// 导出时要不要转码:按用户起的文件名的扩展名来。`None` = 原样拷贝。
fn export_bytes(bytes: &[u8], have: &str, want: &str) -> Result<Option<Vec<u8>>, String> {
    let norm = |e: &str| match e.to_ascii_lowercase().as_str() {
        "jpeg" => "jpg".to_string(),
        other => other.to_string(),
    };
    let (have, want) = (norm(have), norm(want));
    if want.is_empty() || want == have {
        return Ok(None);
    }
    let img = image::load_from_memory(bytes).map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?;
    let mut out = Vec::new();
    match want.as_str() {
        "jpg" => out = flatten_to_jpeg(&img, 92)?,
        "png" => img
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?,
        "webp" => img
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::WebP)
            .map_err(|e| errcode::err(errcode::IMAGE_UNREADABLE, e))?,
        other => return Err(invalid(format!("cannot export as .{other}; use jpg, png or webp"))),
    }
    Ok(Some(out))
}

// ---------------------------------------------------------------- 对话

/// 演示模式的规划:不理解话的内容,按「是不是问句 / 有没有选中的图」走三条固定的路。
fn demo_plan(text: &str, has_current: bool, count: u32) -> String {
    let t = text.trim();
    let is_question = t.ends_with('?') || t.ends_with('?') || t.contains('吗') || t.contains("哪张");
    if is_question {
        json!({ "action": "reply", "prompt": "", "reply": "演示数据:当前选中的这张更适合——主体完整、背景干净。真实模式下我会真的看图再回答。" }).to_string()
    } else if has_current {
        json!({
            "action": "edit",
            "prompt": format!("{t}。只改背景,主体的形状、颜色、位置和视角保持不变。"),
            "count": 1,
            "reply": "演示数据:只换背景,主体像素保持不变(演示用的本机「画师」只会这一种改法)。"
        })
        .to_string()
    } else {
        json!({
            "action": "generate",
            "prompt": format!("{t}。纯白无缝背景,单个物体完整入画,约 30° 俯视的 3/4 视角,柔和均匀的棚拍光,哑光 PLA 质感。"),
            "count": count,
            "reply": format!("演示数据:按白底 3/4 视角出 {count} 张(本机画的示意图,不是真的图片模型)。")
        })
        .to_string()
    }
}

fn history_lines(messages: &[pp_common::imagery::ImageMessage]) -> Vec<ChatLine> {
    messages
        .iter()
        .filter(|m| !m.content.trim().is_empty())
        .map(|m| ChatLine {
            from_user: m.from_user,
            text: m.content.clone(),
        })
        .collect()
}

/// 对话里的一轮:用户说一句话(可带图)。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_send(
    app: AppHandle,
    ctx: State<'_, Arc<AppCtx>>,
    id: String,
    text: String,
    image_asset_ids: Vec<String>,
    count: u32,
) -> Result<ImageTurnResult, String> {
    let text = text.trim().to_string();
    if text.is_empty() && image_asset_ids.is_empty() {
        return Err(invalid("message is empty"));
    }
    if image_asset_ids.len() > MAX_TURN_IMAGES {
        return Err(invalid(format!("at most {MAX_TURN_IMAGES} images per message")));
    }
    let started = Instant::now();
    let ctx = ctx.inner().clone();
    let demo = ctx.config().demo_mode;
    let board = db(ctx.db.get_board(&id))?;
    // 这一轮可以中途取消:规划和出图两段包在 `turn_guard.run` 里;落盘入库那一段不包(见 turns.rs)。
    // 注意:出图请求发出去之后再取消,供应商那边多半已经在画了——那几张图照样计费,只是我们不要了
    let turn_guard = ctx.turns.begin(&format!("board:{id}"))?;
    let timeline = db(ctx.db.list_image_messages(&id))?;
    let current = board.current_image_id.as_deref().map(|v| db(ctx.db.get_image_version(v))).transpose()?;

    let list = {
        let ctx = ctx.clone();
        tauri::async_runtime::spawn_blocking(move || providers(&ctx)).await.map_err(join_err)??
    };
    if list.is_empty() {
        return Err(provider_err(ProviderError::MissingKey));
    }
    let can_edit = list.iter().any(|p| p.can_edit());
    let max_count = list.iter().map(|p| p.max_count()).min().unwrap_or(1);

    // 这一轮涉及的图:当前选中的图在前,这句话里新贴的在后。
    // 同一批图准备两份:给规划模型「看」的缩小版,和给图片模型「改」的原图。
    let mut turn_ids: Vec<String> = current.iter().map(|v| v.asset_id.clone()).collect();
    turn_ids.extend(image_asset_ids.iter().cloned());
    turn_ids.truncate(MAX_TURN_IMAGES);
    let (looks, inputs) = {
        let (ctx, ids) = (ctx.clone(), turn_ids);
        tauri::async_runtime::spawn_blocking(move || turn_images(&ctx, &ids)).await.map_err(join_err)??
    };

    // ---- 1. 规划 ----
    emit_progress(&app, &id, "planning", "", 0);
    let mut plan_cfg = ImagePlanConfig::default();
    if demo {
        plan_cfg.pricing = LlmPricing {
            input_cache_hit: 0.0,
            input_cache_miss: 0.0,
            output: 0.0,
        };
    }
    let llm = llm_or_demo(&ctx, vec![demo_plan(&text, current.is_some(), count.clamp(1, max_count))])?;
    let history = history_lines(&timeline);
    let image_turn = ImageTurn {
        history: &history,
        message: if text.is_empty() { "(见图)" } else { &text },
        purpose: board.purpose,
        aspect: board.aspect,
        can_edit,
        max_count,
        default_count: count,
        // 导入的图没有提示词(空串):给 None,规划那边会说明「这是用户自己导入的图」
        current_prompt: current.as_ref().map(|v| v.prompt.as_str()).filter(|p| !p.trim().is_empty()),
        images: &looks,
        has_current: current.is_some(),
    };
    let plan = turn_guard
        .run(async { plan_image_turn(llm.as_ref(), image_turn, &plan_cfg).await.map_err(agent_err) })
        .await?;

    // ---- 2. 出图 ----
    let mut report = ImageTurnReport {
        llm_calls: plan.llm_calls,
        cost_fen: plan.cost_fen,
        ..Default::default()
    };
    let mut produced: Vec<ImageBytes> = Vec::new();
    let is_edit = plan.action == ImageAction::Edit;
    if plan.action != ImageAction::Reply {
        let provider = pick(&list, is_edit)?;
        let request = ImageRequest {
            prompt: plan.prompt.clone(),
            aspect: board.aspect,
            count: plan.count.min(provider.max_count()),
            // 编辑的输入:当前图 + 新贴的图(规划时看的就是这几张,只是这里给的是原图)
            images: if is_edit { inputs } else { Vec::new() },
        };
        emit_progress(&app, &id, "rendering", provider.name(), request.count);
        produced = turn_guard.run(async { provider.generate(&request).await.map_err(provider_err) }).await?;
        emit_progress(&app, &id, "saving", provider.name(), produced.len() as u32);
        report.images = produced.len() as u32;
        report.cost_fen += provider.price_fen() * produced.len() as f64;
        report.provider = provider.name().to_string();
        report.model = provider.model();
    }
    report.elapsed_ms = started.elapsed().as_millis() as u64;

    // ---- 3. 落盘 + 入库(到这里为止什么都还没写;从这里开始整轮一起写)----
    tauri::async_runtime::spawn_blocking(move || {
        let mut versions = Vec::new();
        let parent = if is_edit { current.as_ref() } else { None };
        for img in &produced {
            let asset = store_image(
                &ctx,
                &board,
                NewImage {
                    bytes: &img.bytes,
                    ext: &img.ext,
                    role: "studio_image",
                    ai_generated: true,
                    parent_asset: parent.map(|p| p.asset_id.as_str()),
                    size: None,
                    original_name: None,
                },
            )?;
            versions.push(db(ctx.db.insert_image_version(&NewImageVersion {
                board_id: id.clone(),
                parent_id: parent.map(|p| p.id.clone()),
                asset_id: asset.id,
                prompt: plan.prompt.clone(),
                mode: if is_edit { "edit".into() } else { "generate".into() },
                provider: report.provider.clone(),
                model: report.model.clone(),
                aspect: board.aspect,
            }))?);
        }
        if let Some(first) = versions.first() {
            db(ctx.db.set_board_current(&id, Some(&first.id)))?;
        }
        if !image_asset_ids.is_empty() {
            let mut refs = board.ref_asset_ids.clone();
            refs.extend(image_asset_ids.iter().cloned());
            refs.dedup();
            let drop = refs.len().saturating_sub(MAX_REFERENCE_IMAGES);
            refs.drain(..drop);
            db(ctx.db.set_board_refs(&id, &refs))?;
        }
        if board.name == DEFAULT_NAME && !text.is_empty() {
            let name: String = text.chars().take(14).collect();
            db(ctx.db.rename_board(&id, &name))?;
        }
        let _ = ctx.db.add_cost(board.project_id.as_deref(), "image", report.cost_fen, &format!("{} · 出图", board.name));
        log::info!(
            "[imagery] 「{}」一轮:{:?} · {} 张 · {} · {:.2} 分 · {}ms",
            board.name,
            plan.action,
            report.images,
            report.provider,
            report.cost_fen,
            report.elapsed_ms
        );

        let user_msg = NewImageMessage {
            board_id: id.clone(),
            from_user: true,
            kind: ImageMsgKind::Text,
            content: text.clone(),
            extra: ImageMsgExtra::default(),
            image_asset_ids,
        };
        let mut extra = ImageMsgExtra {
            report: Some(report),
            ..Default::default()
        };
        let kind = if versions.is_empty() {
            ImageMsgKind::Text
        } else {
            extra.prompt = Some(plan.prompt.clone());
            extra.mode = Some(if is_edit { "edit".into() } else { "generate".into() });
            extra.version_ids = versions.iter().map(|v| v.id.clone()).collect();
            extra.base_version_id = parent.map(|p| p.id.clone());
            ImageMsgKind::Images
        };
        let reply = NewImageMessage {
            board_id: id.clone(),
            from_user: false,
            kind,
            content: plan.reply.clone(),
            extra,
            image_asset_ids: Vec::new(),
        };
        let messages = vec![db(ctx.db.insert_image_message(&user_msg))?, db(ctx.db.insert_image_message(&reply))?];
        Ok(ImageTurnResult {
            board: db(ctx.db.get_board(&id))?,
            messages,
            versions,
        })
    })
    .await
    .map_err(join_err)?
}

// ---------------------------------------------------------------- 去向

/// 送去建模:用这张图新建一个「设计」,它就是参考图。返回新设计,前端跳到「建模」打开它。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_to_design(ctx: State<'_, Arc<AppCtx>>, version_id: String) -> Result<CadDesign, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let version = db(ctx.db.get_image_version(&version_id))?;
        let board = db(ctx.db.get_board(&version.board_id))?;
        let reference = design_reference(&ctx, &board, &version.asset_id)?;
        let design = db(ctx.db.create_design(&board.name, board.project_id.as_deref()))?;
        let design = db(ctx.db.set_design_refs(&design.id, &[reference]))?;
        let _ = ctx.db.set_image_adopted(&version.id, true);
        log::info!("[imagery] 「{}」的一张图 → 新设计 {}", board.name, design.id);
        Ok(design)
    })
    .await
    .map_err(join_err)?
}

/// 建模时参考图是原样发给视觉模型的:导入的原片(最长边可以到 4096)和带透明的 PNG 先做成一张参考图
/// (缩小、铺白、JPEG),和「贴参考图」入库时的处理一样;本来就不大的图直接用。
fn design_reference(ctx: &AppCtx, board: &ImageBoard, asset_id: &str) -> Result<String, String> {
    let asset = db(ctx.db.get_asset(asset_id))?;
    let bytes = std::fs::read(ctx.asset_path(&asset.rel_path)).map_err(fs_err)?;
    let longest = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .map(|(w, h)| w.max(h));
    let small_jpeg = asset.ext.eq_ignore_ascii_case("jpg") && longest.is_some_and(|side| side <= REFERENCE_MAX_SIDE);
    if small_jpeg {
        return Ok(asset.id);
    }
    let (jpeg, w, h) = prepare_reference(&bytes)?;
    let mut reference = NewImage::reference(&jpeg, (w, h));
    reference.ai_generated = asset.ai_generated;
    reference.parent_asset = Some(&asset.id);
    Ok(store_image(ctx, board, reference)?.id)
}

/// 导出到用户选的位置。文件名的扩展名和原图不一样时转码(通义千问出的是 PNG,平台上传一般要 JPG)。
#[tauri::command(rename_all = "snake_case")]
pub async fn board_export(ctx: State<'_, Arc<AppCtx>>, version_id: String, dest_path: String) -> Result<(), String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let version = db(ctx.db.get_image_version(&version_id))?;
        let asset = db(ctx.db.get_asset(&version.asset_id))?;
        let src = ctx.asset_path(&asset.rel_path);
        let want = Path::new(&dest_path).extension().and_then(|e| e.to_str()).unwrap_or("");
        match export_bytes(&std::fs::read(&src).map_err(fs_err)?, &asset.ext, want)? {
            Some(converted) => std::fs::write(&dest_path, converted).map_err(fs_err)?,
            None => {
                std::fs::copy(&src, &dest_path).map_err(fs_err)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(join_err)?
}

/// 「测试连接」用:出一张最便宜的图,丢掉。会产生一张图的费用。
pub(crate) async fn test_image_provider(ctx: &AppCtx, id: ProviderId) -> Result<(), String> {
    let key = key_of(ctx, id)?.ok_or_else(|| provider_err(ProviderError::MissingKey))?;
    let cfg = ctx.config();
    let client = pp_providers::http::build_client(None).map_err(provider_err)?;
    let provider: Box<dyn ImageProvider> = match id {
        ProviderId::Minimax => Box::new(
            MiniMaxImage::new(client, &cfg.minimax_base_url.unwrap_or_else(|| MiniMaxImage::DEFAULT_BASE.into()), &key).map_err(provider_err)?,
        ),
        _ => Box::new(QwenImage::new(client, &cfg.qwen_base_url.unwrap_or_else(|| QwenImage::DEFAULT_BASE.into()), &key).map_err(provider_err)?),
    };
    let req = ImageRequest {
        prompt: "纯白背景上的一个蓝色立方体".into(),
        aspect: ImageAspect::Square,
        count: 1,
        images: Vec::new(),
    };
    provider.generate(&req).await.map(|_| ()).map_err(provider_err)
}

#[cfg(test)]
mod tests {
    use pp_providers::image::sniff_ext;

    use super::*;

    fn data_url(img: &image::RgbImage) -> String {
        image_data_url(&encode_jpeg(img).unwrap(), "jpg")
    }

    #[tokio::test]
    async fn the_demo_painter_draws_and_its_edit_keeps_the_subject() {
        let painter = DemoPainter;
        let made = painter
            .generate(&ImageRequest {
                prompt: "白底,单个线缆夹".into(),
                aspect: ImageAspect::Portrait,
                count: 3,
                images: vec![],
            })
            .await
            .unwrap();
        assert_eq!(made.len(), 3);
        let first = image::load_from_memory(&made[0].bytes).unwrap().to_rgb8();
        assert_eq!(first.dimensions(), (720, 960), "按画幅出图");
        let center = *first.get_pixel(360, 560);
        assert!(center[2] > 150 && center[0] < 160, "中间是蓝色系的主体:{center:?}");
        assert!(first.get_pixel(5, 5)[0] > 240, "提示词里要了白底");
        assert_ne!(made[0].bytes, made[1].bytes, "一批里的几张要有差别");

        let edited = painter
            .generate(&ImageRequest {
                prompt: "背景换成木纹桌面".into(),
                aspect: ImageAspect::Portrait,
                count: 1,
                images: vec![data_url(&first)],
            })
            .await
            .unwrap();
        let after = image::load_from_memory(&edited[0].bytes).unwrap().to_rgb8();
        let (bg, subject) = (*after.get_pixel(5, 5), *after.get_pixel(360, 560));
        assert!(bg[0] > 200 && bg[2] < 190, "背景换成了木色:{bg:?}");
        assert!((0..3).all(|c| subject[c].abs_diff(center[c]) < 12), "主体像素保持不变:{subject:?} vs {center:?}");
    }

    fn decode(data_url: &str) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(data_url.split_once(',').unwrap().1).unwrap()
    }

    #[test]
    fn the_planner_sees_a_small_copy_but_the_editor_gets_the_original() {
        // 一张 2000×1000 的 PNG(图片模型出的原图大概是这个量级)
        let big = image::RgbImage::from_fn(2000, 1000, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, 128]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(big)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let look = planner_view(&png, "png");
        assert!(look.starts_with("data:image/jpeg;base64,"), "给规划模型看的是 JPEG");
        let seen = image::load_from_memory(&decode(&look)).unwrap();
        assert_eq!((seen.width(), seen.height()), (1024, 512), "缩到最长边 1024,比例不变");

        let input = edit_input(&png, "png").unwrap();
        assert!(input.starts_with("data:image/png;base64,"));
        assert_eq!(decode(&input), png, "给图片模型改的是原图的字节,一个像素都不重压");

        // 解不开的「图」:规划那份原样给,不在这里失败
        assert!(planner_view(b"not an image", "png").starts_with("data:image/png;base64,"));
    }

    #[test]
    fn exporting_converts_only_when_the_extension_differs() {
        // 带透明底的 PNG → JPG:透明的地方要铺白,不能变黑
        let mut rgba = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0]));
        rgba.put_pixel(4, 4, image::Rgba([200, 30, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        assert_eq!(export_bytes(&png, "png", "PNG").unwrap(), None, "同一种格式原样拷贝");
        assert_eq!(export_bytes(&png, "jpg", "jpeg").unwrap(), None, "jpeg 和 jpg 是一回事");
        assert_eq!(export_bytes(&png, "png", "").unwrap(), None, "没写扩展名就原样给");

        let jpg = export_bytes(&png, "png", "jpg").unwrap().expect("png → jpg 要转码");
        assert_eq!(sniff_ext(&jpg), "jpg");
        let flat = image::load_from_memory(&jpg).unwrap().to_rgb8();
        assert!(flat.get_pixel(0, 0)[0] > 240, "透明底铺成了白色:{:?}", flat.get_pixel(0, 0));

        let back = export_bytes(&jpg, "jpg", "png").unwrap().unwrap();
        assert_eq!(sniff_ext(&back), "png");
        assert!(export_bytes(&png, "png", "gif").is_err(), "不支持的格式要明说");
    }

    /// 一个临时的资料库(测试完删掉)。
    struct TempCtx {
        ctx: Option<AppCtx>,
        dir: PathBuf,
    }

    impl TempCtx {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pp-imagery-{tag}-{}", pp_db::new_id()));
            std::fs::create_dir_all(dir.join("library")).unwrap();
            let db = pp_db::Db::open(&dir.join("library").join("printpilot.db")).unwrap();
            let ctx = AppCtx::new(
                db,
                dir.join("library"),
                dir.join("data"),
                dir.join("config"),
                Default::default(),
                crate::config_manager::LoadState::Missing,
            );
            Self { ctx: Some(ctx), dir }
        }

        fn ctx(&self) -> &AppCtx {
            self.ctx.as_ref().unwrap()
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TempCtx {
        fn drop(&mut self) {
            // 先关库(Windows 上删不掉开着的文件),再删目录
            self.ctx = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn png_of(img: image::DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn opaque(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, 90])))
    }

    /// 左半边透明的图(抠好的产品图就是这个样子)
    fn cutout(w: u32, h: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, _| {
            if x < w / 2 {
                image::Rgba([0, 0, 0, 0])
            } else {
                image::Rgba([200, 30, 30, 255])
            }
        }))
    }

    #[test]
    fn importing_keeps_the_good_files_and_explains_the_bad_ones() {
        let t = TempCtx::new("import");
        let ctx = t.ctx();
        let board = ctx.db.create_board(DEFAULT_NAME, ImagePurpose::ModelRef, None).unwrap();
        std::fs::create_dir_all(t.dir.join("文件夹.png")).unwrap();
        let sources = vec![
            ImportSource::File(t.file("线缆夹实拍.png", &png_of(opaque(64, 48)))),
            ImportSource::File(t.file("抠好的.png", &png_of(cutout(40, 40)))),
            ImportSource::File(t.file("坏的.png", b"not an image at all")),
            ImportSource::File(t.file("IMG_0001.HEIC", b"ftypheic")),
            ImportSource::File(t.dir.join("文件夹.png")),
            ImportSource::File(t.dir.join("不存在.jpg")),
        ];
        let res = import_into_board(ctx, &board.id, sources).unwrap();

        assert_eq!(res.versions.len(), 2, "两张好的照常导入");
        let names: Vec<&str> = res.failed.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["坏的.png", "IMG_0001.HEIC", "文件夹.png", "不存在.jpg"]);
        let codes: Vec<&str> = res.failed.iter().map(|f| errcode::split(&f.reason).unwrap().0).collect();
        assert_eq!(codes, [errcode::IMAGE_UNREADABLE, errcode::IMAGE_UNSUPPORTED, errcode::IMAGE_UNSUPPORTED, errcode::IO_FAILED]);
        assert!(res.failed[1].reason.ends_with(".heic"), "说清是什么格式:{}", res.failed[1].reason);

        // 第一个文件排最前、成为当前图;重新从库里读出来也是这个顺序
        assert_eq!(res.board.current_image_id.as_deref(), Some(res.versions[0].id.as_str()));
        let listed: Vec<String> = ctx.db.list_image_versions(&board.id).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(listed, res.versions.iter().map(|v| v.id.clone()).collect::<Vec<_>>());
        assert!(res.versions.iter().all(|v| v.mode == "import" && v.prompt.is_empty() && v.parent_id.is_none() && !v.adopted));
        assert_eq!(res.versions[0].aspect, ImageAspect::Landscape, "64 × 48 最接近 4:3");

        let photo = ctx.db.get_asset(&res.versions[0].asset_id).unwrap();
        assert_eq!((photo.ext.as_str(), photo.role.as_str(), photo.ai_generated), ("jpg", "studio_import", false), "不透明的存 JPEG,不算 AI 生成");
        let meta: serde_json::Value = serde_json::from_str(photo.meta_json.as_deref().unwrap()).unwrap();
        assert_eq!((meta["width"].as_u64(), meta["height"].as_u64(), meta["original_name"].as_str()), (Some(64), Some(48), Some("线缆夹实拍.png")));
        assert_eq!(sniff_ext(&std::fs::read(ctx.asset_path(&photo.rel_path)).unwrap()), "jpg");

        let cut = ctx.db.get_asset(&res.versions[1].asset_id).unwrap();
        assert_eq!(cut.ext, "png", "真有透明的保留 PNG");
        let kept = image::load_from_memory(&std::fs::read(ctx.asset_path(&cut.rel_path)).unwrap()).unwrap().to_rgba8();
        assert_eq!((kept.get_pixel(2, 2)[3], kept.get_pixel(30, 2)[3]), (0, 255));

        // 时间线上记了一条「用户导入了这两张」;它没有文字,所以不会进给规划模型的历史
        let msg = res.message.expect("导入了就有一条记录");
        assert!(msg.from_user && msg.kind == ImageMsgKind::Images && msg.content.is_empty());
        assert_eq!(msg.extra.mode.as_deref(), Some("import"));
        assert_eq!(msg.extra.version_ids, listed);
        assert!(history_lines(&ctx.db.list_image_messages(&board.id).unwrap()).is_empty());
        assert_eq!(ctx.db.get_board(&board.id).unwrap().name, DEFAULT_NAME, "导入不给画板起名——第一句话才起");
    }

    #[test]
    fn an_import_with_nothing_usable_changes_nothing() {
        let t = TempCtx::new("import-none");
        let ctx = t.ctx();
        let board = ctx.db.create_board("夹子", ImagePurpose::ModelRef, None).unwrap();
        let res = import_into_board(ctx, &board.id, vec![ImportSource::File(t.file("说明.txt", b"hello"))]).unwrap();
        assert!(res.versions.is_empty() && res.message.is_none() && res.board.current_image_id.is_none());
        assert_eq!(res.failed.len(), 1);
        assert!(ctx.db.list_image_messages(&board.id).unwrap().is_empty());
        assert!(import_into_board(ctx, &board.id, Vec::new()).is_err(), "什么都没给要明说");

        // 一次太多:前 20 张导入,多出来的逐个说明
        let bytes = png_of(opaque(8, 8));
        let many: Vec<ImportSource> = (0..IMPORT_MAX_FILES + 2).map(|i| ImportSource::File(t.file(&format!("{i}.png"), &bytes))).collect();
        let res = import_into_board(ctx, &board.id, many).unwrap();
        assert_eq!((res.versions.len(), res.failed.len()), (IMPORT_MAX_FILES, 2));
        assert!(res.failed.iter().all(|f| f.reason.starts_with("#invalid_input#")));
    }

    #[test]
    fn imported_pictures_are_downsized_and_clipboard_bitmaps_are_not_see_through() {
        let (bytes, ext, w, h) = prepare_import(opaque(5000, 2500)).unwrap();
        assert_eq!((ext, w, h), ("jpg", 4096, 2048), "最长边缩到 4096,比例不变");
        let stored = image::load_from_memory(&bytes).unwrap();
        assert_eq!((stored.width(), stored.height()), (4096, 2048));

        // 剪贴板里的位图经常整张 alpha = 0:那是没填,不是全透明
        let blank = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(20, 10, image::Rgba([10, 120, 200, 0])));
        let (bytes, ext, ..) = prepare_import(ImportSource::Pixels(blank).load().unwrap()).unwrap();
        assert_eq!(ext, "jpg");
        let px = *image::load_from_memory(&bytes).unwrap().to_rgb8().get_pixel(5, 5);
        assert!(px[2] > 150 && px[0] < 60, "颜色还在,没有被铺成白色:{px:?}");
    }

    #[test]
    fn a_big_or_see_through_picture_goes_to_modeling_as_a_small_reference() {
        let t = TempCtx::new("to-design");
        let ctx = t.ctx();
        let board = ctx.db.create_board("夹子", ImagePurpose::ModelRef, None).unwrap();
        let sources = vec![
            ImportSource::File(t.file("大图.png", &png_of(opaque(3000, 2000)))),
            ImportSource::File(t.file("小图.png", &png_of(opaque(800, 600)))),
            ImportSource::File(t.file("抠图.png", &png_of(cutout(400, 400)))),
        ];
        let res = import_into_board(ctx, &board.id, sources).unwrap();
        let (big, small, cut) = (&res.versions[0].asset_id, &res.versions[1].asset_id, &res.versions[2].asset_id);

        assert_eq!(&design_reference(ctx, &res.board, small).unwrap(), small, "本来就不大的 JPEG 直接用");
        for (source, why) in [(big, "太大"), (cut, "带透明的 PNG")] {
            let id = design_reference(ctx, &res.board, source).unwrap();
            assert_ne!(&id, source, "{why}:另做一张参考图");
            let reference = ctx.db.get_asset(&id).unwrap();
            assert_eq!((reference.ext.as_str(), reference.role.as_str(), reference.parent_asset_id.as_deref()), ("jpg", "image_reference", Some(source.as_str())));
            let img = image::load_from_memory(&std::fs::read(ctx.asset_path(&reference.rel_path)).unwrap()).unwrap();
            assert!(img.width().max(img.height()) <= REFERENCE_MAX_SIDE);
        }
    }

    #[test]
    fn the_demo_plan_is_valid_input_for_the_real_planner() {
        for (text, has_current, want) in [("做一个线缆夹的白底图", false, "generate"), ("背景换成木桌面", true, "edit"), ("哪张更适合当主图?", true, "reply")] {
            let v: serde_json::Value = serde_json::from_str(&demo_plan(text, has_current, 4)).unwrap();
            assert_eq!(v["action"], want, "{text}");
            assert!(v["reply"].as_str().unwrap().contains("演示数据"), "演示数据必须自己说明自己是演示数据");
        }
    }

    #[test]
    fn generation_prefers_the_cheap_provider_and_edits_need_an_editor() {
        let client = pp_providers::http::build_client(None).unwrap();
        let list: Vec<Box<dyn ImageProvider>> = vec![
            Box::new(MiniMaxImage::new(client.clone(), MiniMaxImage::DEFAULT_BASE, "k").unwrap()),
            Box::new(QwenImage::new(client.clone(), QwenImage::DEFAULT_BASE, "k").unwrap()),
        ];
        assert_eq!(pick(&list, false).unwrap().name(), "minimax");
        assert_eq!(pick(&list, true).unwrap().name(), "qwen");

        let only_minimax: Vec<Box<dyn ImageProvider>> = vec![Box::new(MiniMaxImage::new(client, MiniMaxImage::DEFAULT_BASE, "k").unwrap())];
        assert!(pick(&only_minimax, true).err().is_some_and(|e| e.starts_with("#provider_missing_key#")));
        assert!(pick(&[], false).is_err());
    }
}
