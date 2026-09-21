//! 建模工作室的命令:设计(一个零件的一条建模线索)、和 AI 的持续对话、时间线。
//!
//! 对话是主线:
//! - 还没有模型时,用户说的话(可带图)→ 设计规格卡(给人审;继续说话 = 继续改规格)→ `design_generate` 出第一版;
//! - 有了模型之后,每句话交给 `pp_agent::chat_turn`:AI 自己判断是改模型(出新版本)、回答问题,还是反问澄清;
//! - 改参数、手改代码、看图复核也记在同一条时间线上。
//!
//! 约定:**一轮对话要么整轮入库,要么什么都不留**——模型调用失败(没密钥、断网)时不写半轮消息,
//! 用户的话还留在输入框里,改好配置再发一次即可。

use std::path::Path;
use std::sync::Arc;

use pp_agent::{chat_turn, describe_images, design_spec, generate_model, review_model, ChatLine, ChatOutcome, ChatTurn};
use pp_cad::{set_param, CadError, ParamError};
use pp_common::cad::{CadBuildReport, CadPick, CadVersion, DesignSpec};
use pp_common::design::{CadDesign, CadDesignDetail, CadDesignSummary, CadMessage, CadTier, CadTurnResult, MsgKind, MsgRole};
use pp_common::{errcode, Asset, AssetKind};
use pp_db::{NewAsset, NewCadMessage};
use pp_providers::llm::LlmPricing;
use serde_json::json;
use tauri::{AppHandle, State};

use super::cad::{
    agent_err, cad_config, cad_err, check_code, check_renders, demo_edit, emitter, fenced, folder_for, fs_err, invalid, llm_or_demo, persist,
    prepare_reference, reference_data_urls, EngineExecutor, Lineage, DEMO_CODE, DEMO_SPEC, MAX_REFERENCE_IMAGES,
};
use super::join_err;
use crate::AppCtx;

const DEFAULT_NAME: &str = "未命名设计";

fn db<T>(r: pp_db::DbResult<T>) -> Result<T, String> {
    r.map_err(|e| e.to_wire())
}

/// 档位 → 用哪个模型、开不开思考。看图那一步始终用视觉模型,不受档位影响。
fn config_for(tier: CadTier, demo: bool) -> pp_agent::CadConfig {
    let mut cfg = cad_config(demo);
    match tier {
        CadTier::Fast => {
            cfg.code_model = cfg.vision_model.clone();
            cfg.code_pricing = if demo { cfg.code_pricing } else { LlmPricing::deepseek_flash() };
            cfg.code_thinking = false;
            cfg.code_max_tokens = 8_000;
        }
        CadTier::Balanced => {
            cfg.code_thinking = false;
            cfg.code_max_tokens = 8_000;
        }
        CadTier::Precise => {}
    }
    cfg
}

fn add_report(total: &mut CadBuildReport, part: &CadBuildReport) {
    total.llm_calls += part.llm_calls;
    total.tokens_in += part.tokens_in;
    total.tokens_out += part.tokens_out;
    total.cost_fen += part.cost_fen;
}

/// 时间线 → 给模型的对话历史(只有文字)。手动改参数之类的事件也要让模型知道。
fn history_lines(messages: &[CadMessage]) -> Vec<ChatLine> {
    messages
        .iter()
        .filter_map(|m| {
            let text = match (m.role, m.kind) {
                (MsgRole::User, _) => m.content.clone(),
                (MsgRole::Assistant, MsgKind::Spec) => {
                    let name = m.extra.spec.as_ref().map(|s| s.name.as_str()).unwrap_or("");
                    format!("(给出了设计规格:{name})")
                }
                (MsgRole::Assistant, MsgKind::Build) if m.content.trim().is_empty() => "(已按规格生成模型)".to_string(),
                (MsgRole::Assistant, MsgKind::Failed) => "(这次修改没有成功,模型没有变)".to_string(),
                (MsgRole::Assistant, MsgKind::Review) => {
                    let diffs = m.extra.review.as_ref().map(|r| r.differences.join(";")).unwrap_or_default();
                    format!("(看图复核:{})", if diffs.is_empty() { "与参考图一致" } else { &diffs })
                }
                (MsgRole::Assistant, _) => m.content.clone(),
                (MsgRole::Event, MsgKind::Param) => format!("(我手动改了参数:{})", m.content),
                (MsgRole::Event, _) => "(我手动改了代码并运行)".to_string(),
            };
            (!text.trim().is_empty()).then(|| ChatLine {
                from_user: m.role != MsgRole::Assistant,
                text,
            })
        })
        .collect()
}

fn turn_result(ctx: &AppCtx, design_id: &str, messages: Vec<CadMessage>, version: Option<CadVersion>) -> Result<CadTurnResult, String> {
    Ok(CadTurnResult {
        design: db(ctx.db.get_design(design_id))?,
        messages,
        version,
    })
}

// ---------------------------------------------------------------- 设计

#[tauri::command(rename_all = "snake_case")]
pub async fn design_list(ctx: State<'_, Arc<AppCtx>>) -> Result<Vec<CadDesignSummary>, String> {
    db(ctx.db.list_designs())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn design_create(ctx: State<'_, Arc<AppCtx>>, name: Option<String>, project_id: Option<String>) -> Result<CadDesign, String> {
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| DEFAULT_NAME.to_string());
    db(ctx.db.create_design(&name, project_id.as_deref()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn design_get(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<CadDesignDetail, String> {
    Ok(CadDesignDetail {
        design: db(ctx.db.get_design(&id))?,
        messages: db(ctx.db.list_cad_messages(&id))?,
        versions: db(ctx.db.list_design_versions(&id))?,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn design_rename(ctx: State<'_, Arc<AppCtx>>, id: String, name: String) -> Result<CadDesign, String> {
    db(ctx.db.rename_design(&id, &name))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn design_delete(ctx: State<'_, Arc<AppCtx>>, id: String) -> Result<(), String> {
    db(ctx.db.delete_design(&id))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn design_link_project(ctx: State<'_, Arc<AppCtx>>, id: String, project_id: Option<String>) -> Result<CadDesign, String> {
    if let Some(pid) = &project_id {
        db(ctx.db.get_project(pid))?;
    }
    db(ctx.db.link_design_project(&id, project_id.as_deref()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn design_set_thumb(ctx: State<'_, Arc<AppCtx>>, id: String, thumb: String) -> Result<(), String> {
    db(ctx.db.set_design_thumb(&id, &thumb))
}

/// 选中一个版本。之后说的话、改的参数都从这一版接着来(从旧版本分叉)。
#[tauri::command(rename_all = "snake_case")]
pub async fn design_select_version(ctx: State<'_, Arc<AppCtx>>, id: String, version_id: String) -> Result<CadDesign, String> {
    db(ctx.db.set_design_current(&id, Some(&version_id)))
}

/// 导入一张图(缩小、去 EXIF)。`as_reference`:同时记为这个设计的参考图(看图出规格、看图复核都用它)。
#[tauri::command(rename_all = "snake_case")]
pub async fn design_add_image(ctx: State<'_, Arc<AppCtx>>, id: String, path: String, as_reference: bool) -> Result<Asset, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let design = db(ctx.db.get_design(&id))?;
        let src = Path::new(&path);
        if std::fs::metadata(src).map_err(fs_err)?.len() > 64 * 1024 * 1024 {
            return Err(invalid("image is larger than 64 MB"));
        }
        let (jpeg, w, h) = prepare_reference(&std::fs::read(src).map_err(fs_err)?)?;
        let mut new = NewAsset::new(AssetKind::Image, "cad_reference", "", "jpg", jpeg.len() as i64);
        new.rel_path = format!("assets/{}/{}.jpg", folder_for(&ctx, design.project_id.as_deref())?, new.id);
        new.project_id = design.project_id.clone();
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        new.meta_json = Some(json!({ "width": w, "height": h, "original_name": name }).to_string());
        let dest = ctx.asset_path(&new.rel_path);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(fs_err)?;
        }
        std::fs::write(&dest, &jpeg).map_err(fs_err)?;
        let asset = db(ctx.db.insert_asset(&new))?;
        if as_reference {
            let mut refs = design.ref_asset_ids.clone();
            refs.push(asset.id.clone());
            keep_latest(&mut refs);
            db(ctx.db.set_design_refs(&id, &refs))?;
        }
        log::info!("[design] 图片 {name} → {w} × {h} · {:.0} KB(已去除 EXIF)", jpeg.len() as f64 / 1024.0);
        Ok(asset)
    })
    .await
    .map_err(join_err)?
}

fn keep_latest(refs: &mut Vec<String>) {
    refs.dedup();
    if refs.len() > MAX_REFERENCE_IMAGES {
        let drop = refs.len() - MAX_REFERENCE_IMAGES;
        refs.drain(..drop);
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn design_remove_reference(ctx: State<'_, Arc<AppCtx>>, id: String, asset_id: String) -> Result<CadDesign, String> {
    let design = db(ctx.db.get_design(&id))?;
    let refs: Vec<String> = design.ref_asset_ids.into_iter().filter(|r| r != &asset_id).collect();
    db(ctx.db.set_design_refs(&id, &refs))
}

// ---------------------------------------------------------------- 对话

/// 演示模式下「改模型」的脚本:不理解话的内容,按当前代码的状态轮着演示几种典型的一轮对话。
fn demo_chat_answers(code: &str, text: &str, with_images: bool) -> Vec<String> {
    let mut answers = Vec::new();
    if with_images {
        answers.push("演示数据:图里的线槽槽底是半圆形(U 形槽),半径约为槽宽的一半。".to_string());
    }
    let t = text.trim();
    let is_question = t.ends_with('?') || t.ends_with('?') || t.contains('吗') || t.contains("怎么") || t.contains("多少");
    let answer = if is_question {
        "演示数据:不需要支撑——线槽开口朝上,没有超过 45° 的悬垂;建议底面贴床、0.2 mm 层高。".to_string()
    } else if !code.contains("demo_bottom_chamfer") {
        format!("演示数据:已给底边加了一圈 0.6 mm 的倒角(抵消第一层的「象脚」),其余不变。\n{}", fenced(&demo_edit(code)))
    } else if code.contains("slot_width = 6.0 ") {
        format!(
            "演示数据:线槽从 6 mm 加宽到 8 mm,其余不变。\n{}",
            fenced(&code.replace("slot_width = 6.0 ", "slot_width = 8.0 "))
        )
    } else {
        "演示模式只内置了两种修改(底边倒角、线槽加宽)和一种回答。想让我真的听懂你的话,请在「设置」里填入 DeepSeek 密钥并关闭演示模式。要改哪里?".to_string()
    };
    answers.push(answer);
    answers
}

/// 对话里的一轮:用户说一句话(可带点选位置、可带图)。
#[tauri::command(rename_all = "snake_case")]
pub async fn design_send(
    app: AppHandle,
    ctx: State<'_, Arc<AppCtx>>,
    id: String,
    text: String,
    pick: Option<CadPick>,
    image_asset_ids: Vec<String>,
    tier: CadTier,
) -> Result<CadTurnResult, String> {
    let text = text.trim().to_string();
    if text.is_empty() && image_asset_ids.is_empty() {
        return Err(invalid("message is empty"));
    }
    let ctx = ctx.inner().clone();
    let demo = ctx.config().demo_mode;
    let design = db(ctx.db.get_design(&id))?;
    let timeline = db(ctx.db.list_cad_messages(&id))?;
    let cfg = config_for(tier, demo);

    let mut user_msg = NewCadMessage::new(&id, MsgRole::User, MsgKind::Text, text.clone());
    user_msg.extra.pick = pick;
    user_msg.extra.base_version_id = design.current_version_id.clone();
    user_msg.image_asset_ids = image_asset_ids.clone();

    // 这一句里贴的图:同时成为参考图(之后的看图复核要对照它)
    let mut refs = design.ref_asset_ids.clone();
    refs.extend(image_asset_ids.iter().cloned());
    keep_latest(&mut refs);

    let Some(current_id) = design.current_version_id.clone() else {
        // ---- 还没有模型:出(或更新)设计规格 ----
        let images = {
            let (ctx, refs) = (ctx.clone(), refs.clone());
            tauri::async_runtime::spawn_blocking(move || reference_data_urls(&ctx, &refs)).await.map_err(join_err)??
        };
        // 之前说过的话都算数:「总长改成 80」是对上一版规格的修正
        let mut brief: Vec<String> = timeline.iter().filter(|m| m.role == MsgRole::User).map(|m| m.content.clone()).collect();
        brief.push(text.clone());
        let llm = llm_or_demo(&ctx, vec![DEMO_SPEC.to_string()])?;
        let (spec, report) = design_spec(llm.as_ref(), &brief.join("\n"), &images, &cfg, &emitter(&app))
            .await
            .map_err(agent_err)?;
        log::info!("[design] 规格「{}」· {} 个特征 · {} 张图 · {:.2} 分 · {}ms", spec.name, spec.features.len(), images.len(), report.cost_fen, report.elapsed_ms);
        let _ = ctx.db.add_cost(design.project_id.as_deref(), "model3d", report.cost_fen, &format!("{} · 出规格", spec.name));

        db(ctx.db.set_design_refs(&id, &refs))?;
        db(ctx.db.set_design_spec(&id, Some(&spec)))?;
        if design.name == DEFAULT_NAME && !spec.name.trim().is_empty() {
            db(ctx.db.rename_design(&id, &spec.name))?;
        }
        let mut reply = NewCadMessage::new(&id, MsgRole::Assistant, MsgKind::Spec, spec.summary.clone());
        reply.extra.spec = Some(spec);
        reply.extra.report = Some(report);
        let messages = vec![db(ctx.db.insert_cad_message(&user_msg))?, db(ctx.db.insert_cad_message(&reply))?];
        return turn_result(&ctx, &id, messages, None);
    };

    // ---- 已有模型:对话式修改 ----
    let mut cfg = cfg;
    if demo {
        // 演示脚本不会修代码:第一版没建成就如实给出「没改成 + 引擎的真实报错」,而不是去问一个答不上来的假模型
        cfg.max_repairs = 0;
    }
    let current = db(ctx.db.get_cad_version(&current_id))?;
    let exec = EngineExecutor::new(&ctx)?; // 先确认引擎在,再去花模型的钱
    let llm = llm_or_demo(&ctx, demo_chat_answers(&current.code, &text, !image_asset_ids.is_empty()))?;
    let mut spent = CadBuildReport::default();

    let image_notes = if image_asset_ids.is_empty() {
        None
    } else {
        let images = {
            let (ctx, ids) = (ctx.clone(), image_asset_ids.clone());
            tauri::async_runtime::spawn_blocking(move || reference_data_urls(&ctx, &ids)).await.map_err(join_err)??
        };
        let (notes, report) = describe_images(llm.as_ref(), &images, &text, &cfg).await.map_err(agent_err)?;
        add_report(&mut spent, &report);
        Some(notes)
    };

    let history = history_lines(&timeline);
    let turn = ChatTurn {
        history: &history,
        code: &current.code,
        spec: design.spec.as_ref(),
        message: if text.is_empty() { "(见图)" } else { &text },
        pick,
        image_notes: image_notes.as_deref(),
    };
    let outcome = chat_turn(llm.as_ref(), &exec, turn, &cfg, &emitter(&app)).await.map_err(agent_err)?;

    let label = design.spec.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| design.name.clone());
    tauri::async_runtime::spawn_blocking(move || {
        db(ctx.db.set_design_refs(&id, &refs))?;
        let (reply, version) = match outcome {
            ChatOutcome::Reply { text: answer, mut report } => {
                add_report(&mut report, &spent);
                let mut reply = NewCadMessage::new(&id, MsgRole::Assistant, MsgKind::Text, answer);
                reply.extra.report = Some(report);
                (reply, None)
            }
            ChatOutcome::Build { text: said, mut build } => {
                add_report(&mut build.report, &spent);
                match (build.metrics.is_some(), exec.take_output(&build.code)) {
                    (true, Some(output)) => {
                        let lineage = Lineage {
                            design_id: Some(id.clone()),
                            project_id: design.project_id.clone(),
                            spec: design.spec.clone(),
                            ref_asset_ids: refs.clone(),
                            parent: Some(current.clone()),
                            source: "edit",
                            note: text.clone(),
                            report: Some(build.report.clone()),
                        };
                        let version = persist(&ctx, &build.code, &output, lineage)?;
                        db(ctx.db.set_design_current(&id, Some(&version.id)))?;
                        let mut reply = NewCadMessage::new(&id, MsgRole::Assistant, MsgKind::Build, said);
                        reply.version_id = Some(version.id.clone());
                        reply.extra.report = Some(build.report);
                        reply.extra.base_version_id = Some(current.id.clone());
                        (reply, Some(version))
                    }
                    _ => {
                        let mut reply = NewCadMessage::new(&id, MsgRole::Assistant, MsgKind::Failed, said);
                        reply.extra.report = Some(build.report);
                        reply.extra.code = Some(build.code);
                        reply.extra.base_version_id = Some(current.id.clone());
                        (reply, None)
                    }
                }
            }
        };
        let report = reply.extra.report.clone().unwrap_or_default();
        log::info!(
            "[design] 「{label}」一轮对话:{} · {} 次模型调用 · {} 次执行 · {:.2} 分 · {}ms",
            reply.kind.as_str(),
            report.llm_calls,
            report.runs,
            report.cost_fen,
            report.elapsed_ms
        );
        let _ = ctx.db.add_cost(design.project_id.as_deref(), "model3d", report.cost_fen, &format!("{label} · 对话"));
        let messages = vec![db(ctx.db.insert_cad_message(&user_msg))?, db(ctx.db.insert_cad_message(&reply))?];
        turn_result(&ctx, &id, messages, version)
    })
    .await
    .map_err(join_err)?
}

/// 按(人审过的)规格生成第一版——规格卡上的「生成模型」。已经有模型时再点 = 按新规格重新生成一版。
#[tauri::command(rename_all = "snake_case")]
pub async fn design_generate(app: AppHandle, ctx: State<'_, Arc<AppCtx>>, id: String, spec: DesignSpec, tier: CadTier) -> Result<CadTurnResult, String> {
    let ctx = ctx.inner().clone();
    let demo = ctx.config().demo_mode;
    let design = db(ctx.db.get_design(&id))?;
    let exec = EngineExecutor::new(&ctx)?;
    let llm = llm_or_demo(&ctx, vec![fenced(&super::cad::demo_broken_code()), fenced(DEMO_CODE)])?;
    let build = generate_model(llm.as_ref(), &exec, &spec, &config_for(tier, demo), &emitter(&app))
        .await
        .map_err(agent_err)?;

    tauri::async_runtime::spawn_blocking(move || {
        let _ = ctx.db.add_cost(design.project_id.as_deref(), "model3d", build.report.cost_fen, &format!("{} · 生成", spec.name));
        log::info!(
            "[design] 「{}」生成:{} · {} 次模型调用 · {} 次执行 · {} 轮未通过 · {:.2} 分 · {}ms",
            spec.name,
            if build.metrics.is_some() { "建成" } else { "没有一版能跑" },
            build.report.llm_calls,
            build.report.runs,
            build.report.rounds.len(),
            build.report.cost_fen,
            build.report.elapsed_ms
        );
        db(ctx.db.set_design_spec(&id, Some(&spec)))?;
        if design.name == DEFAULT_NAME && !spec.name.trim().is_empty() {
            db(ctx.db.rename_design(&id, &spec.name))?;
        }
        let parent = design.current_version_id.as_deref().and_then(|v| ctx.db.get_cad_version(v).ok());
        let (reply, version) = match (build.metrics.is_some(), exec.take_output(&build.code)) {
            (true, Some(output)) => {
                let lineage = Lineage {
                    design_id: Some(id.clone()),
                    project_id: design.project_id.clone(),
                    spec: Some(spec.clone()),
                    ref_asset_ids: design.ref_asset_ids.clone(),
                    parent: parent.clone(),
                    source: "generate",
                    note: spec.name.clone(),
                    report: Some(build.report.clone()),
                };
                let version = persist(&ctx, &build.code, &output, lineage)?;
                db(ctx.db.set_design_current(&id, Some(&version.id)))?;
                let mut reply = NewCadMessage::new(&id, MsgRole::Assistant, MsgKind::Build, "");
                reply.version_id = Some(version.id.clone());
                reply.extra.report = Some(build.report);
                reply.extra.base_version_id = parent.map(|p| p.id);
                (reply, Some(version))
            }
            _ => {
                let mut reply = NewCadMessage::new(&id, MsgRole::Assistant, MsgKind::Failed, "");
                reply.extra.report = Some(build.report);
                reply.extra.code = Some(build.code);
                (reply, None)
            }
        };
        let messages = vec![db(ctx.db.insert_cad_message(&reply))?];
        turn_result(&ctx, &id, messages, version)
    })
    .await
    .map_err(join_err)?
}

// ---------------------------------------------------------------- 时间线上的其它事

/// 本机重建(改参数 / 手改代码):执行 → 入库 → 选中 → 在时间线上记一笔。不经过模型。
async fn rebuild(ctx: Arc<AppCtx>, id: String, code: String, source: &'static str, kind: MsgKind, note: String) -> Result<CadTurnResult, String> {
    use pp_agent::CadExecutor as _;
    let design = db(ctx.db.get_design(&id))?;
    let parent = design.current_version_id.as_deref().map(|v| db(ctx.db.get_cad_version(v))).transpose()?;
    let exec = EngineExecutor::new(&ctx)?;
    exec.execute(&code).await.map_err(cad_err)?;
    let output = exec.take_output(&code).ok_or_else(|| cad_err(CadError::Protocol("no output".into())))?;
    tauri::async_runtime::spawn_blocking(move || {
        let lineage = Lineage {
            design_id: Some(id.clone()),
            project_id: design.project_id.clone(),
            spec: design.spec.clone(),
            ref_asset_ids: design.ref_asset_ids.clone(),
            parent,
            source,
            note: note.clone(),
            report: None,
        };
        let version = persist(&ctx, &code, &output, lineage)?;
        db(ctx.db.set_design_current(&id, Some(&version.id)))?;
        let mut event = NewCadMessage::new(&id, MsgRole::Event, kind, note);
        event.version_id = Some(version.id.clone());
        let messages = vec![db(ctx.db.insert_cad_message(&event))?];
        turn_result(&ctx, &id, messages, Some(version))
    })
    .await
    .map_err(join_err)?
}

/// 改一个参数:只换 PARAMS 段里那一个数字,本机重建。不经过模型,不花钱。
#[tauri::command(rename_all = "snake_case")]
pub async fn design_set_param(ctx: State<'_, Arc<AppCtx>>, id: String, name: String, value: f64) -> Result<CadTurnResult, String> {
    let ctx = ctx.inner().clone();
    let design = db(ctx.db.get_design(&id))?;
    let current_id = design.current_version_id.ok_or_else(|| invalid("this design has no model yet"))?;
    let current = db(ctx.db.get_cad_version(&current_id))?;
    let before = current.params.iter().find(|p| p.name == name);
    let code = set_param(&current.code, &name, value).map_err(|e| match e {
        ParamError::UnknownParam(_) => errcode::err(errcode::NOT_FOUND, e),
        _ => invalid(e),
    })?;
    let fmt = |v: f64| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
    let label = before.map(|p| if p.label.is_empty() { p.name.clone() } else { p.label.clone() }).unwrap_or_else(|| name.clone());
    let note = format!("{label} {} → {}", before.map(|p| fmt(p.value)).unwrap_or_default(), fmt(value));
    rebuild(ctx, id, code, "param", MsgKind::Param, note).await
}

/// 运行手改过的代码,成功则成为新版本。
#[tauri::command(rename_all = "snake_case")]
pub async fn design_run_code(ctx: State<'_, Arc<AppCtx>>, id: String, code: String) -> Result<CadTurnResult, String> {
    check_code(&code)?;
    rebuild(ctx.inner().clone(), id, code, "manual", MsgKind::Manual, String::new()).await
}

/// 看图复核:前端截的几张渲染图 vs 这个设计的参考图 → 差异清单进时间线(每条都能一键变成下一句话)。
#[tauri::command(rename_all = "snake_case")]
pub async fn design_review(app: AppHandle, ctx: State<'_, Arc<AppCtx>>, id: String, renders: Vec<String>) -> Result<CadTurnResult, String> {
    check_renders(&renders)?;
    let ctx = ctx.inner().clone();
    let demo = ctx.config().demo_mode;
    let design = db(ctx.db.get_design(&id))?;
    if design.ref_asset_ids.is_empty() {
        return Err(invalid("this design has no reference image to compare against"));
    }
    let references = {
        let (ctx, ids) = (ctx.clone(), design.ref_asset_ids.clone());
        tauri::async_runtime::spawn_blocking(move || reference_data_urls(&ctx, &ids)).await.map_err(join_err)??
    };
    let demo_answer = json!({ "matches": false, "differences": ["演示数据:线槽底部应为半圆形(U 形槽),现在是直角槽"] });
    let llm = llm_or_demo(&ctx, vec![demo_answer.to_string()])?;
    let (review, report) = review_model(llm.as_ref(), &references, &renders, design.spec.as_ref(), &cad_config(demo), &emitter(&app))
        .await
        .map_err(agent_err)?;
    let _ = ctx.db.add_cost(design.project_id.as_deref(), "model3d", report.cost_fen, "看图复核");
    let mut reply = NewCadMessage::new(&id, MsgRole::Assistant, MsgKind::Review, "");
    reply.version_id = design.current_version_id.clone();
    reply.extra.review = Some(review);
    reply.extra.report = Some(report);
    let messages = vec![db(ctx.db.insert_cad_message(&reply))?];
    turn_result(&ctx, &id, messages, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pp_common::design::MsgExtra;

    fn msg(role: MsgRole, kind: MsgKind, content: &str) -> CadMessage {
        CadMessage {
            id: "m".into(),
            design_id: "d".into(),
            role,
            kind,
            content: content.into(),
            extra: MsgExtra::default(),
            image_asset_ids: vec![],
            version_id: None,
            created_at: 0,
        }
    }

    #[test]
    fn the_model_hears_about_manual_changes_too() {
        let lines = history_lines(&[
            msg(MsgRole::User, MsgKind::Text, "做一个线缆夹"),
            msg(MsgRole::Assistant, MsgKind::Spec, "桌面线缆夹"),
            msg(MsgRole::Assistant, MsgKind::Build, ""),
            msg(MsgRole::Event, MsgKind::Param, "总长 60 → 90"),
            msg(MsgRole::Assistant, MsgKind::Text, "不需要支撑。"),
            msg(MsgRole::Assistant, MsgKind::Failed, "加了圆角"),
        ]);
        let texts: Vec<(bool, &str)> = lines.iter().map(|l| (l.from_user, l.text.as_str())).collect();
        assert_eq!(texts[0], (true, "做一个线缆夹"));
        assert!(!texts[1].0 && texts[1].1.contains("设计规格"));
        assert_eq!(texts[2], (false, "(已按规格生成模型)"));
        assert_eq!(texts[3], (true, "(我手动改了参数:总长 60 → 90)"), "手动改动以用户的口吻告诉模型");
        assert_eq!(texts[4], (false, "不需要支撑。"));
        assert!(texts[5].1.contains("没有成功"), "失败的那轮不能让模型以为改成了");
    }

    #[test]
    fn tiers_pick_the_model_and_thinking() {
        let fast = config_for(CadTier::Fast, false);
        assert_eq!((fast.code_model.as_str(), fast.code_thinking), ("deepseek-flash", false));
        let balanced = config_for(CadTier::Balanced, false);
        assert_eq!((balanced.code_model.as_str(), balanced.code_thinking), ("deepseek-v4-pro", false));
        let precise = config_for(CadTier::Precise, false);
        assert_eq!((precise.code_model.as_str(), precise.code_thinking), ("deepseek-v4-pro", true));
        assert_eq!(config_for(CadTier::Fast, true).code_pricing.output, 0.0, "演示模式不计费");
        assert!(fast.code_pricing.output < balanced.code_pricing.output);
    }

    #[test]
    fn only_the_latest_reference_images_are_kept() {
        let mut refs: Vec<String> = (0..7).map(|i| format!("r{i}")).collect();
        keep_latest(&mut refs);
        assert_eq!(refs, ["r3", "r4", "r5", "r6"]);
    }

    #[test]
    fn the_demo_conversation_walks_through_an_edit_a_second_edit_and_an_answer() {
        let first = demo_chat_answers(DEMO_CODE, "底边倒个角", false);
        assert_eq!(first.len(), 1);
        let (said, code) = pp_agent::split_answer(&first[0]);
        assert!(said.contains("演示数据") && code.as_deref().is_some_and(|c| c.contains("demo_bottom_chamfer")));

        let second = demo_chat_answers(&code.unwrap(), "线槽宽一点", false);
        let (_, code2) = pp_agent::split_answer(&second[0]);
        assert!(code2.as_deref().is_some_and(|c| c.contains("slot_width = 8.0 ")), "第二次演示的是只改一个参数");

        let q = demo_chat_answers(DEMO_CODE, "这个要加支撑吗", false);
        assert!(pp_agent::split_answer(&q[0]).1.is_none(), "问句得到的是回答,不是代码");

        let with_image = demo_chat_answers(DEMO_CODE, "改成这样", true);
        assert_eq!(with_image.len(), 2, "带图时先有一次看图的调用");
    }
}
