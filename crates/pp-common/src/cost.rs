//! 成本定价器(M4):一个单品「卖多少钱、赚多少、一天最多接多少单」。
//!
//! 全是纯函数,前后端共用——前端改任何一个参数都在本地即时重算,不走后端;后端只负责存取。
//! 金额在这里一律用「元」的小数(这是给人填、给人看的数);落库时才换成整数「分」。
//!
//! 和 PRD 最初的公式有一处不同:**失败的打印浪费的不只是耗材,还有机时**,
//! 所以「一次成功率」同时摊到材料成本和机时成本上(宁多勿少)。见 docs/adr/0007-cost-pricing.md。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- 档案

/// 打印机档案。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Printer {
    /// 新建时留空
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub model: String,
    /// 成型空间(毫米)
    pub build_mm: [f64; 3],
    /// 打印时的平均功率(瓦)。不是铭牌上的峰值功率
    pub power_w: f64,
    /// 购入价(元)
    pub price_yuan: f64,
    /// 预期寿命(小时):折旧 = 购入价 ÷ 它
    pub lifetime_hours: f64,
    /// 每小时出料克数——用自己的打样记录校准,决定「多少克要打多久」
    pub grams_per_hour: f64,
    #[serde(default)]
    pub created_at: i64,
}

impl Default for Printer {
    /// 没建档案时用的出厂假设:一台 256 mm 立方的桌面 FDM 机器。
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            model: String::new(),
            build_mm: [256.0, 256.0, 256.0],
            power_w: 100.0,
            price_yuan: 3000.0,
            lifetime_hours: 5000.0,
            grams_per_hour: 28.0,
            created_at: 0,
        }
    }
}

/// 耗材档案。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// PLA / PETG / TPU …
    pub kind: String,
    #[serde(default)]
    pub color: String,
    pub yuan_per_kg: f64,
    /// g/cm³
    pub density: f64,
    /// 库存(克);现在只是记一笔,还不会自动扣减
    #[serde(default)]
    pub stock_g: f64,
    #[serde(default)]
    pub created_at: i64,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: "PLA".into(),
            color: String::new(),
            yuan_per_kg: 70.0,
            density: 1.24,
            stock_g: 0.0,
            created_at: 0,
        }
    }
}

// ---------------------------------------------------------------- 渠道费率

/// 渠道费率预设(docs/04-integrations.md §7,2026-09 核对)。都可以改——平台的费率说变就变。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeePreset {
    pub key: &'static str,
    pub rate: f64,
}

pub const FEE_PRESETS: [FeePreset; 5] = [
    // 小红书:模玩、家居饰品、艺术品周边、文创手作
    FeePreset { key: "xhs_general", rate: 0.05 },
    // 小红书:个性定制 / DIY
    FeePreset { key: "xhs_custom", rate: 0.02 },
    FeePreset { key: "xianyu", rate: 0.006 },
    // 微信小店 1%~5%,按类目;取一个中间值,让用户自己改
    FeePreset { key: "wechat_shop", rate: 0.02 },
    FeePreset { key: "custom", rate: 0.0 },
];

pub fn fee_preset(key: &str) -> Option<FeePreset> {
    FEE_PRESETS.iter().copied().find(|p| p.key == key)
}

// ---------------------------------------------------------------- 成本模型

/// 一个单品的成本参数。新项目从 `CostDefaults` 起步;打印机 / 耗材那几项可以从档案带入,也可以手改。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostParams {
    /// 单件克数(含支撑、裙边)
    pub grams: f64,
    /// 单件打印小时
    pub print_hours: f64,
    /// 损耗率:换料、挤出测试、废边
    pub waste_rate: f64,
    /// 一次成功率
    pub success_rate: f64,

    /// 从哪份档案带入的(只是记着,算钱用的是下面的数)
    pub printer_id: Option<String>,
    pub material_id: Option<String>,
    pub material_yuan_per_kg: f64,
    pub power_w: f64,
    pub electricity_yuan_per_kwh: f64,
    pub printer_price_yuan: f64,
    pub printer_lifetime_hours: f64,

    /// 后处理 + 打包(分钟)
    pub labor_minutes: f64,
    pub labor_yuan_per_min: f64,
    pub packaging_yuan: f64,
    /// 包邮时自己贴的运费
    pub shipping_subsidy_yuan: f64,

    /// 渠道费率预设的键(`FEE_PRESETS`);`custom` = 手填
    pub channel: String,
    pub fee_rate: f64,

    /// 产能:几台机器、一天开几个小时
    pub printers: u32,
    pub hours_per_day: f64,
}

impl Default for CostParams {
    fn default() -> Self {
        CostDefaults::default().new_params()
    }
}

/// 用户自己的成本默认值(设置页里改;存在资料库里)。新项目的成本模型从它起步。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostDefaults {
    pub waste_rate: f64,
    pub success_rate: f64,
    pub electricity_yuan_per_kwh: f64,
    pub labor_minutes: f64,
    pub labor_yuan_per_min: f64,
    pub packaging_yuan: f64,
    pub shipping_subsidy_yuan: f64,
    pub channel: String,
    pub fee_rate: f64,
    pub printers: u32,
    pub hours_per_day: f64,
    /// 三档建议价对应的目标毛利率
    pub target_margins: [f64; 3],
    /// 每机时毛利低于这个数(元 / 小时)就提醒:这个单品不值得占机器
    pub min_profit_per_hour: f64,
}

impl Default for CostDefaults {
    fn default() -> Self {
        Self {
            waste_rate: 0.05,
            success_rate: 0.92,
            electricity_yuan_per_kwh: 0.6,
            labor_minutes: 8.0,
            labor_yuan_per_min: 0.5,
            packaging_yuan: 1.2,
            shipping_subsidy_yuan: 4.5,
            channel: "xhs_general".into(),
            fee_rate: 0.05,
            printers: 1,
            hours_per_day: 16.0,
            target_margins: [0.5, 0.6, 0.65],
            min_profit_per_hour: 5.0,
        }
    }
}

impl CostDefaults {
    /// 一份新的成本参数:默认值 + 出厂假设的机器和耗材;克数和时长留空,等人填或从模型估。
    pub fn new_params(&self) -> CostParams {
        let (printer, material) = (Printer::default(), Material::default());
        CostParams {
            grams: 0.0,
            print_hours: 0.0,
            waste_rate: self.waste_rate,
            success_rate: self.success_rate,
            printer_id: None,
            material_id: None,
            material_yuan_per_kg: material.yuan_per_kg,
            power_w: printer.power_w,
            electricity_yuan_per_kwh: self.electricity_yuan_per_kwh,
            printer_price_yuan: printer.price_yuan,
            printer_lifetime_hours: printer.lifetime_hours,
            labor_minutes: self.labor_minutes,
            labor_yuan_per_min: self.labor_yuan_per_min,
            packaging_yuan: self.packaging_yuan,
            shipping_subsidy_yuan: self.shipping_subsidy_yuan,
            channel: self.channel.clone(),
            fee_rate: self.fee_rate,
            printers: self.printers,
            hours_per_day: self.hours_per_day,
        }
    }
}

impl CostParams {
    pub fn apply_printer(&mut self, p: &Printer) {
        self.printer_id = (!p.id.is_empty()).then(|| p.id.clone());
        self.power_w = p.power_w;
        self.printer_price_yuan = p.price_yuan;
        self.printer_lifetime_hours = p.lifetime_hours;
    }

    pub fn apply_material(&mut self, m: &Material) {
        self.material_id = (!m.id.is_empty()).then(|| m.id.clone());
        self.material_yuan_per_kg = m.yuan_per_kg;
    }

    /// 克数和时长都填了才算得出有意义的价。
    pub fn is_ready(&self) -> bool {
        self.grams > 0.0 && self.print_hours > 0.0
    }
}

/// 成本拆解(元)。每一行都对应界面上的一行,方便对账。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub material: f64,
    /// 其中电费
    pub electricity: f64,
    /// 其中机器折旧
    pub depreciation: f64,
    /// 机时 = 电费 + 折旧
    pub machine: f64,
    pub labor: f64,
    pub packaging: f64,
    pub shipping: f64,
    pub unit_cost: f64,
    /// 每卖出一件实际占用的机时(含失败重打的摊销)
    pub machine_hours: f64,
}

fn clean(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

/// 成功率夹在 (0, 1]:0 或乱填都按「几乎打不成」算,而不是除以 0。
fn success(rate: f64) -> f64 {
    if rate.is_finite() {
        rate.clamp(0.05, 1.0)
    } else {
        1.0
    }
}

pub fn breakdown(p: &CostParams) -> CostBreakdown {
    let ok = success(p.success_rate);
    let material = clean(p.grams) * (1.0 + clean(p.waste_rate)) * clean(p.material_yuan_per_kg) / 1000.0 / ok;
    // 失败的那几次同样占着机器、耗着电:机时也按成功率摊
    let machine_hours = clean(p.print_hours) / ok;
    let electricity = machine_hours * clean(p.power_w) / 1000.0 * clean(p.electricity_yuan_per_kwh);
    let depreciation = if p.printer_lifetime_hours > 0.0 {
        machine_hours * clean(p.printer_price_yuan) / p.printer_lifetime_hours
    } else {
        0.0
    };
    let labor = clean(p.labor_minutes) * clean(p.labor_yuan_per_min);
    let (packaging, shipping) = (clean(p.packaging_yuan), clean(p.shipping_subsidy_yuan));
    let machine = electricity + depreciation;
    CostBreakdown {
        material,
        electricity,
        depreciation,
        machine,
        labor,
        packaging,
        shipping,
        unit_cost: material + machine + labor + packaging + shipping,
        machine_hours,
    }
}

// ---------------------------------------------------------------- 定价

/// 某个售价下的账。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PriceCheck {
    pub price: f64,
    /// 平台抽走的
    pub fee: f64,
    /// 每件毛利(元)= 售价 − 平台费 − 单位成本
    pub profit: f64,
    /// 毛利率 = 毛利 ÷ 售价
    pub margin: f64,
    /// 每机时毛利(元 / 小时):机器是瓶颈,这才是「值不值得打」的数
    pub profit_per_hour: f64,
}

pub fn check_price(price: f64, fee_rate: f64, cost: &CostBreakdown) -> PriceCheck {
    let price = clean(price);
    let fee = price * clean(fee_rate).min(1.0);
    let profit = price - fee - cost.unit_cost;
    PriceCheck {
        price,
        fee,
        profit,
        margin: if price > 0.0 { profit / price } else { 0.0 },
        profit_per_hour: if cost.machine_hours > 0.0 { profit / cost.machine_hours } else { 0.0 },
    }
}

/// 往上取到最近的「尾数价」:100 元以下落在 x5.9 / x9.9,100 元及以上落在整数的 x9。
/// 只往上取——往下取会吃掉目标毛利。
pub fn charm_price(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 0.0;
    }
    if raw > 99.9 {
        // 109、119、129…
        let tens = ((raw - 9.0) / 10.0).ceil().max(10.0);
        return tens * 10.0 + 9.0;
    }
    // 5.9、9.9、15.9、19.9…:每 10 元里有两个落点
    let base = (raw / 10.0).floor() * 10.0;
    [base + 5.9, base + 9.9, base + 15.9]
        .into_iter()
        .find(|c| *c + 1e-9 >= raw)
        .unwrap_or(base + 15.9)
}

/// 按目标毛利率反推售价:`成本 ÷ (1 − 费率 − 目标毛利率)`,再取尾数价。
/// 费率 + 目标毛利 ≥ 100% 时无解,返回 `None`。
pub fn price_for_margin(cost: &CostBreakdown, fee_rate: f64, target_margin: f64) -> Option<f64> {
    let keep = 1.0 - clean(fee_rate) - clean(target_margin);
    if keep <= 0.0 || cost.unit_cost <= 0.0 {
        return None;
    }
    Some(charm_price(cost.unit_cost / keep))
}

/// 三档建议价(已经算好各自的账)。算不出来的档不出现;取完尾数撞在一起的只留一个。
pub fn price_tiers(cost: &CostBreakdown, fee_rate: f64, target_margins: &[f64]) -> Vec<PriceCheck> {
    let mut out: Vec<PriceCheck> = Vec::new();
    for m in target_margins {
        if let Some(price) = price_for_margin(cost, fee_rate, *m) {
            if !out.iter().any(|t| (t.price - price).abs() < 1e-9) {
                out.push(check_price(price, fee_rate, cost));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- 产能

/// 产能测算。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Capacity {
    /// 一天能交付几件好的(所有机器合计,已扣掉失败的)
    pub units_per_day: f64,
    /// 48 小时内最多能发几件——小红书的「现货」要求 48 小时内发货;接单超过这个数就该挂预售
    pub units_in_48h: u32,
}

pub fn capacity(p: &CostParams) -> Capacity {
    if p.print_hours <= 0.0 {
        return Capacity::default();
    }
    let machine_hours_per_good_unit = p.print_hours / success(p.success_rate);
    let units_per_day = p.printers.max(1) as f64 * clean(p.hours_per_day).min(24.0) / machine_hours_per_good_unit;
    Capacity {
        units_per_day,
        units_in_48h: (units_per_day * 2.0).floor() as u32,
    }
}

// ---------------------------------------------------------------- 从模型估克数和时长

/// 体积法估算:`克数 ≈ (壳体积 + 内部体积 × 填充率) × 密度`,壳体积 ≈ 表面积 × 壁厚(不超过总体积)。
/// 和预研页网格工具用的是同一条公式(`MeshReport::estimate`);切片软件的数更准,打过一次之后请用实际值。
pub fn estimate_print(volume_mm3: f64, area_mm2: f64, density: f64, grams_per_hour: f64) -> (f64, f64) {
    const WALL_MM: f64 = 0.84;
    const INFILL: f64 = 0.15;
    let total = clean(volume_mm3);
    let shell = (clean(area_mm2) * WALL_MM).min(total);
    let grams = (shell + (total - shell) * INFILL) / 1000.0 * clean(density);
    let hours = if grams_per_hour > 0.0 { grams / grams_per_hour } else { 0.0 };
    (grams, hours)
}

/// 项目名下已经建出来的模型的几何量(多个零件就合计)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelGeometry {
    pub volume_mm3: f64,
    pub area_mm2: f64,
    /// 合计了几个模型
    pub designs: u32,
}

// ---------------------------------------------------------------- 打样记录

/// 一次打样:预估 vs. 实际。失败也要记——失败率是成本的一部分。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrintRun {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub printer_id: Option<String>,
    #[serde(default)]
    pub material_id: Option<String>,
    /// 记这一笔时成本模型里的预估
    #[serde(default)]
    pub est_hours: Option<f64>,
    #[serde(default)]
    pub est_grams: Option<f64>,
    #[serde(default)]
    pub actual_hours: Option<f64>,
    #[serde(default)]
    pub actual_grams: Option<f64>,
    pub success: bool,
    #[serde(default)]
    pub fail_reason: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created_at: i64,
}

/// 打样记录里看得出来的一次成功率。记录太少(< 3 次)时不给数——两次里失败一次不等于成功率 50%。
pub fn observed_success_rate(runs: &[PrintRun]) -> Option<f64> {
    if runs.len() < 3 {
        return None;
    }
    Some(runs.iter().filter(|r| r.success).count() as f64 / runs.len() as f64)
}

/// 最近一次成功、并且填了实际值的打样(`runs` 最新的在前)。
pub fn latest_actuals(runs: &[PrintRun]) -> Option<(f64, f64)> {
    runs.iter().filter(|r| r.success).find_map(|r| match (r.actual_grams, r.actual_hours) {
        (Some(g), Some(h)) if g > 0.0 && h > 0.0 => Some((g, h)),
        _ => None,
    })
}

// ---------------------------------------------------------------- 往返后端的整包

/// 一个项目的成本模型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostModel {
    pub params: CostParams,
    /// 定下来的售价(元);没定 = None
    #[serde(default)]
    pub chosen_price: Option<f64>,
    /// 库里已经有这个项目的成本模型(false = 这是一份刚按默认值拼出来的草稿)
    #[serde(default)]
    pub saved: bool,
    #[serde(default)]
    pub updated_at: i64,
}

/// 定价页打开一个项目时要的全部东西。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingDetail {
    pub model: CostModel,
    /// 最新的在前
    #[serde(default)]
    pub runs: Vec<PrintRun>,
    /// 调研给的竞品价格带(元):这个项目采用自哪张机会卡就用哪张的;否则取名下调研里所有机会的范围
    #[serde(default)]
    pub price_band: Option<(u32, u32)>,
    /// 名下已经建出来的模型(用来估克数和时长)
    #[serde(default)]
    pub geometry: Option<ModelGeometry>,
}

/// 项目中枢 / 定价页列表里的一行摘要。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PricingSummary {
    /// 单位成本(元);还没算过 = 0
    pub unit_cost: f64,
    pub chosen_price: Option<f64>,
    pub runs: u32,
    pub successes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.005
    }

    /// 线框里的那个例子:27 g、1.7 小时的线缆夹。
    fn clip() -> CostParams {
        CostParams {
            grams: 27.0,
            print_hours: 1.7,
            ..Default::default()
        }
    }

    #[test]
    fn the_breakdown_adds_up_line_by_line() {
        let c = breakdown(&clip());
        // 27 g × 1.05 × ¥0.07/g ÷ 92%
        assert!(near(c.material, 2.157), "{c:?}");
        // 失败的那几次也占机时:1.7 h ÷ 92% = 1.848 h
        assert!(near(c.machine_hours, 1.848), "{c:?}");
        // 电:1.848 h × 0.1 kW × ¥0.6;折旧:1.848 h × ¥3000 ÷ 5000 h
        assert!(near(c.electricity, 0.111) && near(c.depreciation, 1.109), "{c:?}");
        assert!(near(c.machine, c.electricity + c.depreciation));
        assert!(near(c.labor, 4.0) && near(c.packaging, 1.2) && near(c.shipping, 4.5));
        assert!(near(c.unit_cost, 2.157 + 1.220 + 4.0 + 1.2 + 4.5), "{c:?}");
    }

    #[test]
    fn a_lower_success_rate_costs_both_material_and_machine_time() {
        let mut p = clip();
        let good = breakdown(&p);
        p.success_rate = 0.5;
        let bad = breakdown(&p);
        assert!(near(bad.material, good.material * 0.92 / 0.5));
        assert!(near(bad.machine, good.machine * 0.92 / 0.5), "机时也要摊");
        assert!(near(bad.labor, good.labor), "人工只花在打成的那件上");
        // 乱填不会除以 0
        p.success_rate = 0.0;
        assert!(breakdown(&p).unit_cost.is_finite());
        p.success_rate = f64::NAN;
        assert!(breakdown(&p).unit_cost.is_finite());
    }

    #[test]
    fn charm_prices_only_round_up_to_the_next_5_9_or_9_9() {
        for (raw, want) in [(0.5, 5.9), (5.9, 5.9), (6.0, 9.9), (9.91, 15.9), (29.07, 29.9), (29.9, 29.9), (30.0, 35.9), (37.4, 39.9), (96.0, 99.9)] {
            assert!(near(charm_price(raw), want), "{raw} → {} (想要 {want})", charm_price(raw));
        }
        // 100 元以上落在整数的 x9
        for (raw, want) in [(100.0, 109.0), (109.0, 109.0), (109.5, 119.0), (250.0, 259.0)] {
            assert!(near(charm_price(raw), want), "{raw} → {}", charm_price(raw));
        }
        assert_eq!(charm_price(0.0), 0.0);
        assert_eq!(charm_price(f64::NAN), 0.0);
    }

    #[test]
    fn tiers_hit_at_least_their_target_margin_after_rounding() {
        let cost = breakdown(&clip());
        let tiers = price_tiers(&cost, 0.05, &[0.5, 0.6, 0.65]);
        assert_eq!(tiers.iter().map(|t| t.price).collect::<Vec<_>>(), [29.9, 39.9, 45.9]);
        for (tier, target) in tiers.iter().zip([0.5, 0.6, 0.65]) {
            assert!(tier.margin >= target, "取尾数只往上取,毛利不会低于目标:{tier:?}");
            assert!(near(tier.profit, tier.price * 0.95 - cost.unit_cost));
            assert!(near(tier.profit_per_hour, tier.profit / cost.machine_hours));
        }
        // 费率 + 目标毛利 ≥ 100%:无解,这一档不出现
        assert!(price_tiers(&cost, 0.5, &[0.5]).is_empty());
        // 两档取完尾数撞在一起:只留一个
        assert_eq!(price_tiers(&cost, 0.05, &[0.5, 0.505]).len(), 1);
        // 还没填克数和时长:成本只有人工和杂费,照样能出价(这是「地板价」)
        assert!(!clip().is_ready() || breakdown(&CostParams::default()).unit_cost > 0.0);
    }

    #[test]
    fn a_price_below_cost_shows_a_loss_not_a_crash() {
        let cost = breakdown(&clip());
        let check = check_price(9.9, 0.05, &cost);
        assert!(check.profit < 0.0 && check.margin < 0.0 && check.profit_per_hour < 0.0);
        assert_eq!(check_price(0.0, 0.05, &cost).margin, 0.0);
    }

    #[test]
    fn capacity_counts_good_units_across_all_printers() {
        let mut p = clip();
        // 1 台 × 16 小时 ÷ (1.7 ÷ 0.92) = 8.66 件 / 天
        assert!(near(capacity(&p).units_per_day, 16.0 / (1.7 / 0.92)));
        assert_eq!(capacity(&p).units_in_48h, 17);
        p.printers = 3;
        assert!(near(capacity(&p).units_per_day, 3.0 * 16.0 / (1.7 / 0.92)));
        p.hours_per_day = 99.0;
        assert!(near(capacity(&p).units_per_day, 3.0 * 24.0 / (1.7 / 0.92)), "一天最多 24 小时");
        p.print_hours = 0.0;
        assert_eq!(capacity(&p), Capacity::default());
    }

    #[test]
    fn the_volume_estimate_matches_the_mesh_tool() {
        // 20 mm 的实心立方体:8 cm³,表面积 2400 mm²
        let (grams, hours) = estimate_print(8000.0, 2400.0, 1.24, 28.0);
        let shell = 2400.0 * 0.84;
        let want = (shell + (8000.0 - shell) * 0.15) / 1000.0 * 1.24;
        assert!(near(grams, want) && near(hours, want / 28.0));
        assert_eq!(estimate_print(0.0, 0.0, 1.24, 28.0), (0.0, 0.0));
        assert_eq!(estimate_print(8000.0, 2400.0, 1.24, 0.0).1, 0.0);
    }

    fn run(success: bool, actual: Option<(f64, f64)>) -> PrintRun {
        PrintRun {
            id: String::new(),
            project_id: "p".into(),
            printer_id: None,
            material_id: None,
            est_hours: None,
            est_grams: None,
            actual_hours: actual.map(|a| a.1),
            actual_grams: actual.map(|a| a.0),
            success,
            fail_reason: String::new(),
            note: String::new(),
            created_at: 0,
        }
    }

    #[test]
    fn print_runs_feed_back_into_the_model() {
        let runs = vec![run(false, None), run(true, None), run(true, Some((31.0, 2.1))), run(true, Some((27.0, 1.7)))];
        assert_eq!(latest_actuals(&runs), Some((31.0, 2.1)), "最近一次成功且填了实际值的那次");
        assert_eq!(observed_success_rate(&runs), Some(0.75));
        assert_eq!(observed_success_rate(&runs[..2]), None, "两次记录不足以说成功率");
        assert_eq!(latest_actuals(&[run(false, Some((30.0, 2.0)))]), None, "失败的那次不算");
    }

    #[test]
    fn profiles_fill_the_model_and_defaults_are_the_starting_point() {
        let mut p = CostDefaults::default().new_params();
        assert!(!p.is_ready());
        assert_eq!((p.channel.as_str(), p.fee_rate), ("xhs_general", 0.05));
        p.apply_printer(&Printer {
            id: "pr-1".into(),
            power_w: 150.0,
            price_yuan: 4999.0,
            lifetime_hours: 8000.0,
            ..Default::default()
        });
        p.apply_material(&Material {
            id: "m-1".into(),
            yuan_per_kg: 95.0,
            ..Default::default()
        });
        assert_eq!((p.printer_id.as_deref(), p.power_w, p.printer_price_yuan), (Some("pr-1"), 150.0, 4999.0));
        assert_eq!((p.material_id.as_deref(), p.material_yuan_per_kg), (Some("m-1"), 95.0));
        assert_eq!(fee_preset("xhs_custom").unwrap().rate, 0.02);
        assert!(fee_preset("nope").is_none());
        // 旧数据 / 少字段的 JSON 也能读(缺的用默认值)
        let old: CostParams = serde_json::from_str(r#"{"grams": 27, "print_hours": 1.7}"#).unwrap();
        assert_eq!(old.fee_rate, 0.05);
    }
}
