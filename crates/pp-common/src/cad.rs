//! 代码式 CAD(build123d)的共享类型(docs/adr/0003-code-cad-build123d.md)。

use serde::{Deserialize, Serialize};

/// 执行器量出来的指标。体积 / 面积是 B-rep 的精确值。包围盒在平面 / 圆柱 / 圆锥 / 球面上也是精确的;
/// 环面和自由曲面上用三角网量——只会偏小、且不超过弦差 0.02 mm(原因和实测见 `runner.py` 的 `bounds`)。
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
    /// 贴在打印床上的平面面积(mm²)。`None` = 这一版是旧执行器量的,没有这一项
    #[serde(default)]
    pub bed_contact_mm2: Option<f64>,
    /// 离开床面、正朝下的平面(天花板 / 悬臂 / 桥)的面积(mm²):这些地方要么搭桥、要么加支撑
    #[serde(default)]
    pub overhang_mm2: Option<f64>,
}

/// 贴床面积小于这个数 = 零件实际上是靠一条边、一个点或一个曲面着床的(球、躺着的圆柱)。
pub const MIN_BED_CONTACT_MM2: f64 = 1.0;
/// 最低点离床面多远算「没贴床」。比执行器的弦差(0.02 mm)大:底部是环面 / 自由曲面的零件,
/// 最低点是用三角网量的,会偏高最多 0.02——它不该被当成悬空。
pub const PLATE_TOLERANCE_MM: f64 = 0.05;

/// 打印时要留意的事,从指标里算出来,指标卡上一直显示。它们不喂给修复循环:
/// 对话里用户想要一个球,就该给他一个球——只是要让他知道这个球直接打不了。
/// (按规格**首次生成**时「没有平的底面」另外算一条问题,见 `CadProblem::NoFlatBase`。)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrintNote {
    /// 没有平的底面:靠一条边、一个点或一个曲面着床
    NoFlatBase,
    /// 贴床面积相对零件的占地很小:首层容易粘不住,切片时加裙边(brim)
    SmallContact { contact_mm2: f64, footprint_mm2: f64 },
    /// 有朝下的悬空平面:要支撑,或者本来就是可以搭桥的短跨
    Overhang { area_mm2: f64 },
}

impl CadMetrics {
    pub fn fits(&self, build: [f64; 3]) -> bool {
        self.size.iter().zip(build).all(|(s, b)| *s <= b)
    }

    /// 零件在床上的占地(包围盒的 X × Y)。
    pub fn footprint_mm2(&self) -> f64 {
        self.size[0] * self.size[1]
    }

    /// 零件有没有一个平的底面贴在床上。量不出来(旧版本)时当作有——不拿没有的数据去判人家不合格。
    pub fn rests_flat(&self) -> bool {
        self.bed_contact_mm2.is_none_or(|c| c >= MIN_BED_CONTACT_MM2)
    }

    /// 给用户的打印提醒(见 `PrintNote`)。
    pub fn print_notes(&self) -> Vec<PrintNote> {
        let mut notes = Vec::new();
        let footprint = self.footprint_mm2();
        if !self.rests_flat() {
            notes.push(PrintNote::NoFlatBase);
        }
        if let Some(contact) = self.bed_contact_mm2 {
            // 着床的面不到占地的 3%(且绝对值也不大):细腿、尖脚这类
            if contact >= MIN_BED_CONTACT_MM2 && contact < footprint * 0.03 && contact < 400.0 {
                notes.push(PrintNote::SmallContact {
                    contact_mm2: contact,
                    footprint_mm2: footprint,
                });
            }
        }
        if let Some(area) = self.overhang_mm2 {
            // 零头不提:倒角留下的小台阶、文字的内腔
            if area >= 25.0 {
                notes.push(PrintNote::Overhang { area_mm2: area });
            }
        }
        notes
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
    /// 给模型看的修复提示:原始错误 + 用户代码的帧 + (认得出来的话)一句「这种错通常怎么修」。
    pub fn for_model(&self) -> String {
        let at = self.line.map(|l| format!(" (line {l})")).unwrap_or_default();
        let tb = if self.traceback.is_empty() {
            String::new()
        } else {
            format!("\n{}", self.traceback)
        };
        let hint = self.hint().map(|h| format!("\nHint: {h}")).unwrap_or_default();
        format!("[{}] {}: {}{at}{tb}{hint}", self.stage, self.error_type, self.message)
    }

    /// 常见失败的修法。build123d / OpenCascade 的原始报错经常说不到点子上——最典型的是
    /// 「选择器什么都没选中」报出来是一句 `IndexError: list index out of range`。模型光看这句话只能猜,
    /// 猜错一轮就是一次十几秒的调用。这张表里的每个签名都是在真引擎上跑出来的(`target/` 下的一次性探测脚本),
    /// 不是凭印象写的;认不出来就不加提示,原始错误照旧给。
    pub fn hint(&self) -> Option<&'static str> {
        let (ty, msg) = (self.error_type.as_str(), self.message.as_str());
        if self.stage == "validate" {
            return None; // 白名单 / 语法错:执行器的消息本身就写了该怎么改
        }
        if self.stage == "result" {
            return msg.contains("no solid volume").then_some(
                "`result` must be a solid. A sketch or face has to be extruded first; selectors (`.edges()`, `.faces()`) return lists, not shapes; and a cutter that is bigger than the body removes everything.",
            );
        }
        if msg.contains("Failed creating a fillet") {
            return Some(
                "the fillet does not fit. A fillet radius must be smaller than half of the thinnest adjacent wall AND shorter than the adjacent edges, and neighbouring fillets compete for the same material. Reduce the radius (derive it, e.g. `min(corner_radius, wall / 2 - 0.1)`), fillet only the edges that matter instead of `part.edges()`, and apply fillets after all booleans.",
            );
        }
        if msg.contains("Failed creating a chamfer") {
            return Some(
                "the chamfer does not fit. Its length must be smaller than the adjacent faces allow (less than half of the thinnest wall). Reduce the length and chamfer only the edges that matter instead of `part.edges()`.",
            );
        }
        match ty {
            "IndexError" => Some(
                "in modeling code an IndexError almost always means a selector came back with fewer items than expected: `filter_by(...)` matched nothing, or `group_by(...)` / `sort_by(...)` has fewer groups than the index asks for. `fillet()` / `chamfer()` on an EMPTY edge list fails this way too. Selectors see the shape as it is at that line (after the booleans above it): select from the shape that really has those edges, and prefer [0] / [-1] over other indexes.",
            ),
            "NameError" => Some(
                "that name does not exist in build123d 0.12 (or it is a variable that was never defined). Do not invent API names; build the shape from the primitives and operations in the cheat sheet.",
            ),
            "AttributeError" => Some(
                "that attribute / method does not exist on this object in build123d 0.12. Use the free functions from the cheat sheet — `fillet(...)`, `chamfer(...)`, `extrude(...)`, `offset(...)`, `mirror(...)` — instead of methods, and remember that selectors return lists.",
            ),
            "TypeError" if msg.contains("unsupported operand type(s) for *") => Some(
                "locations go on the LEFT of the shape: `Pos(x, y, z) * shape`, `Pos(...) * Rot(...) * shape`. `shape * Pos(...)` is not defined.",
            ),
            "TypeError" => Some(
                "wrong call signature. Use exactly the signatures from the cheat sheet, e.g. `Box(length, width, height, align=...)`, `Cylinder(radius, height, align=...)`. There is no `center=` / `centered=` flag: alignment is `align=(Align.CENTER, Align.CENTER, Align.MIN)`.",
            ),
            "ValueError" if msg.contains("is not a valid Align") => Some(
                "too many positional arguments: the extra value landed in `align=`. Check the signature in the cheat sheet (`Box(length, width, height)`, `Cylinder(radius, height)`).",
            ),
            "ValueError" if msg.contains("No depth provided") || msg.contains("context") => Some(
                "`Hole`, `CounterBoreHole`, `Locations` and friends are builder-mode helpers that need a `with BuildPart():` context. In algebra mode cut a hole with `body - Pos(x, y, z) * Cylinder(radius, height)`.",
            ),
            "ZeroDivisionError" => Some(
                "a parameter is 0 where the code divides by it. Guard the division (e.g. `max(count, 1)`) or give that parameter a minimum of 1 in its PARAMS range.",
            ),
            // OpenCascade 内核的异常:StdFail_NotDone、Standard_ConstructionError、Standard_NullObject…
            _ if ty.starts_with("StdFail_") || ty.starts_with("Standard_") => Some(
                "the OpenCascade kernel could not compute the operation at that line. Typical causes: a revolve profile that touches or crosses the rotation axis (keep it strictly on one side, x > 0); a sweep / loft with degenerate, coplanar or self-intersecting sections; a fillet, chamfer or offset larger than the local geometry allows; booleans between exactly coincident faces (give cutters 0.01-1 mm of over-travel). Simplify that operation or build the feature from simpler primitives.",
            ),
            _ => None,
        }
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
    /// 贴着床,但没有一个平的底面:靠一条边、一个点或一个曲面着床(球、躺着的圆柱),FDM 打不成
    NoFlatBase { contact_mm2: f64 },
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
                "The script ran longer than {secs} s and was killed. The usual cause is a boolean inside a loop (`for ...: body = body - tool`): every pass re-computes the whole body. Collect the tools in a list and cut / fuse them in ONE operation (`body - [tools]`), which is 10-25x faster. Also remove unbounded loops and do not fillet hundreds of tiny edges."
            ),
            CadProblem::Solids { count: 0 } => "`result` has no solid volume. Check that the booleans do not remove everything.".to_string(),
            CadProblem::Solids { count } => format!(
                "`result` consists of {count} separate solids. Every added feature must overlap (not just touch at an edge) the main body so the union gives ONE solid; position the loose pieces so they intersect the body by at least 0.2 mm."
            ),
            CadProblem::Invalid => "OpenCascade reports `result` as an invalid shape (self-intersection or degenerate faces). Avoid coincident faces in booleans (give cutting tools some over-travel) and reduce fillet radii.".to_string(),
            CadProblem::OffPlate { z } => format!(
                "The part must sit on the build plate: its lowest point is at z = {z:.3}, it has to be z = 0. Use align=(Align.CENTER, Align.CENTER, Align.MIN) for the main body and position the other features relative to it."
            ),
            CadProblem::NoFlatBase { contact_mm2 } => format!(
                "The part has no flat face on the build plate (flat contact area: {contact_mm2:.1} mm2): it rests on a curved surface, an edge or a point, which cannot be printed. Give it a flat bottom at z = 0 — cut the underside flat (e.g. intersect with a box that starts at z = 0), or orient the part so that a planar face lies on the plate."
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
    /// 需要的引擎不在安装目录里(在线升级装的是不带引擎的精简包,而这一版换了引擎):要联网下载。
    /// 几百 MB,界面要先问一句,不自动下
    #[serde(default)]
    pub needs_download: bool,
    #[serde(default)]
    pub download_bytes: u64,
}

/// 模型的一个版本。每次生成、改参数、指令修补都产生一个新版本(父子关系 = 版本树)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadVersion {
    pub id: String,
    /// 属于哪个设计(建模工作室)。0.9.0 在预研页里建的版本没有
    #[serde(default)]
    pub design_id: Option<String>,
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

/// 用户在 3D 视图里点中的位置(毫米,模型坐标系:Z 朝上)。指令修补时帮模型判断「说的是哪个特征」。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CadPick {
    pub point: [f64; 3],
    #[serde(default)]
    pub normal: Option<[f64; 3]>,
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

    fn script_error(stage: &str, ty: &str, message: &str) -> CadScriptError {
        CadScriptError {
            stage: stage.into(),
            error_type: ty.into(),
            message: message.into(),
            line: Some(3),
            traceback: String::new(),
        }
    }

    /// 左边是真引擎(build123d 0.12 / OCP 7.9)上跑出来的原始报错,右边是提示里必须出现的关键词。
    #[test]
    fn common_failures_get_a_targeted_hint_and_unknown_ones_are_left_alone() {
        let cases = [
            ("exec", "ValueError", "Failed creating a fillet with radius of 50, try a smaller value or use max_fillet() to find the largest valid fillet radius", "half of the thinnest"),
            ("exec", "ValueError", "Failed creating a chamfer, try a smaller length value(s)", "chamfer does not fit"),
            ("exec", "IndexError", "list index out of range", "selector"),
            ("exec", "NameError", "name 'RoundedBox' is not defined", "Do not invent API names"),
            ("exec", "TypeError", "Cylinder.__init__() got an unexpected keyword argument 'centered'", "no `center=`"),
            ("exec", "TypeError", "unsupported operand type(s) for *: 'Box' and 'Pos'", "on the LEFT"),
            ("exec", "ValueError", "6 is not a valid Align", "too many positional arguments"),
            ("exec", "ValueError", "No depth provided", "builder-mode"),
            ("exec", "AttributeError", "'Box' object has no attribute 'fillet_edges'", "free functions"),
            ("exec", "ZeroDivisionError", "division by zero", "max(count, 1)"),
            ("exec", "StdFail_NotDone", "BRep_API: command not done", "rotation axis"),
            ("exec", "Standard_ConstructionError", "", "OpenCascade kernel"),
            ("result", "Rejected", "`result` has no solid volume (did a boolean operation remove everything?)", "must be a solid"),
        ];
        for (stage, ty, message, keyword) in cases {
            let e = script_error(stage, ty, message);
            let hint = e.hint().unwrap_or_else(|| panic!("{ty}: {message} 应该有提示"));
            assert!(hint.contains(keyword), "{ty}: {message}\n  提示里应该有「{keyword}」:{hint}");
            // 提示跟在原始错误后面,不替换它
            let text = e.for_model();
            assert!(text.starts_with(&format!("[{stage}] {ty}: {message}")) && text.ends_with(hint), "{text}");
        }
        // 认不出来的、以及执行器自己已经说清楚的,不画蛇添足
        for (stage, ty, message) in [
            ("exec", "ValueError", "math domain error"),
            ("exec", "RuntimeError", "something new"),
            ("validate", "Rejected", "import os is not allowed; use `from build123d import *` and `import math` only"),
            ("validate", "SyntaxError", "invalid syntax"),
            ("result", "Rejected", "the script must assign the final shape to a variable named `result`"),
        ] {
            let e = script_error(stage, ty, message);
            assert_eq!(e.hint(), None, "{ty}: {message}");
            assert!(!e.for_model().contains("Hint:"));
        }
    }

    #[test]
    fn a_part_without_a_flat_base_is_a_problem_but_thin_feet_and_overhangs_are_only_notes() {
        let measured = |contact: f64, overhang: f64| CadMetrics {
            size: [60.0, 40.0, 24.0],
            bed_contact_mm2: Some(contact),
            overhang_mm2: Some(overhang),
            ..Default::default()
        };
        // 球 / 躺着的圆柱:没有平的底面
        assert!(!measured(0.0, 0.0).rests_flat());
        assert_eq!(measured(0.0, 0.0).print_notes(), vec![PrintNote::NoFlatBase]);
        // 实心的盒子:什么都不用提
        assert!(measured(2400.0, 0.0).rests_flat());
        assert!(measured(2400.0, 0.0).print_notes().is_empty());
        // 四条 3 × 3 的细腿 + 桌面底下悬空
        let table = measured(36.0, 2364.0);
        assert!(table.rests_flat());
        assert_eq!(
            table.print_notes(),
            vec![
                PrintNote::SmallContact { contact_mm2: 36.0, footprint_mm2: 2400.0 },
                PrintNote::Overhang { area_mm2: 2364.0 }
            ]
        );
        // 大零件上一块不小的着床面:占比低也不提(绝对面积够粘住了)
        let big = CadMetrics {
            size: [200.0, 200.0, 50.0],
            bed_contact_mm2: Some(900.0),
            overhang_mm2: Some(4.0),
            ..Default::default()
        };
        assert!(big.print_notes().is_empty(), "倒角留下的零头悬空也不提");
        // 旧版本没量这两项:不判、不提
        let old = CadMetrics {
            size: [60.0, 40.0, 24.0],
            ..Default::default()
        };
        assert!(old.rests_flat() && old.print_notes().is_empty());
        // 旧版本存下来的 JSON 里没有这两个字段
        let wire = r#"{"bbox_min":[0,0,0],"bbox_max":[1,1,1],"size":[1,1,1],"volume_mm3":1,"area_mm2":6,"solids":1,"faces":6,"edges":12,"is_valid":true}"#;
        let parsed: CadMetrics = serde_json::from_str(wire).unwrap();
        assert_eq!((parsed.bed_contact_mm2, parsed.overhang_mm2), (None, None));

        let p = CadProblem::NoFlatBase { contact_mm2: 0.0 };
        assert_eq!(serde_json::to_value(&p).unwrap(), serde_json::json!({"kind": "no_flat_base", "contact_mm2": 0.0}));
        assert!(p.for_model().contains("flat bottom at z = 0"));
        assert!(CadProblem::Timeout { secs: 90 }.for_model().contains("body - [tools]"));
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
