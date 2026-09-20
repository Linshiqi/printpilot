//! 项目阶段:一个单品从想法到复盘的 8 个阶段(定义见 docs/01-prd.md §4)。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Idea,
    Research,
    Concept,
    Model,
    Prototype,
    Listing,
    Operating,
    Review,
}

impl Stage {
    /// 按流水线顺序排列,看板的列顺序就是它。
    pub const ALL: [Stage; 8] = [
        Stage::Idea,
        Stage::Research,
        Stage::Concept,
        Stage::Model,
        Stage::Prototype,
        Stage::Listing,
        Stage::Operating,
        Stage::Review,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Idea => "idea",
            Stage::Research => "research",
            Stage::Concept => "concept",
            Stage::Model => "model",
            Stage::Prototype => "prototype",
            Stage::Listing => "listing",
            Stage::Operating => "operating",
            Stage::Review => "review",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|st| st.as_str() == s)
    }

    pub fn index(self) -> usize {
        Stage::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn next(self) -> Option<Stage> {
        Stage::ALL.get(self.index() + 1).copied()
    }

    /// 停留超过这个天数,看板上高亮提醒——速度是第一指标(docs/02-ux-flows.md §3.1)。
    /// 运营与复盘是长期阶段,不提醒。
    pub fn stale_after_days(self) -> Option<u32> {
        match self {
            Stage::Idea => Some(7),
            Stage::Research => Some(2),
            Stage::Concept => Some(4),
            Stage::Model => Some(3),
            Stage::Prototype => Some(3),
            Stage::Listing => Some(2),
            Stage::Operating | Stage::Review => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    Paused,
    Killed,
    Done,
}

impl ProjectStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectStatus::Active => "active",
            ProjectStatus::Paused => "paused",
            ProjectStatus::Killed => "killed",
            ProjectStatus::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<ProjectStatus> {
        [
            ProjectStatus::Active,
            ProjectStatus::Paused,
            ProjectStatus::Killed,
            ProjectStatus::Done,
        ]
        .into_iter()
        .find(|st| st.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_roundtrip_for_every_stage() {
        for s in Stage::ALL {
            assert_eq!(Stage::parse(s.as_str()), Some(s));
        }
        assert_eq!(Stage::parse("nope"), None);
    }

    #[test]
    fn next_walks_the_pipeline_and_stops() {
        let mut cur = Stage::Idea;
        let mut seen = vec![cur];
        while let Some(n) = cur.next() {
            seen.push(n);
            cur = n;
        }
        assert_eq!(seen, Stage::ALL);
        assert_eq!(Stage::Review.next(), None);
    }

    #[test]
    fn serde_string_matches_as_str() {
        // 前后端(serde)与数据库(as_str)用的必须是同一套字符串,这里锁住
        for s in Stage::ALL {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s.as_str()));
            assert_eq!(serde_json::from_str::<Stage>(&json).unwrap(), s);
        }
        for st in ["active", "paused", "killed", "done"] {
            let parsed = ProjectStatus::parse(st).unwrap();
            assert_eq!(serde_json::to_string(&parsed).unwrap(), format!("\"{st}\""));
        }
    }
}
