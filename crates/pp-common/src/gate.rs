//! 阶段门:项目是主线,三个工作台(调研 / 图片 / 建模)的产出挂在项目名下;
//! **每个阶段「该完成什么」从这些产出里算出来**,而不是让人手工打勾。
//!
//! 清单只回答一个问题:「离开这个阶段之前,该有的东西有了吗?」有了 → 提示可以进入下一阶段;
//! 没有 → 照样可以推进,但要写一句原因(`stage_events.forced`)——复盘时这是很值钱的数据。
//! 运营、复盘两个阶段的工具还没做出来,清单是空的,只能人工判断。

use serde::{Deserialize, Serialize};

use crate::design::CadDesignSummary;
use crate::imagery::ImageBoardSummary;
use crate::research::ResearchRunSummary;
use crate::stage::Stage;

/// 项目名下各工作台的产出(计数)。阶段门清单、看板卡片上的小图标都从它来。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFacts {
    /// 写下了要验证的假设
    #[serde(default)]
    pub has_hypothesis: bool,
    #[serde(default)]
    pub research_runs: u32,
    #[serde(default)]
    pub boards: u32,
    #[serde(default)]
    pub images: u32,
    #[serde(default)]
    pub adopted_images: u32,
    #[serde(default)]
    pub designs: u32,
    /// 已经建出模型的设计(有当前版本)
    #[serde(default)]
    pub models: u32,
    /// 成功的打样记录
    #[serde(default)]
    pub print_successes: u32,
    /// 成本模型里定了售价
    #[serde(default)]
    pub has_price: bool,
    /// 出过的发布包(上架,M5)
    #[serde(default)]
    pub publish_packs: u32,
    /// 回填了发布链接的笔记 / 商品
    #[serde(default)]
    pub published: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKey {
    /// 写下要验证的假设(谁会买、为什么、愿意付多少)
    Hypothesis,
    /// 至少一份调研报告
    Research,
    /// 至少采用了一张预览图
    AdoptedImage,
    /// 至少建出了一个模型
    Model,
    /// 至少一次成功的打样记录
    PrintRun,
    /// 在成本定价器里定了售价
    Price,
    /// 出过一个发布包(处理好的图 + 过了检查的文案)
    PublishPack,
    /// 发出去了:回填了笔记 / 商品的链接
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateItem {
    pub key: GateKey,
    pub done: bool,
}

/// 离开 `stage` 之前该完成什么。空 = 这个阶段的工具还没做出来,只能人工判断。
pub fn stage_gate(stage: Stage, facts: &ProjectFacts) -> Vec<GateItem> {
    let item = |key, done| GateItem { key, done };
    match stage {
        Stage::Idea => vec![item(GateKey::Hypothesis, facts.has_hypothesis)],
        Stage::Research => vec![item(GateKey::Research, facts.research_runs > 0)],
        Stage::Concept => vec![item(GateKey::AdoptedImage, facts.adopted_images > 0)],
        Stage::Model => vec![item(GateKey::Model, facts.models > 0)],
        // 打样:真的打成过一次,并且算过账、定了价(成本定价器,M4)
        Stage::Prototype => vec![item(GateKey::PrintRun, facts.print_successes > 0), item(GateKey::Price, facts.has_price)],
        // 上架:出过发布包,并且真的发出去了(回填了链接)
        Stage::Listing => vec![item(GateKey::PublishPack, facts.publish_packs > 0), item(GateKey::Published, facts.published > 0)],
        Stage::Operating | Stage::Review => Vec::new(),
    }
}

/// 当前阶段的清单全部完成(空清单不算:那是「没法自动判断」,不是「完成了」)。
pub fn gate_passed(stage: Stage, facts: &ProjectFacts) -> bool {
    let items = stage_gate(stage, facts);
    !items.is_empty() && items.iter().all(|i| i.done)
}

/// 从 `from` 往前挪到 `to`,路过的阶段里还没完成的清单项。往回挪(返工)永远是空。
pub fn unmet_on_the_way(from: Stage, to: Stage, facts: &ProjectFacts) -> Vec<(Stage, GateKey)> {
    Stage::ALL
        .into_iter()
        .filter(|s| s.index() >= from.index() && s.index() < to.index())
        .flat_map(|s| stage_gate(s, facts).into_iter().filter(|i| !i.done).map(move |i| (s, i.key)))
        .collect()
}

/// 这次挪动要不要写一句原因。
/// - 往回挪(返工)、原地不动:不用;
/// - 路过的阶段有清单项没完成:要;
/// - **整个跳过**了一个没法自动判断的阶段(比如从「建模」直接到「上架」,跳过了打样):要。
///   只是顺着走出这样一个阶段(「打样」→「上架」)不用。
pub fn needs_reason(from: Stage, to: Stage, facts: &ProjectFacts) -> bool {
    if to.index() <= from.index() {
        return false;
    }
    if !unmet_on_the_way(from, to, facts).is_empty() {
        return true;
    }
    Stage::ALL
        .into_iter()
        .filter(|s| s.index() > from.index() && s.index() < to.index())
        .any(|s| stage_gate(s, facts).is_empty())
}

/// 项目名下的一张图(给项目中枢展示用)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectImage {
    pub version_id: String,
    pub board_id: String,
    pub asset_id: String,
    pub adopted: bool,
}

/// 项目中枢要的全部东西:名下三个工作台的产出 + 到现在花了多少钱。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectOverview {
    pub facts: ProjectFacts,
    /// 最新的在前
    #[serde(default)]
    pub research: Vec<ResearchRunSummary>,
    #[serde(default)]
    pub boards: Vec<ImageBoardSummary>,
    /// 采用的在前,其次按时间;最多 12 张
    #[serde(default)]
    pub images: Vec<ProjectImage>,
    #[serde(default)]
    pub designs: Vec<CadDesignSummary>,
    /// 这个项目到现在的 AI 花费(分)
    #[serde(default)]
    pub cost_fen: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> ProjectFacts {
        ProjectFacts::default()
    }

    #[test]
    fn each_early_stage_asks_for_the_output_of_its_own_workbench() {
        let mut f = facts();
        assert!(!gate_passed(Stage::Idea, &f));
        f.has_hypothesis = true;
        assert!(gate_passed(Stage::Idea, &f));

        assert!(!gate_passed(Stage::Research, &f));
        f.research_runs = 1;
        assert!(gate_passed(Stage::Research, &f));

        f.images = 4;
        assert!(!gate_passed(Stage::Concept, &f), "出了图但一张都没采用,不算");
        f.adopted_images = 1;
        assert!(gate_passed(Stage::Concept, &f));

        f.designs = 1;
        assert!(!gate_passed(Stage::Model, &f), "只有设计、还没建出模型,不算");
        f.models = 1;
        assert!(gate_passed(Stage::Model, &f));

        // 打样要两样都有:打成过一次 + 定了价
        f.print_successes = 1;
        assert!(!gate_passed(Stage::Prototype, &f), "只打成了、还没定价,不算");
        assert_eq!(unmet_on_the_way(Stage::Prototype, Stage::Listing, &f), vec![(Stage::Prototype, GateKey::Price)]);
        f.has_price = true;
        assert!(gate_passed(Stage::Prototype, &f));
    }

    #[test]
    fn stages_without_tools_yet_can_never_pass_automatically() {
        let f = ProjectFacts {
            has_hypothesis: true,
            research_runs: 9,
            boards: 9,
            images: 9,
            adopted_images: 9,
            designs: 9,
            models: 9,
            print_successes: 9,
            has_price: true,
            publish_packs: 9,
            published: 9,
        };
        for s in [Stage::Operating, Stage::Review] {
            assert!(stage_gate(s, &f).is_empty());
            assert!(!gate_passed(s, &f), "{s:?}:空清单是「没法判断」,不是「完成了」");
        }
    }

    #[test]
    fn a_reason_is_needed_exactly_when_the_evidence_is_missing() {
        let mut f = facts();
        // 什么都没有:连顺着走一步都要说明(以前只有跳阶段才问)
        assert!(needs_reason(Stage::Idea, Stage::Research, &f));
        f.has_hypothesis = true;
        assert!(!needs_reason(Stage::Idea, Stage::Research, &f));

        // 证据齐了,一次跨两个阶段也不用解释(以前只要跨阶段就问)
        f.research_runs = 1;
        assert!(!needs_reason(Stage::Idea, Stage::Concept, &f));
        assert_eq!(unmet_on_the_way(Stage::Idea, Stage::Model, &f), vec![(Stage::Concept, GateKey::AdoptedImage)]);
        assert!(needs_reason(Stage::Idea, Stage::Model, &f));

        // 往回拖 = 返工,永远不用解释
        assert!(!needs_reason(Stage::Listing, Stage::Model, &facts()));
        assert!(!needs_reason(Stage::Model, Stage::Model, &facts()));
    }

    #[test]
    fn walking_out_of_a_manual_stage_is_fine_but_skipping_one_is_not() {
        let f = ProjectFacts {
            has_hypothesis: true,
            research_runs: 1,
            adopted_images: 1,
            models: 1,
            print_successes: 1,
            has_price: true,
            ..Default::default()
        };
        assert!(!needs_reason(Stage::Model, Stage::Listing, &f), "打样的证据齐了,一次走两步也不拦");
        // 上架(M5)从此有清单:出过发布包 + 回填了链接
        assert!(needs_reason(Stage::Listing, Stage::Operating, &f), "还没发出去就想进运营");
        let listed = ProjectFacts {
            publish_packs: 1,
            published: 1,
            ..f
        };
        assert!(!needs_reason(Stage::Listing, Stage::Operating, &listed));
        assert!(!needs_reason(Stage::Operating, Stage::Review, &listed), "运营的工具还没做,顺着走不拦");
        assert!(needs_reason(Stage::Listing, Stage::Review, &listed), "整个跳过了运营(没法自动判断的阶段)");
    }

    #[test]
    fn gate_keys_are_snake_case_on_the_wire() {
        assert_eq!(serde_json::to_string(&GateKey::AdoptedImage).unwrap(), "\"adopted_image\"");
        let f: ProjectFacts = serde_json::from_str("{}").unwrap();
        assert_eq!(f, ProjectFacts::default(), "旧前端 / 旧后端少字段也能读");
    }
}
