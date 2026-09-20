//! 代码式 CAD(build123d)的共享类型(docs/adr/0003-code-cad-build123d.md)。

use serde::{Deserialize, Serialize};

/// 执行器量出来的指标。B-rep 的精确值,不是网格近似。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CadMetrics {
    pub bbox_min: [f64; 3],
    pub bbox_max: [f64; 3],
    pub size: [f64; 3],
    pub volume_mm3: f64,
    pub area_mm2: f64,
    /// 实体个数:>1 说明模型是几个散件(通常是布尔运算没接上)
    pub solids: u32,
    pub faces: u32,
    pub edges: u32,
    /// OpenCascade 的 B-rep 有效性检查
    pub is_valid: bool,
}

impl CadMetrics {
    pub fn fits(&self, build: [f64; 3]) -> bool {
        self.size.iter().zip(build).all(|(s, b)| *s <= b)
    }
}

/// `# ---- PARAMS ----` 段里的一个参数(代码契约见 ADR-0003)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadParam {
    pub name: String,
    pub value: f64,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// 在代码里的行号(从 1 起)
    pub line: u32,
    /// 原始字面量是整数(数量、齿数…):面板上用步长 1,写回时也保持整数
    pub integer: bool,
}

/// 执行失败的原因——原样喂回给模型让它修,也展示给用户。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadScriptError {
    /// validate(白名单拒绝 / 语法错)· exec(运行时异常)· result(没有 result 或没有体积)· export
    pub stage: String,
    pub error_type: String,
    pub message: String,
    #[serde(default)]
    pub line: Option<u32>,
    /// 只含用户代码的帧
    #[serde(default)]
    pub traceback: String,
}

impl CadScriptError {
    /// 给模型看的修复提示。
    pub fn for_model(&self) -> String {
        let at = self.line.map(|l| format!(" (line {l})")).unwrap_or_default();
        let tb = if self.traceback.is_empty() {
            String::new()
        } else {
            format!("\n{}", self.traceback)
        };
        format!("[{}] {}: {}{at}{tb}", self.stage, self.error_type, self.message)
    }
}

/// 设计规格:视觉模型「看图」的产出,也是写代码的依据。先给人看、给人改,再去生成代码。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignSpec {
    pub name: String,
    #[serde(default)]
    pub summary: String,
    /// 适不适合代码式 CAD。人物、动物这类有机造型不适合——如实告诉用户,而不是硬生成一个四不像
    #[serde(default = "yes")]
    pub suitable: bool,
    #[serde(default)]
    pub unsuitable_reason: String,
    /// 总体尺寸(毫米)
    pub overall_mm: [f64; 3],
    pub features: Vec<DesignFeature>,
    /// 模型自己估的、图里看不出来的东西——要让用户看见
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub print_notes: String,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignFeature {
    pub name: String,
    pub description: String,
    /// 关键尺寸:名字 → 数值(毫米 / 个 / 度)
    #[serde(default)]
    pub dimensions: std::collections::BTreeMap<String, f64>,
}

/// 一版代码没通过检查的一条原因。带类型而不是一句话:喂给模型用 `for_model()`(英文,和提示词一致),
/// 给用户看则由前端按 `kind` 本地化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CadProblem {
    /// 不符合代码契约(缺 PARAMS / FEATURE 段、没有 `result =`)。`detail` 是给模型的祈使句
    Contract { detail: String },
    /// 代码跑不起来:白名单拒绝、语法错、运行时异常、没有体积、导出失败
    Script { error: CadScriptError },
    Timeout { secs: u64 },
    /// 不是恰好一个实体:0 = 没有体积,>1 = 散件(特征没和主体接上)
    Solids { count: u32 },
    /// OpenCascade 判定 B-rep 无效(自交、退化面…)
    Invalid,
    /// 最低点不在打印床上
    OffPlate { z: f64 },
    /// 包围盒和设计规格对不上
    Size { got: [f64; 3], want: [f64; 3] },
    /// 超出成型空间
    TooBig { size: [f64; 3], build: [f64; 3] },
    /// PARAMS 段里一个参数都解析不出来——参数面板就没法用
    NoParams,
    /// 参数的当前值不在它自己声明的范围里
    ParamRange {
        name: String,
        value: f64,
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
    /// 指令修补:交回来的代码和原来一模一样
    Unchanged,
}

fn fmt_mm(v: [f64; 3]) -> String {
    format!("{:.1} x {:.1} x {:.1}", v[0], v[1], v[2])
}

impl CadProblem {
    /// 给模型看的修复提示。
    pub fn for_model(&self) -> String {
        match self {
            CadProblem::Contract { detail } => detail.clone(),
            CadProblem::Script { error } => error.for_model(),
            CadProblem::Timeout { secs } => format!(
                "The script ran longer than {secs} s and was killed. Remove unbounded loops and simplify heavy operations (fewer booleans in loops, no tiny fillets on many edges)."
            ),
            CadProblem::Solids { count: 0 } => "`result` has no solid volume. Check that the booleans do not remove everything.".to_string(),
            CadProblem::Solids { count } => format!(
                "`result` consists of {count} separate solids. Every added feature must overlap (not just touch at an edge) the main body so the union gives ONE solid; position the loose pieces so they intersect the body by at least 0.2 mm."
            ),
            CadProblem::Invalid => "OpenCascade reports `result` as an invalid shape (self-intersection or degenerate faces). Avoid coincident faces in booleans (give cutting tools some over-travel) and reduce fillet radii.".to_string(),
            CadProblem::OffPlate { z } => format!(
                "The part must sit on the build plate: its lowest point is at z = {z:.3}, it has to be z = 0. Use align=(Align.CENTER, Align.CENTER, Align.MIN) for the main body and position the other features relative to it."
            ),
            CadProblem::Size { got, want } => format!(
                "The bounding box is {} mm but the spec asks for {} mm (X x Y x Z). Fix the dimensions (check radius vs diameter, and whether features stick out of the body).",
                fmt_mm(*got),
                fmt_mm(*want)
            ),
            CadProblem::TooBig { size, build } => format!(
                "The part is {} mm, which does not fit the printer's build volume of {} mm. Scale the PARAMS down.",
                fmt_mm(*size),
                fmt_mm(*build)
            ),
            CadProblem::NoParams => "The PARAMS section has no parsable lines. Each user-adjustable dimension must be one line of the form `name = number  # unit | label | [min, max]` with a plain numeric literal.".to_string(),
            CadProblem::ParamRange { name, value, min, max } => format!(
                "Parameter `{name}` = {value} is outside its own declared range [{}, {}]. Fix the value or the range.",
                min.map(|v| v.to_string()).unwrap_or_default(),
                max.map(|v| v.to_string()).unwrap_or_default()
            ),
            CadProblem::Unchanged => "You returned the script unchanged. Apply the requested change.".to_string(),
        }
    }
}

/// 一次「看图出规格」「生成」「指令修补」「看图复核」的花费与过程(还没入库)。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CadBuildReport {
    /// 调了几次模型、执行了几次代码
    pub llm_calls: u32,
    pub runs: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_fen: f64,
    pub elapsed_ms: u64,
    /// 每一轮没通过的原因(一轮可能有好几条)。最后一轮通过了,就比轮数少一项
    #[serde(default)]
    pub rounds: Vec<Vec<CadProblem>>,
    /// 交付的这一版仍然存在的问题(修复次数用完了)。模型建出来了,但要如实告诉用户
    #[serde(default)]
    pub warnings: Vec<CadProblem>,
    /// 指令修补时:相对上一版,改动了哪些段
    #[serde(default)]
    pub changed_sections: Vec<String>,
    /// 自动加了一行「贴到打印床」(模型忘了把零件放到 z = 0,这种不值得再花一次模型调用)
    #[serde(default)]
    pub plate_snapped: bool,
}

/// 看图复核的结论。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CadReview {
    pub matches: bool,
    #[serde(default)]
    pub differences: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CadEngineInfo {
    pub available: bool,
    #[serde(default)]
    pub python: String,
    #[serde(default)]
    pub python_version: String,
    #[serde(default)]
    pub build123d_version: String,
    /// 不可用时的原因(没装 / 起不来)
    #[serde(default)]
    pub problem: String,
    /// 安装包里带着引擎包(正式版都带;开发构建不带)
    #[serde(default)]
    pub bundled: bool,
    /// 带着引擎包、但还没解开(或解开的是旧版本)→ 界面应当直接开始安装,不用用户点
    #[serde(default)]
    pub needs_install: bool,
    /// 引擎包解开后占多少字节(给用户一个预期)
    #[serde(default)]
    pub unpacked_bytes: u64,
}

/// 模型的一个版本。每次生成、改参数、指令修补都产生一个新版本(父子关系 = 版本树)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadVersion {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    /// manual(手写 / 粘贴)· generate(看图生成)· param(改参数)· edit(指令修补)· repair(自动修复)
    pub source: String,
    /// 这一版是怎么来的:改了哪个参数、用户的指令原文…
    #[serde(default)]
    pub note: String,
    pub code: String,
    /// 这一版依据的设计规格。改参数 / 指令修补出来的子版本沿用父版本的(复核时要用)
    #[serde(default)]
    pub spec: Option<DesignSpec>,
    /// 参考图(资产编号)。同样由子版本沿用
    #[serde(default)]
    pub ref_asset_ids: Vec<String>,
    /// 这一版是怎么来的:花了多少、修了几轮、遗留什么问题、动了哪些段
    #[serde(default)]
    pub report: Option<CadBuildReport>,
    pub params: Vec<CadParam>,
    pub metrics: CadMetrics,
    /// 预览用的网格(STL)与源头真值(STEP)
    pub stl_asset_id: String,
    #[serde(default)]
    pub step_asset_id: Option<String>,
    pub elapsed_ms: u64,
    pub created_at: i64,
}

/// 「看图出规格」的结果:规格 + 入了库的参考图 + 花费。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadSpecResult {
    pub spec: DesignSpec,
    pub ref_asset_ids: Vec<String>,
    pub report: CadBuildReport,
}

/// 「看图复核」的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadReviewResult {
    pub review: CadReview,
    pub report: CadBuildReport,
}

/// 用户在 3D 视图里点中的位置(毫米,模型坐标系:Z 朝上)。指令修补时帮模型判断「说的是哪个特征」。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CadPick {
    pub point: [f64; 3],
    #[serde(default)]
    pub normal: Option<[f64; 3]>,
}

/// 「生成 / 指令修补」的结果。`version = None` 表示没有任何一版能跑:
/// 这时 `code` 是最后一版代码、`report.rounds` 里是每一轮的报错——用户可以接着手改,而不是只看到一句「失败」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadBuildResult {
    #[serde(default)]
    pub version: Option<CadVersion>,
    pub code: String,
    pub report: CadBuildReport,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_errors_are_rendered_compactly_for_the_model() {
        let e = CadScriptError {
            stage: "exec".into(),
            error_type: "ValueError".into(),
            message: "Failed creating a fillet".into(),
            line: Some(14),
            traceback: "  line 14: part = fillet(part.edges(), 50)\nValueError: Failed creating a fillet".into(),
        };
        let text = e.for_model();
        assert!(text.starts_with("[exec] ValueError: Failed creating a fillet (line 14)"));
        assert!(text.contains("line 14: part = fillet"));
    }

    #[test]
    fn problems_travel_as_tagged_objects_so_the_ui_can_localize_them() {
        let p = CadProblem::Solids { count: 3 };
        let wire = serde_json::to_value(&p).unwrap();
        assert_eq!(wire, serde_json::json!({"kind": "solids", "count": 3}));
        assert_eq!(serde_json::from_value::<CadProblem>(wire).unwrap(), p);
        assert_eq!(serde_json::to_value(CadProblem::Invalid).unwrap(), serde_json::json!({"kind": "invalid"}));
        // 给模型的话要点出数字——「3 个散件」比「不是一个实体」好修
        assert!(p.for_model().contains("3 separate solids"));
        assert!(CadProblem::Solids { count: 0 }.for_model().contains("no solid volume"));
        let size = CadProblem::Size {
            got: [60.0, 24.0, 12.0],
            want: [60.0, 24.0, 18.0],
        };
        assert!(size.for_model().contains("60.0 x 24.0 x 12.0") && size.for_model().contains("60.0 x 24.0 x 18.0"));
    }

    #[test]
    fn fits_checks_every_axis() {
        let m = CadMetrics {
            size: [100.0, 300.0, 20.0],
            ..Default::default()
        };
        assert!(!m.fits([256.0, 256.0, 256.0]));
        assert!(m.fits([256.0, 300.0, 256.0]));
    }
}
