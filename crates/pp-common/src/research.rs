//! 市场调研的数据结构(docs/01-prd.md M1、docs/03-architecture.md §7)。
//! 后端流水线产出,前端报告页展示,所以放在共享 crate 里。

use serde::{Deserialize, Serialize};

/// 一次调研的输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchBrief {
    /// 品类 / 关键词 / 场景,例如「桌面收纳」「猫咪周边」
    pub topic: String,
    /// 打印机成型空间(毫米)
    #[serde(default = "default_build")]
    pub build_mm: [f64; 3],
    #[serde(default = "default_materials")]
    pub materials: String,
    /// 目标价位(元),可不填
    #[serde(default)]
    pub price_min: Option<u32>,
    #[serde(default)]
    pub price_max: Option<u32>,
    #[serde(default = "default_channel")]
    pub channel: String,
    /// 用户补充的一手证据:竞品链接、自己的观察。搜索接口搜不到平台站内内容,这是调研质量的关键输入
    #[serde(default)]
    pub user_notes: Vec<String>,
}

/// 与 serde 的缺省值保持一致:无论是代码里 `..Default::default()` 还是从 JSON 反序列化,得到的都是同一套出厂约束。
impl Default for ResearchBrief {
    fn default() -> Self {
        Self {
            topic: String::new(),
            build_mm: default_build(),
            materials: default_materials(),
            price_min: None,
            price_max: None,
            channel: default_channel(),
            user_notes: Vec::new(),
        }
    }
}

fn default_build() -> [f64; 3] {
    [256.0, 256.0, 256.0]
}

fn default_materials() -> String {
    "PLA".into()
}

fn default_channel() -> String {
    "小红书".into()
}

/// 证据等级:A = 平台一手数据 / 用户提供;B = 媒体、博客、帖子;C = 模型推断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceGrade {
    A,
    B,
    C,
}

/// 证据卡:报告里的每个数字型结论都要能指回这里的一条。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: u32,
    pub title: String,
    /// 用户提供的证据没有链接
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub published: Option<String>,
    pub excerpt: String,
    pub grade: EvidenceGrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpRisk {
    Low,
    Medium,
    /// 一票否决:不参与加权,机会卡不能直接采用
    High,
}

/// 建模路线建议:AI 图生 3D 不擅长尺寸精确的功能件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoute {
    /// 有机 / 装饰造型 → AI 生成
    AiGenerated,
    /// 尺寸精确的功能件、个性化定制 → 参数化模板
    Parametric,
    /// 需要手工 CAD 建模后导入
    Import,
}

/// 六个加权维度,各 0~5 分。「反向」维度(竞争、物流麻烦)在这里已经换算成「越高越好」。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Scores {
    /// 需求热度
    pub demand: f64,
    /// 差异化与定制潜力
    pub differentiation: f64,
    /// 竞争宽松度(竞争越激烈分越低)
    pub competition: f64,
    /// 可打印性
    pub printability: f64,
    /// 毛利空间
    pub margin: f64,
    /// 物流友好度
    pub logistics: f64,
}

/// 默认权重(docs/05-growth-cro.md §9)。IP 与合规风险不参与加权,「高」直接一票否决。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoreWeights {
    pub demand: f64,
    pub differentiation: f64,
    pub competition: f64,
    pub printability: f64,
    pub margin: f64,
    pub logistics: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self {
            demand: 25.0,
            differentiation: 15.0,
            competition: 15.0,
            printability: 15.0,
            margin: 15.0,
            logistics: 5.0,
        }
    }
}

impl Scores {
    pub fn as_array(&self) -> [f64; 6] {
        [
            self.demand,
            self.differentiation,
            self.competition,
            self.printability,
            self.margin,
            self.logistics,
        ]
    }

    pub fn in_range(&self) -> bool {
        self.as_array().iter().all(|v| v.is_finite() && (0.0..=5.0).contains(v))
    }

    /// 加权总分(0~5)。
    pub fn total(&self, w: &ScoreWeights) -> f64 {
        let weights = [w.demand, w.differentiation, w.competition, w.printability, w.margin, w.logistics];
        let sum: f64 = weights.iter().sum();
        if sum <= 0.0 {
            return 0.0;
        }
        self.as_array().iter().zip(weights).map(|(s, w)| s * w).sum::<f64>() / sum
    }
}

/// 机会卡:一个具体的、可以立项的产品机会。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opportunity {
    pub title: String,
    /// 一句话定位
    pub pitch: String,
    pub persona: String,
    pub selling_points: Vec<String>,
    /// 建议价格带(元)
    pub price_low: u32,
    pub price_high: u32,
    pub scores: Scores,
    pub ip_risk: IpRisk,
    /// 评分依据(要引用证据编号)
    pub rationale: String,
    /// 引用的证据卡编号
    pub evidence_ids: Vec<u32>,
    #[serde(default)]
    pub risks: Vec<String>,
    pub route: ModelRoute,
    /// 量级估算:最长边(毫米)与克数,给成本预估用
    #[serde(default)]
    pub size_mm: f64,
    #[serde(default)]
    pub grams: f64,
    /// 加权总分,由流水线按权重算出(不信模型自己报的总分)
    #[serde(default)]
    pub total: f64,
}

impl Opportunity {
    /// IP 风险高 = 一票否决:可以看,但不能直接「采用为项目」。
    pub fn vetoed(&self) -> bool {
        self.ip_risk == IpRisk::High
    }
}

/// 一次调研花了多少。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ResearchUsage {
    pub llm_calls: u32,
    pub searches: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// 总花费(分)
    pub cost_fen: f64,
    pub elapsed_ms: u64,
    /// 结构化输出失败后重试的次数(预研 ③ 的观测指标)
    pub json_retries: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchReport {
    /// 报告正文(Markdown,带 [n] 引用)
    pub summary_md: String,
    /// 按总分从高到低;被否决的排在最后
    pub opportunities: Vec<Opportunity>,
    pub evidence: Vec<Evidence>,
    pub usage: ResearchUsage,
    /// 没配搜索密钥(或检索全部失败):退化为「用户证据 + 模型知识」,报告页顶部要醒目标注
    pub degraded: bool,
    /// 结论是否经过了强模型审校(把没有证据的断言降级为「推测」)。审校失败不影响出报告,但要如实标注
    pub reviewed: bool,
}

/// 调研历史列表里的一行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchRunSummary {
    pub id: String,
    pub topic: String,
    pub created_at: i64,
    /// 花费(分,向上取整)
    pub cost_fen: i64,
    pub opportunities: u32,
}

/// 一次调研的完整结果 + 它在库里的编号(「采用为项目」要用)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedResearch {
    pub run_id: String,
    pub brief: ResearchBrief,
    pub report: ResearchReport,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_is_a_weighted_average_on_the_same_0_to_5_scale() {
        let all_fives = Scores {
            demand: 5.0,
            differentiation: 5.0,
            competition: 5.0,
            printability: 5.0,
            margin: 5.0,
            logistics: 5.0,
        };
        assert!((all_fives.total(&ScoreWeights::default()) - 5.0).abs() < 1e-12);

        // 需求权重最大:只有需求满分时,总分 = 5 × 25/90
        let only_demand = Scores {
            demand: 5.0,
            ..Default::default()
        };
        assert!((only_demand.total(&ScoreWeights::default()) - 5.0 * 25.0 / 90.0).abs() < 1e-12);
    }

    #[test]
    fn zero_weights_do_not_divide_by_zero() {
        let w = ScoreWeights {
            demand: 0.0,
            differentiation: 0.0,
            competition: 0.0,
            printability: 0.0,
            margin: 0.0,
            logistics: 0.0,
        };
        assert_eq!(Scores::default().total(&w), 0.0);
    }

    #[test]
    fn scores_must_stay_within_zero_to_five() {
        let mut s = Scores::default();
        assert!(s.in_range());
        s.margin = 5.1;
        assert!(!s.in_range());
        s.margin = f64::NAN;
        assert!(!s.in_range());
    }

    #[test]
    fn brief_fills_in_defaults_from_minimal_json() {
        let b: ResearchBrief = serde_json::from_str(r#"{ "topic": "桌面收纳" }"#).unwrap();
        assert_eq!(b.build_mm, [256.0, 256.0, 256.0]);
        assert_eq!(b.materials, "PLA");
        assert_eq!(b.channel, "小红书");
        assert!(b.user_notes.is_empty());
        // 代码里的 Default 与 JSON 缺省值必须是同一套
        assert_eq!(
            b,
            ResearchBrief {
                topic: "桌面收纳".into(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn enums_use_snake_case_on_the_wire() {
        assert_eq!(serde_json::to_string(&IpRisk::High).unwrap(), "\"high\"");
        assert_eq!(serde_json::to_string(&ModelRoute::AiGenerated).unwrap(), "\"ai_generated\"");
        assert_eq!(serde_json::to_string(&EvidenceGrade::B).unwrap(), "\"B\"");
    }
}
