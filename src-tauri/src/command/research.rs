//! 市场调研命令:运行流水线、读历史、把机会采用为项目。

use std::sync::Arc;

use pp_agent::{run_research, ResearchConfig, ResearchProgress};
use pp_common::errcode;
use pp_common::research::{ResearchBrief, ResearchRunSummary, SavedResearch};
use pp_common::Project;
use pp_providers::llm::{LlmProvider, MockLlm};
use pp_providers::search::{MockSearch, SearchProvider};
use serde_json::json;
use tauri::{AppHandle, Emitter, State};

use super::join_err;
use super::provider::{llm_for, search_for};
use crate::AppCtx;

/// 进度事件名(前端 listen 这个)。载荷:`{ step, done?, total? }`
const PROGRESS_EVENT: &str = "research-progress";

fn progress_payload(p: &ResearchProgress) -> serde_json::Value {
    match p {
        ResearchProgress::Planning => json!({ "step": "planning" }),
        ResearchProgress::Searching { done, total } => json!({ "step": "searching", "done": done, "total": total }),
        ResearchProgress::Scoring => json!({ "step": "scoring" }),
        ResearchProgress::Reviewing => json!({ "step": "reviewing" }),
        ResearchProgress::Done => json!({ "step": "done" }),
    }
}

/// 演示模式的脚本:不联网、不花钱,但走的是同一条流水线(规划 → 检索 → 评分 → 审校),
/// 所以界面、落库、采用为项目都能完整体验。
fn demo_llm(topic: &str) -> MockLlm {
    let plan = json!({ "queries": [format!("{topic} 3D打印"), format!("{topic} 价格"), format!("{topic} 差评")] });
    let scored = json!({
        "summary_md": format!("**演示数据**:围绕「{topic}」,检索到的内容显示需求稳定、同质化款式价格战明显 [1][3],差异化要靠原创造型与个性化定制 [2]。"),
        "opportunities": [
            {
                "title": format!("{topic} · 可刻字定制款"), "pitch": "同类里唯一能刻名字的", "persona": "送礼与自用的年轻上班族",
                "selling_points": ["刻字定制", "多配色", "48 小时发货"], "price_low": 29, "price_high": 59,
                "scores": { "demand": 4, "differentiation": 4.5, "competition": 3, "printability": 4.5, "margin": 4, "logistics": 5 },
                "ip_risk": "low", "rationale": "需求稳定 [1],定制能避开价格战 [2]。", "evidence_ids": [1, 2],
                "risks": ["定制单沟通成本高"], "route": "parametric", "size_mm": 80, "grams": 35
            },
            {
                "title": format!("{topic} · 基础款"), "pitch": "便宜够用", "persona": "价格敏感的学生",
                "selling_points": ["便宜", "现货"], "price_low": 9, "price_high": 19,
                "scores": { "demand": 4, "differentiation": 1.5, "competition": 1, "printability": 5, "margin": 2, "logistics": 5 },
                "ip_risk": "low", "rationale": "同款多、价格战凶 [3]。", "evidence_ids": [3],
                "risks": ["利润薄,占机时"], "route": "parametric", "size_mm": 80, "grams": 30
            },
            {
                "title": format!("{topic} · 热门动漫角色款"), "pitch": "蹭 IP 热度", "persona": "二次元人群",
                "selling_points": ["角色造型"], "price_low": 39, "price_high": 89,
                "scores": { "demand": 5, "differentiation": 3, "competition": 3, "printability": 3, "margin": 4, "logistics": 4 },
                "ip_risk": "high", "rationale": "热度高 [1],但未经授权即侵权。", "evidence_ids": [1],
                "risks": ["版权方投诉下架、索赔"], "route": "ai_generated", "size_mm": 120, "grams": 60
            }
        ]
    });
    let review = json!({
        "summary_md": format!("**演示数据**:围绕「{topic}」,需求稳定 [1],同质化款式价格战明显 [3]。推测:个性化定制款的溢价空间更大 [2]。")
    });
    MockLlm::new([plan.to_string(), scored.to_string(), review.to_string()])
}

#[tauri::command(rename_all = "snake_case")]
pub async fn research_run(app: AppHandle, ctx: State<'_, Arc<AppCtx>>, brief: ResearchBrief) -> Result<SavedResearch, String> {
    let ctx = ctx.inner().clone();
    let demo = ctx.config().demo_mode;
    let emit = {
        let app = app.clone();
        move |p: ResearchProgress| {
            let _ = app.emit(PROGRESS_EVENT, progress_payload(&p));
        }
    };

    let (llm, search): (Box<dyn LlmProvider>, Option<Box<dyn SearchProvider>>) = if demo {
        (Box::new(demo_llm(brief.topic.trim())), Some(Box::new(MockSearch::new(2))))
    } else {
        let search = search_for(&ctx)?.map(|s| Box::new(s) as Box<dyn SearchProvider>);
        (Box::new(llm_for(&ctx)?), search)
    };

    let mut cfg = ResearchConfig::default();
    if demo {
        // 假模型不花钱:别让演示数据污染费用账本
        cfg.pricing = pp_providers::llm::LlmPricing { input_cache_hit: 0.0, input_cache_miss: 0.0, output: 0.0 };
        cfg.critic_pricing = cfg.pricing;
    }

    let mut report = run_research(llm.as_ref(), search.as_deref(), &brief, &cfg, &emit)
        .await
        .map_err(|e| errcode::err(e.code(), e))?;
    if demo {
        report.usage.cost_fen = 0.0;
    }
    log::info!(
        "[research] 「{}」完成:{} 个机会 · {} 次模型调用 · {} 次检索 · {:.1} 分 · {}ms{}{}",
        brief.topic.trim(),
        report.opportunities.len(),
        report.usage.llm_calls,
        report.usage.searches,
        report.usage.cost_fen,
        report.usage.elapsed_ms,
        if report.degraded { " · 降级(无检索)" } else { "" },
        if demo { " · 演示模式" } else { "" },
    );

    let saved = tauri::async_runtime::spawn_blocking(move || {
        let run_id = ctx.db.save_research(None, &brief, &report).map_err(|e| e.to_wire())?;
        Ok::<_, String>(SavedResearch { run_id, brief, report })
    })
    .await
    .map_err(join_err)??;
    Ok(saved)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_research_runs(ctx: State<'_, Arc<AppCtx>>) -> Result<Vec<ResearchRunSummary>, String> {
    ctx.db.list_research_runs(50).map_err(|e| e.to_wire())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_research(ctx: State<'_, Arc<AppCtx>>, run_id: String) -> Result<SavedResearch, String> {
    ctx.db.get_research(&run_id).map_err(|e| e.to_wire())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn adopt_opportunity(ctx: State<'_, Arc<AppCtx>>, run_id: String, index: u32) -> Result<Project, String> {
    let p = ctx.db.adopt_opportunity(&run_id, index as usize).map_err(|e| e.to_wire())?;
    log::info!("[research] 采用机会 → {} {}", p.code, p.title);
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_demo_script_survives_the_real_pipeline_validation() {
        // 演示脚本如果和流水线的校验规则脱节(比如改了评分范围),演示模式会在用户面前报错——这里提前拦住
        let brief: ResearchBrief = serde_json::from_str(r#"{ "topic": "桌面收纳" }"#).unwrap();
        let llm = demo_llm(&brief.topic);
        let search = MockSearch::new(2);
        let report = run_research(&llm, Some(&search), &brief, &ResearchConfig::default(), &|_| {})
            .await
            .expect("demo script must pass validation");
        assert_eq!(report.opportunities.len(), 3);
        assert_eq!(report.usage.json_retries, 0, "演示脚本一次就该通过");
        assert!(report.opportunities.last().unwrap().vetoed(), "IP 款垫底");
        assert!(report.reviewed && !report.degraded);
        assert!(report.summary_md.contains("演示数据"), "演示数据必须自己说明自己是演示数据");
    }

    #[test]
    fn progress_payloads_carry_a_step_name() {
        assert_eq!(progress_payload(&ResearchProgress::Planning)["step"], "planning");
        let p = progress_payload(&ResearchProgress::Searching { done: 2, total: 5 });
        assert_eq!((p["done"].as_u64(), p["total"].as_u64()), (Some(2), Some(5)));
    }
}
