//! 有界流水线式的 Agent(docs/03-architecture.md §7)。
//!
//! 为什么不是开放式的「模型自己决定下一步」循环:步骤固定、每步有预算、产出有结构,
//! 质量稳定、成本可控,而且能用脚本化的假模型做回归测试——这三点对一个要花用户钱的功能都是必须的。

pub mod cad;
mod json;
pub mod research;

pub use cad::{design_spec, edit_model, generate_model, review_model, CadBuild, CadConfig, CadExecutor, CadProgress};
pub use json::{ask_json, extract_json};
pub use research::{run_research, ResearchConfig, ResearchProgress};

use pp_cad::CadError;
use pp_providers::ProviderError;

#[derive(Debug, Clone, PartialEq)]
pub enum AgentError {
    /// 输入是空的(调研方向 / 建模要求 / 修改指令)
    EmptyBrief,
    Provider(ProviderError),
    /// 模型的输出重试之后仍然不合格(结构不对、评分越界、引用了不存在的证据…)
    InvalidOutput(String),
    /// 建模引擎本身出了问题(没装、起不来)——不是模型改代码能解决的
    Engine(CadError),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::EmptyBrief => write!(f, "the request is empty"),
            AgentError::Provider(e) => write!(f, "{e}"),
            AgentError::InvalidOutput(why) => write!(f, "model output rejected: {why}"),
            AgentError::Engine(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<ProviderError> for AgentError {
    fn from(e: ProviderError) -> Self {
        AgentError::Provider(e)
    }
}

impl AgentError {
    pub fn code(&self) -> &'static str {
        match self {
            AgentError::EmptyBrief => "invalid_input",
            AgentError::Provider(e) => e.code(),
            AgentError::InvalidOutput(_) => "agent_bad_output",
            AgentError::Engine(e) => e.code(),
        }
    }
}
