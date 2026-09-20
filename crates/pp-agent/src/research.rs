//! 市场调研流水线:规划 → 检索 → 机会评分 → 审校(docs/03-architecture.md §7)。
//!
//! 几条产品上的硬规矩落在这里:
//! - 每个机会的评分依据必须引用证据卡编号,引用了不存在的编号 → 打回重写;
//! - 总分由我们按权重算,不信模型自己报的总分;IP 风险「高」一票否决、排到最后;
//! - 没配搜索 → 退化为「用户证据 + 模型知识」并如实标注,而不是报错;
//! - 审校失败不影响出报告,但报告要标明「未经审校」。

use std::collections::HashSet;
use std::time::Instant;

use pp_common::research::{
    Evidence, EvidenceGrade, Opportunity, ResearchBrief, ResearchReport, ResearchUsage, ScoreWeights,
};
use pp_providers::llm::{ChatMessage, ChatRequest, LlmPricing, LlmProvider, Usage};
use pp_providers::search::{Recency, SearchProvider, SearchQuery};
use pp_providers::ProviderError;
use serde::Deserialize;

use crate::json::ask_json;
use crate::AgentError;

const PLAN_PROMPT: &str = include_str!("../prompts/research_plan.md");
const SCORE_PROMPT: &str = include_str!("../prompts/research_score.md");
const REVIEW_PROMPT: &str = include_str!("../prompts/research_review.md");

#[derive(Debug, Clone, PartialEq)]
pub struct ResearchConfig {
    /// 规划与评分用的模型(便宜、快)
    pub model: String,
    pub pricing: LlmPricing,
    /// 审校用的强模型;`None` = 跳过审校
    pub critic_model: Option<String>,
    pub critic_pricing: LlmPricing,
    /// 检索次数上限——预算闸门的一部分
    pub max_searches: u32,
    pub hits_per_search: u32,
    pub max_evidence: usize,
    /// 每张证据卡的摘录长度上限(字符)
    pub excerpt_chars: usize,
    pub weights: ScoreWeights,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            model: "deepseek-flash".into(),
            pricing: LlmPricing::deepseek_flash(),
            critic_model: Some("deepseek-v4-pro".into()),
            critic_pricing: LlmPricing::deepseek_pro(),
            max_searches: 8,
            hits_per_search: 6,
            max_evidence: 24,
            excerpt_chars: 500,
            weights: ScoreWeights::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResearchProgress {
    Planning,
    Searching { done: u32, total: u32 },
    Scoring,
    Reviewing,
    Done,
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    queries: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawScored {
    summary_md: String,
    opportunities: Vec<Opportunity>,
}

#[derive(Debug, Deserialize)]
struct RawReview {
    summary_md: String,
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn brief_block(brief: &ResearchBrief) -> String {
    let price = match (brief.price_min, brief.price_max) {
        (Some(lo), Some(hi)) => format!("¥{lo}~¥{hi}"),
        (Some(lo), None) => format!("¥{lo} 以上"),
        (None, Some(hi)) => format!("¥{hi} 以内"),
        (None, None) => "不限".to_string(),
    };
    format!(
        "## 调研方向\n{}\n\n## 约束\n- 成型空间:{} × {} × {} mm\n- 材料:{}\n- 目标价位:{}\n- 渠道:{}\n",
        brief.topic.trim(),
        brief.build_mm[0],
        brief.build_mm[1],
        brief.build_mm[2],
        brief.materials,
        price,
        brief.channel
    )
}

fn evidence_block(evidence: &[Evidence]) -> String {
    if evidence.is_empty() {
        return "## 证据卡\n(没有任何证据。请只基于常识判断,`evidence_ids` 留空,并把所有判断明确标注为「推测」。)\n".into();
    }
    let mut out = String::from("## 证据卡\n");
    for e in evidence {
        let source = match e.grade {
            EvidenceGrade::A => "A · 卖家提供的一手观察".to_string(),
            _ => format!(
                "B · {}{}",
                if e.site.is_empty() { "网页" } else { &e.site },
                e.published.as_deref().map(|d| format!(" · {d}")).unwrap_or_default()
            ),
        };
        out.push_str(&format!("[{}] ({source}) {}\n{}\n\n", e.id, e.title, e.excerpt));
    }
    out
}

/// 用户补充的一手证据 → A 级证据卡,编号排在最前面。
fn user_evidence(brief: &ResearchBrief, excerpt_chars: usize) -> Vec<Evidence> {
    brief
        .user_notes
        .iter()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .enumerate()
        .map(|(i, note)| {
            let url = note
                .split_whitespace()
                .find(|w| w.starts_with("http://") || w.starts_with("https://"))
                .unwrap_or("")
                .to_string();
            Evidence {
                id: i as u32 + 1,
                title: truncate_chars(note, 30),
                url,
                site: String::new(),
                published: None,
                excerpt: truncate_chars(note, excerpt_chars),
                grade: EvidenceGrade::A,
            }
        })
        .collect()
}

fn validate_plan(plan: &RawPlan) -> Result<(), String> {
    let usable = plan.queries.iter().filter(|q| !q.trim().is_empty()).count();
    if usable == 0 {
        return Err("queries 至少要有一条非空的检索词".into());
    }
    Ok(())
}

fn validate_scored(raw: &RawScored, known_ids: &HashSet<u32>, brief: &ResearchBrief) -> Result<(), String> {
    if raw.summary_md.trim().is_empty() {
        return Err("summary_md 不能为空".into());
    }
    if raw.opportunities.is_empty() || raw.opportunities.len() > 6 {
        return Err(format!("opportunities 应为 3~5 个,现在是 {} 个", raw.opportunities.len()));
    }
    let longest_build = brief.build_mm.iter().cloned().fold(0.0, f64::max);
    for (i, o) in raw.opportunities.iter().enumerate() {
        let at = format!("第 {} 个机会「{}」", i + 1, o.title);
        if o.title.trim().is_empty() {
            return Err(format!("第 {} 个机会缺少 title", i + 1));
        }
        if o.selling_points.is_empty() {
            return Err(format!("{at} 缺少 selling_points"));
        }
        if o.price_high == 0 || o.price_low > o.price_high {
            return Err(format!("{at} 的价格带不合理:{}~{}", o.price_low, o.price_high));
        }
        if !o.scores.in_range() {
            return Err(format!("{at} 的评分必须都在 0~5 之间"));
        }
        if let Some(bad) = o.evidence_ids.iter().find(|id| !known_ids.contains(id)) {
            return Err(format!("{at} 引用了不存在的证据编号 [{bad}];只能引用给出的证据卡"));
        }
        if !known_ids.is_empty() && o.evidence_ids.is_empty() {
            return Err(format!("{at} 没有引用任何证据卡;评分依据必须引用证据编号"));
        }
        if longest_build > 0.0 && o.size_mm > longest_build {
            return Err(format!("{at} 的尺寸 {} mm 超过了成型空间 {} mm", o.size_mm, longest_build));
        }
    }
    Ok(())
}

pub async fn run_research(
    llm: &dyn LlmProvider,
    search: Option<&dyn SearchProvider>,
    brief: &ResearchBrief,
    cfg: &ResearchConfig,
    on_progress: &(dyn Fn(ResearchProgress) + Send + Sync),
) -> Result<ResearchReport, AgentError> {
    if brief.topic.trim().is_empty() {
        return Err(AgentError::EmptyBrief);
    }
    let started = Instant::now();
    let mut main_usage = Usage::default();
    let mut critic_usage = Usage::default();
    let mut usage = ResearchUsage::default();
    let mut evidence = user_evidence(brief, cfg.excerpt_chars);
    let mut search_cost_fen = 0.0;
    let mut got_hits = false;

    // ---- 1~2. 规划 + 检索(没有搜索供应商就整段跳过,连规划那次模型调用也省了)----
    if let Some(search) = search {
        on_progress(ResearchProgress::Planning);
        let system = PLAN_PROMPT.replace("{{max_queries}}", &cfg.max_searches.to_string());
        let req = ChatRequest::new(&cfg.model, vec![ChatMessage::system(system), ChatMessage::user(brief_block(brief))])
            .max_tokens(800);
        let plan = ask_json::<RawPlan, _>(llm, req, validate_plan).await?;
        main_usage.add(plan.usage);
        usage.llm_calls += plan.calls;
        usage.json_retries += plan.retries;

        let mut seen_queries = HashSet::new();
        let queries: Vec<String> = plan
            .value
            .queries
            .iter()
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty() && seen_queries.insert(q.clone()))
            .take(cfg.max_searches as usize)
            .collect();

        let mut seen_urls: HashSet<String> = evidence.iter().map(|e| e.url.clone()).filter(|u| !u.is_empty()).collect();
        let total = queries.len() as u32;
        for (i, query) in queries.into_iter().enumerate() {
            on_progress(ResearchProgress::Searching {
                done: i as u32,
                total,
            });
            let mut q = SearchQuery::new(query.clone());
            q.count = cfg.hits_per_search;
            q.recency = Recency::Year;
            match search.search(q).await {
                Ok(hits) => {
                    usage.searches += 1;
                    search_cost_fen += search.price_per_query_fen();
                    for hit in hits {
                        if evidence.len() >= cfg.max_evidence || hit.snippet.trim().is_empty() {
                            continue;
                        }
                        if !seen_urls.insert(hit.url.clone()) {
                            continue; // 不同检索词搜到同一篇
                        }
                        got_hits = true;
                        evidence.push(Evidence {
                            id: evidence.len() as u32 + 1,
                            title: truncate_chars(&hit.title, 80),
                            url: hit.url,
                            site: hit.site,
                            published: hit.published,
                            excerpt: truncate_chars(hit.snippet.trim(), cfg.excerpt_chars),
                            grade: EvidenceGrade::B,
                        });
                    }
                }
                // 没配搜索密钥:按「没有搜索」继续
                Err(ProviderError::MissingKey) => break,
                // 密钥不对 / 没余额:这是用户要去修的配置问题,悄悄降级反而误事
                Err(e @ (ProviderError::Auth | ProviderError::InsufficientBalance)) => return Err(e.into()),
                // 偶发失败:跳过这一条,其它的照常
                Err(e) => log::warn!("[research] 检索「{query}」失败:{e}"),
            }
        }
    }
    let degraded = !got_hits;
    let known_ids: HashSet<u32> = evidence.iter().map(|e| e.id).collect();

    // ---- 3. 机会生成与评分 ----
    on_progress(ResearchProgress::Scoring);
    let system = SCORE_PROMPT.replace("{{channel}}", &brief.channel);
    let user = format!("{}\n{}", brief_block(brief), evidence_block(&evidence));
    let req = ChatRequest::new(&cfg.model, vec![ChatMessage::system(system), ChatMessage::user(user)]).max_tokens(6000);
    let scored = ask_json::<RawScored, _>(llm, req, |raw| validate_scored(raw, &known_ids, brief)).await?;
    main_usage.add(scored.usage);
    usage.llm_calls += scored.calls;
    usage.json_retries += scored.retries;

    let RawScored {
        mut summary_md,
        mut opportunities,
    } = scored.value;
    for o in &mut opportunities {
        o.total = o.scores.total(&cfg.weights);
        o.evidence_ids.sort_unstable();
        o.evidence_ids.dedup();
    }
    // 没被否决的按总分从高到低;被否决的(IP 风险高)排最后
    opportunities.sort_by(|a, b| {
        a.vetoed()
            .cmp(&b.vetoed())
            .then(b.total.partial_cmp(&a.total).unwrap_or(std::cmp::Ordering::Equal))
    });

    // ---- 4. 审校:把没有证据的断言降级为「推测」。没有证据可对照时跳过 ----
    let mut reviewed = false;
    if let (Some(critic), false) = (&cfg.critic_model, evidence.is_empty()) {
        on_progress(ResearchProgress::Reviewing);
        let user = format!("## 结论\n{summary_md}\n\n{}", evidence_block(&evidence));
        let req = ChatRequest::new(critic, vec![ChatMessage::system(REVIEW_PROMPT), ChatMessage::user(user)])
            .thinking(true)
            .max_tokens(4000);
        let not_blank = |r: &RawReview| {
            if r.summary_md.trim().is_empty() {
                Err("summary_md 不能为空".to_string())
            } else {
                Ok(())
            }
        };
        match ask_json::<RawReview, _>(llm, req, not_blank).await {
            Ok(review) => {
                critic_usage.add(review.usage);
                usage.llm_calls += review.calls;
                usage.json_retries += review.retries;
                summary_md = review.value.summary_md;
                reviewed = true;
            }
            Err(e) => log::warn!("[research] 审校失败,保留未审校的结论:{e}"),
        }
    }

    usage.tokens_in = main_usage.prompt_tokens + critic_usage.prompt_tokens;
    usage.tokens_out = main_usage.completion_tokens + critic_usage.completion_tokens;
    usage.cost_fen = cfg.pricing.cost_fen(main_usage) + cfg.critic_pricing.cost_fen(critic_usage) + search_cost_fen;
    usage.elapsed_ms = started.elapsed().as_millis() as u64;
    on_progress(ResearchProgress::Done);

    Ok(ResearchReport {
        summary_md,
        opportunities,
        evidence,
        usage,
        degraded,
        reviewed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pp_common::research::IpRisk;
    use pp_providers::llm::MockLlm;
    use pp_providers::search::MockSearch;
    use std::sync::Mutex;

    fn brief() -> ResearchBrief {
        ResearchBrief {
            topic: "桌面收纳".into(),
            build_mm: [256.0, 256.0, 256.0],
            materials: "PLA".into(),
            price_min: Some(19),
            price_max: Some(49),
            channel: "小红书".into(),
            user_notes: vec![],
        }
    }

    fn opp(title: &str, demand: f64, ip: &str, ids: &[u32]) -> String {
        format!(
            r#"{{"title":"{title}","pitch":"p","persona":"桌搭人群","selling_points":["a","b"],
"price_low":19,"price_high":39,
"scores":{{"demand":{demand},"differentiation":3,"competition":3,"printability":4,"margin":4,"logistics":5}},
"ip_risk":"{ip}","rationale":"见 [1]","evidence_ids":{ids:?},"risks":[],"route":"parametric","size_mm":60,"grams":25}}"#
        )
    }

    fn scored(opps: &[String]) -> String {
        format!(r#"{{"summary_md":"线缆收纳需求稳定 [1]","opportunities":[{}]}}"#, opps.join(","))
    }

    const PLAN: &str = r#"{"queries":["桌面收纳 3D打印","线缆夹 价格","桌面收纳 差评"]}"#;
    const REVIEW: &str = r#"{"summary_md":"线缆收纳需求稳定 [1]。推测:定制款溢价更高。"}"#;

    fn no_progress() -> impl Fn(ResearchProgress) + Send + Sync {
        |_| {}
    }

    #[tokio::test]
    async fn full_run_plans_searches_scores_and_reviews() {
        let llm = MockLlm::new([
            PLAN.to_string(),
            scored(&[opp("普通款", 2.0, "low", &[1]), opp("爆款", 5.0, "low", &[1, 2]), opp("某动漫角色支架", 5.0, "high", &[1])]),
            REVIEW.to_string(),
        ]);
        let search = MockSearch::new(2);
        let seen = Mutex::new(Vec::new());
        let report = run_research(&llm, Some(&search), &brief(), &ResearchConfig::default(), &|p| {
            seen.lock().unwrap().push(p)
        })
        .await
        .unwrap();

        assert_eq!(search.queries(), ["桌面收纳 3D打印", "线缆夹 价格", "桌面收纳 差评"]);
        assert_eq!(report.evidence.len(), 6, "3 次检索 × 2 条");
        assert_eq!(report.evidence.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
        assert!(report.evidence.iter().all(|e| e.grade == EvidenceGrade::B));

        let titles: Vec<_> = report.opportunities.iter().map(|o| o.title.as_str()).collect();
        assert_eq!(titles, ["爆款", "普通款", "某动漫角色支架"], "按总分排序,被否决的垫底");
        assert!(report.opportunities[2].vetoed());
        assert_eq!(report.opportunities[2].ip_risk, IpRisk::High);
        let expected_total = report.opportunities[0].scores.total(&ScoreWeights::default());
        assert!((report.opportunities[0].total - expected_total).abs() < 1e-12, "总分由我们算");

        assert!(report.reviewed);
        assert!(report.summary_md.contains("推测"));
        assert!(!report.degraded);
        assert_eq!((report.usage.llm_calls, report.usage.searches, report.usage.json_retries), (3, 3, 0));
        assert!(report.usage.cost_fen > 3.0, "3 次检索各 1 分 + 模型费用");

        let progress = seen.lock().unwrap().clone();
        assert_eq!(progress.first(), Some(&ResearchProgress::Planning));
        assert_eq!(progress.last(), Some(&ResearchProgress::Done));
        assert!(progress.contains(&ResearchProgress::Searching { done: 2, total: 3 }));

        // 审校要用强模型 + 思考模式;规划与评分不用
        let reqs = llm.requests();
        assert_eq!(reqs[2].model, "deepseek-v4-pro");
        assert!(reqs[2].thinking && !reqs[0].thinking && !reqs[1].thinking);
        assert!(reqs[1].messages[1].content.contains("[2] (B · 示例站 · 2026-08-15)"));
    }

    #[tokio::test]
    async fn without_a_search_provider_it_degrades_instead_of_failing() {
        let llm = MockLlm::new([scored(&[opp("线缆夹", 4.0, "low", &[])])]);
        let report = run_research(&llm, None, &brief(), &ResearchConfig::default(), &no_progress())
            .await
            .unwrap();
        assert!(report.degraded);
        assert!(!report.reviewed, "没有证据可对照,跳过审校");
        assert!(report.evidence.is_empty());
        assert_eq!(report.usage.llm_calls, 1, "没有搜索就不需要规划那一次调用");
        assert!(llm.requests()[0].messages[1].content.contains("没有任何证据"));
    }

    #[tokio::test]
    async fn seller_notes_become_grade_a_evidence_numbered_first() {
        let mut b = brief();
        b.user_notes = vec![
            "  ".into(),
            "同类线缆夹在某店月销 2000+,价格 ¥25 https://example.com/shop/1 评论里都在问有没有磁吸款".into(),
        ];
        let llm = MockLlm::new([PLAN.to_string(), scored(&[opp("磁吸线缆夹", 4.0, "low", &[1, 2])]), REVIEW.to_string()]);
        let search = MockSearch::new(1);
        let report = run_research(&llm, Some(&search), &b, &ResearchConfig::default(), &no_progress())
            .await
            .unwrap();
        assert_eq!(report.evidence[0].grade, EvidenceGrade::A);
        assert_eq!(report.evidence[0].id, 1);
        assert_eq!(report.evidence[0].url, "https://example.com/shop/1");
        assert_eq!(report.evidence[1].grade, EvidenceGrade::B);
        assert!(llm.requests()[1].messages[1].content.contains("[1] (A · 卖家提供的一手观察)"));
    }

    #[tokio::test]
    async fn citing_a_nonexistent_evidence_card_is_sent_back_for_correction() {
        let llm = MockLlm::new([
            PLAN.to_string(),
            scored(&[opp("线缆夹", 4.0, "low", &[99])]),
            scored(&[opp("线缆夹", 4.0, "low", &[1])]),
            REVIEW.to_string(),
        ]);
        let search = MockSearch::new(1);
        let report = run_research(&llm, Some(&search), &brief(), &ResearchConfig::default(), &no_progress())
            .await
            .unwrap();
        assert_eq!(report.usage.json_retries, 1);
        assert_eq!(report.opportunities[0].evidence_ids, [1]);
        let requests = llm.requests();
        let correction = &requests[2].messages.last().unwrap().content;
        assert!(correction.contains("不存在的证据编号 [99]"), "{correction}");
    }

    #[tokio::test]
    async fn uncited_scores_and_oversized_products_are_rejected() {
        let known: HashSet<u32> = [1, 2].into_iter().collect();
        let parse = |json: String| serde_json::from_str::<RawScored>(&json).unwrap();

        let uncited = parse(scored(&[opp("线缆夹", 4.0, "low", &[])]));
        assert!(validate_scored(&uncited, &known, &brief()).unwrap_err().contains("没有引用任何证据卡"));

        let too_big = parse(scored(&[opp("落地灯", 4.0, "low", &[1])]).replace("\"size_mm\":60", "\"size_mm\":900"));
        assert!(validate_scored(&too_big, &known, &brief()).unwrap_err().contains("超过了成型空间"));

        let out_of_range = parse(scored(&[opp("线缆夹", 9.0, "low", &[1])]));
        assert!(validate_scored(&out_of_range, &known, &brief()).unwrap_err().contains("0~5"));

        let upside_down = parse(scored(&[opp("线缆夹", 4.0, "low", &[1])]).replace("\"price_low\":19", "\"price_low\":99"));
        assert!(validate_scored(&upside_down, &known, &brief()).unwrap_err().contains("价格带"));
    }

    #[tokio::test]
    async fn search_budget_caps_the_number_of_queries_and_duplicates_are_dropped() {
        let many = r#"{"queries":["a","b","a"," ","c","d","e"]}"#;
        let llm = MockLlm::new([many.to_string(), scored(&[opp("x", 3.0, "low", &[1])]), REVIEW.to_string()]);
        let search = MockSearch::new(1);
        let cfg = ResearchConfig {
            max_searches: 3,
            ..Default::default()
        };
        let report = run_research(&llm, Some(&search), &brief(), &cfg, &no_progress()).await.unwrap();
        assert_eq!(search.queries(), ["a", "b", "c"]);
        assert_eq!(report.usage.searches, 3);
    }

    #[tokio::test]
    async fn a_rejected_search_key_is_an_error_but_flaky_searches_only_degrade() {
        let llm = MockLlm::new([PLAN.to_string()]);
        let bad_key = MockSearch::failing(ProviderError::Auth);
        let err = run_research(&llm, Some(&bad_key), &brief(), &ResearchConfig::default(), &no_progress())
            .await
            .unwrap_err();
        assert_eq!(err, AgentError::Provider(ProviderError::Auth));

        let llm = MockLlm::new([PLAN.to_string(), scored(&[opp("x", 3.0, "low", &[])])]);
        let flaky = MockSearch::failing(ProviderError::Network("timeout".into()));
        let report = run_research(&llm, Some(&flaky), &brief(), &ResearchConfig::default(), &no_progress())
            .await
            .unwrap();
        assert!(report.degraded, "检索全部失败 = 没有证据 = 降级");
        assert_eq!(report.usage.searches, 0, "失败的检索不计费");
        assert_eq!(flaky.queries().len(), 3, "每条都试过了,没有因为第一条失败就放弃");
    }

    #[tokio::test]
    async fn a_failed_review_keeps_the_unreviewed_summary_and_says_so() {
        let llm = MockLlm::new([PLAN.to_string(), scored(&[opp("x", 3.0, "low", &[1])]), "坏的".into(), "还是坏的".into()]);
        let search = MockSearch::new(1);
        let report = run_research(&llm, Some(&search), &brief(), &ResearchConfig::default(), &no_progress())
            .await
            .unwrap();
        assert!(!report.reviewed);
        assert_eq!(report.summary_md, "线缆收纳需求稳定 [1]");
    }

    #[tokio::test]
    async fn review_can_be_switched_off_and_a_blank_topic_costs_nothing() {
        let llm = MockLlm::new([PLAN.to_string(), scored(&[opp("x", 3.0, "low", &[1])])]);
        let search = MockSearch::new(1);
        let cfg = ResearchConfig {
            critic_model: None,
            ..Default::default()
        };
        let report = run_research(&llm, Some(&search), &brief(), &cfg, &no_progress()).await.unwrap();
        assert!(!report.reviewed);
        assert_eq!(report.usage.llm_calls, 2);

        let untouched = MockLlm::new(Vec::<String>::new());
        let mut blank = brief();
        blank.topic = "  ".into();
        let err = run_research(&untouched, None, &blank, &ResearchConfig::default(), &no_progress())
            .await
            .unwrap_err();
        assert_eq!(err, AgentError::EmptyBrief);
        assert_eq!(untouched.calls(), 0);
    }

    #[test]
    fn long_excerpts_are_cut_on_character_boundaries() {
        assert_eq!(truncate_chars("打印", 5), "打印");
        assert_eq!(truncate_chars("一二三四五六", 3), "一二三…");
    }
}
