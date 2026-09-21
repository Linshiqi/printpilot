//! 上架与发布(M5,docs/adr/0009-publish-pack.md):渠道规格、草稿、合规检查。
//!
//! 全是纯函数和 DTO,前后端共用:界面上边打字边出检查结果;生成发布包时后端用同一套再判一次——
//! 后端那一次才算数(「检查没过不能出包,除非写下原因」不能只靠界面自觉)。
//!
//! 渠道规格和词表是**数据**:平台的规则说变就变。数字的来源和核实日期见 docs/04-integrations.md §4.3 / §7.2。

use serde::{Deserialize, Serialize};

/// 销售 / 内容渠道。现在只有小红书;微信小店、闲鱼按 ADR-0002 的顺序以后加。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Xhs,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Xhs => "xhs",
        }
    }

    pub fn parse(s: &str) -> Channel {
        match s {
            _ => Channel::Xhs,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftKind {
    /// 笔记(内容):标题 + 正文 + 话题 + 图
    Note,
    /// 商品(店铺后台里的那条商品):标题 + 卖点 + 详情 + 规格与价格 + 主图
    Listing,
}

impl DraftKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DraftKind::Note => "note",
            DraftKind::Listing => "listing",
        }
    }

    pub fn parse(s: &str) -> DraftKind {
        if s == "listing" {
            DraftKind::Listing
        } else {
            DraftKind::Note
        }
    }
}

/// 一个渠道对一种内容的硬性规格。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContentSpec {
    pub title_max: usize,
    pub body_max: usize,
    /// 话题个数上限(商品没有话题 = 0)
    pub tags_max: usize,
    pub images_min: usize,
    pub images_max: usize,
    /// 发布包里的图统一处理成这个尺寸(宽, 高)
    pub image_px: (u32, u32),
    pub image_max_bytes: usize,
    /// 第一张图必须是实拍(不能是 AI 生成的)
    pub first_image_must_be_photo: bool,
    /// 官方的网页版发布入口:「在电脑上发布」就是在默认浏览器里打开它,剩下的由人来点
    pub publish_url: &'static str,
}

/// 小红书的数字:笔记标题 ≤ 20 字、正文 ≤ 1000 字、图 ≤ 18 张、推荐 3:4(1080 × 1440);话题上限有 5 和 10 两种说法,按 5。
/// 商品主图 3~9 张、1:1 或 3:4、单张 ≤ 3 MB、**首图必须是商品正面实物图**。商品标题的上限没有找到官方数字,按 60 字从严。
pub fn spec(channel: Channel, kind: DraftKind) -> ContentSpec {
    match (channel, kind) {
        (Channel::Xhs, DraftKind::Note) => ContentSpec {
            title_max: 20,
            body_max: 1000,
            tags_max: 5,
            images_min: 1,
            images_max: 18,
            image_px: (1080, 1440),
            image_max_bytes: 3 * 1024 * 1024,
            first_image_must_be_photo: false,
            publish_url: "https://creator.xiaohongshu.com/publish/publish",
        },
        (Channel::Xhs, DraftKind::Listing) => ContentSpec {
            title_max: 60,
            body_max: 1000,
            tags_max: 0,
            images_min: 3,
            images_max: 9,
            image_px: (1080, 1440),
            image_max_bytes: 3 * 1024 * 1024,
            first_image_must_be_photo: true,
            publish_url: "https://ark.xiaohongshu.com/",
        },
    }
}

/// 笔记从哪个角度写。同一个单品用不同角度各写一篇,是最便宜的测款方式(docs/05-growth-cro.md)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteAngle {
    /// 痛点:先说那个烦人的问题
    #[default]
    Pain,
    /// 场景:它在桌面 / 家里的样子
    Scene,
    /// 过程:打印、打磨、组装的过程
    Process,
    /// 设计幕后:为什么这样设计、改了几版
    Backstage,
    /// 定制故事:给谁做的、刻了什么
    Custom,
}

impl NoteAngle {
    pub const ALL: [NoteAngle; 5] = [NoteAngle::Pain, NoteAngle::Scene, NoteAngle::Process, NoteAngle::Backstage, NoteAngle::Custom];

    pub fn as_str(self) -> &'static str {
        match self {
            NoteAngle::Pain => "pain",
            NoteAngle::Scene => "scene",
            NoteAngle::Process => "process",
            NoteAngle::Backstage => "backstage",
            NoteAngle::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> NoteAngle {
        NoteAngle::ALL.into_iter().find(|a| a.as_str() == s).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    #[default]
    Draft,
    /// 出过发布包了,还没回填链接
    Packed,
    Published,
}

impl DraftStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DraftStatus::Draft => "draft",
            DraftStatus::Packed => "packed",
            DraftStatus::Published => "published",
        }
    }

    pub fn parse(s: &str) -> DraftStatus {
        match s {
            "packed" => DraftStatus::Packed,
            "published" => DraftStatus::Published,
            _ => DraftStatus::Draft,
        }
    }
}

/// 笔记草稿。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoteDraft {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub channel: Channel,
    #[serde(default)]
    pub angle: NoteAngle,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// 话题,不带 `#`
    #[serde(default)]
    pub tags: Vec<String>,
    /// 配图(资产 ID),顺序就是发布时的顺序,第一张是封面
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub status: DraftStatus,
    #[serde(default)]
    pub external_url: Option<String>,
    /// 这一篇是 AI 起草的(花了多少钱记在账本里;这里只是个标记)
    #[serde(default)]
    pub ai_drafted: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

/// 模型(设计)的来源决定了能不能拿来卖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLicense {
    /// 自己设计的(包括在建模工作室里和 AI 一起做的)
    Original,
    /// 买了商用授权的第三方模型
    Licensed,
    /// 只许个人使用 / 非商用(CC BY-NC 之类)——**不能卖**
    NonCommercial,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ShipMode {
    /// 现货:小红书要求 48 小时内发货
    InStock,
    /// 预售:付款后 N 天内发货
    Presale { days: u32 },
}

impl Default for ShipMode {
    fn default() -> Self {
        ShipMode::InStock
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sku {
    /// 规格名:颜色 / 尺寸 / 刻字与否
    pub name: String,
    pub price_yuan: f64,
}

/// 商品草稿(一个项目在一个渠道上一条)。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListingDraft {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub channel: Channel,
    #[serde(default)]
    pub title: String,
    /// 卖点,每条一句
    #[serde(default)]
    pub selling_points: Vec<String>,
    /// 详情文字
    #[serde(default)]
    pub body: String,
    /// 售价(元)。默认取成本定价器里定的价
    #[serde(default)]
    pub price_yuan: Option<f64>,
    #[serde(default)]
    pub skus: Vec<Sku>,
    #[serde(default)]
    pub ship: ShipMode,
    #[serde(default)]
    pub model_license: Option<ModelLicense>,
    /// 主图(资产 ID),第一张必须是实拍
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub status: DraftStatus,
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

/// 可以拿来配图的一张图。检查只关心它是不是 AI 生成的。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishImage {
    pub asset_id: String,
    /// AI 生成(图片工作台出的图)。实拍 = false
    #[serde(default)]
    pub ai_generated: bool,
    /// 从哪来的:photo(导入的实拍)/ board(图片工作台里采用的)/ render(模型渲染图)
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

/// 发布包:处理好的图 + 文案,落在资料库的一个文件夹里。出过的包都留着(再出一次不覆盖旧的)。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishPack {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub draft_id: String,
    pub channel: Channel,
    /// 资料库里的相对路径
    pub dir: String,
    /// 包里的图片文件名(按顺序)
    pub images: Vec<String>,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    /// 出包时没过的检查(被强制跳过的)
    #[serde(default)]
    pub issues: Vec<LintIssue>,
    #[serde(default)]
    pub override_reason: String,
    #[serde(default)]
    pub external_url: Option<String>,
    pub created_at: i64,
}

/// 上架页打开一个项目时要的全部东西。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishOverview {
    pub notes: Vec<NoteDraft>,
    pub listing: ListingDraft,
    /// 最新的在前
    pub packs: Vec<PublishPack>,
    /// 这个项目里可以拿来配图的图:实拍在前,其次是图片工作台里的图(采用的在前)
    pub images: Vec<PublishImage>,
    /// 成本定价器里定的售价、算出来的单位成本(元)
    #[serde(default)]
    pub chosen_price: Option<f64>,
    #[serde(default)]
    pub unit_cost: Option<f64>,
    #[serde(default)]
    pub brand_voice: String,
}

/// AI 起草的结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DraftedNotes {
    pub notes: Vec<NoteDraft>,
    #[serde(default)]
    pub cost_fen: f64,
    #[serde(default)]
    pub elapsed_ms: u64,
}

/// 手机页的地址和二维码。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShareInfo {
    pub url: String,
    /// 二维码(SVG 文本)
    pub qr_svg: String,
    /// false = 只有本机能打开(没找到局域网地址)
    pub lan: bool,
}

/// 回填链接之后:更新后的包,以及项目有没有因此自动进入「运营」。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishMarked {
    pub pack: PublishPack,
    #[serde(default)]
    pub advanced: bool,
}

/// 图片怎么放进渠道要求的画幅里。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// 居中裁切、填满画幅(生活场景图)
    #[default]
    Cover,
    /// 整张放进去、四周留白(白底产品图)
    Contain,
}

impl PublishPack {
    /// 话题拼成发布时要贴的那一行:`#桌搭 #3D打印`
    pub fn tags_line(&self) -> String {
        tags_line(&self.tags)
    }
}

pub fn tags_line(tags: &[String]) -> String {
    tags.iter().map(|t| format!("#{}", t.trim().trim_start_matches('#'))).filter(|t| t.len() > 1).collect::<Vec<_>>().join(" ")
}

/// 把用户敲的一行话题拆开:空格、逗号和分号(半角、全角 U+FF0C / U+FF1B)、顿号、`#` 都算分隔。去重,保持顺序。
pub fn parse_tags(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in input.split(|c: char| c.is_whitespace() || matches!(c, ',' | '\u{ff0c}' | '、' | '#' | ';' | '\u{ff1b}')) {
        let tag = raw.trim();
        if !tag.is_empty() && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

/// 平台按「字」计数:一个汉字、一个字母、一个表情各算一个。
pub fn char_len(s: &str) -> usize {
    s.trim().chars().count()
}

// ---------------------------------------------------------------- 合规检查

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// 不改掉就不能出发布包(可以写下原因强制跳过)
    Block,
    /// 要留意,不拦
    Warn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintField {
    Title,
    Body,
    Tags,
    Images,
    Price,
    License,
}

/// 一条检查结果。带类型而不是一句话:界面按 `code` 本地化,`hit` 是命中的那个词 / 那个数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintIssue {
    pub code: LintCode,
    pub severity: Severity,
    pub field: LintField,
    #[serde(default)]
    pub hit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintCode {
    TitleEmpty,
    TitleTooLong,
    BodyEmpty,
    BodyTooLong,
    TooManyTags,
    TooFewImages,
    TooManyImages,
    /// 商品首图必须是实拍
    FirstImageNotPhoto,
    /// 用了 AI 生成的图:发布时要勾选平台的「AI 内容声明」
    AiImages,
    /// 极限词(最好、第一、顶级…):广告法 + 平台都管
    Extreme,
    /// 功效 / 医疗宣称
    Medical,
    /// IP / 品牌名词(角色、影视游戏、潮玩):没有授权不能用
    IpName,
    /// 站外导流(微信号、链接、手机号)
    Diversion,
    /// 好评返现、刷单
    FakeReview,
    /// 品类预警:儿童用品
    CategoryKids,
    /// 品类预警:带电 / 灯具
    CategoryElectric,
    /// 品类预警:接触食品
    CategoryFood,
    /// 品类预警:刀具 / 武器造型
    CategoryWeapon,
    PriceMissing,
    /// 售价低于单位成本
    PriceBelowCost,
    LicenseMissing,
    /// 非商用授权的模型不能卖
    LicenseNonCommercial,
}

pub fn has_blockers(issues: &[LintIssue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Block)
}

// 词表用「词组」而不是单字:「最」会误伤「最近」「最后」,「第一」会误伤「第一步」。
const EXTREME: &[&str] = &[
    "最好", "最佳", "最强", "最优", "最低价", "最便宜", "最高级", "最先进", "最牛", "最美", "最火", "最全", "第一", "NO.1", "No.1", "no.1", "TOP1", "Top1", "top1",
    "顶级", "顶尖", "极致", "全网", "全国", "国家级", "世界级", "史无前例", "绝无仅有", "独一无二", "万能", "永久", "100%", "百分百", "绝对", "完美", "首选", "唯一", "销量冠军",
    "遥遥领先", "无敌", "神器",
];
/// 「第一」后面跟着这些字时是序数,不是极限词。
const FIRST_IS_ORDINAL: &[&str] = &["第一步", "第一次", "第一眼", "第一时间", "第一层", "第一张", "第一个", "第一版", "第一天", "第一件", "第一根", "第一遍"];
const MEDICAL: &[&str] = &["治疗", "治愈", "疗效", "药用", "抗菌", "杀菌", "抑菌", "消炎", "防辐射", "减肥", "瘦身", "保健", "缓解疼痛", "预防疾病", "降血压", "助眠", "护眼"];
const IP_NAMES: &[&str] = &[
    "迪士尼", "Disney", "漫威", "Marvel", "皮卡丘", "宝可梦", "Pokemon", "Pokémon", "Hello Kitty", "HelloKitty", "凯蒂猫", "哆啦A梦", "机器猫", "原神", "王者荣耀", "任天堂",
    "Nintendo", "马里奥", "塞尔达", "乐高", "LEGO", "海贼王", "火影", "高达", "泡泡玛特", "POP MART", "Labubu", "LABUBU", "拉布布", "三丽鸥", "库洛米", "美乐蒂", "玉桂狗", "玲娜贝儿",
    "星黛露", "奥特曼", "小猪佩奇", "蜡笔小新", "哈利波特", "星球大战", "钢铁侠", "蜘蛛侠", "蝙蝠侠", "米老鼠", "米奇", "史努比", "吉伊卡哇", "chiikawa", "Chiikawa", "线条小狗",
    "龙猫", "吉卜力", "初音", "EVA", "黑神话", "悟空黑神话", "英雄联盟", "我的世界", "Minecraft", "蛋仔派对", "光遇", "崩坏", "咒术回战", "鬼灭", "间谍过家家", "loopy", "Loopy",
];
const DIVERSION: &[&str] = &[
    "微信", "VX", "vx", "Vx", "V信", "v信", "薇信", "威信", "加我", "加V", "加v", "QQ", "qq", "淘宝", "拼多多", "闲鱼", "抖音", "咸鱼", "二维码", "扫码", "私下交易", "站外", "http://", "https://", "www.",
];
const FAKE_REVIEW: &[&str] = &["好评返现", "返现", "刷单", "晒图返", "五星好评", "好评有礼", "好评送"];
const KIDS: &[&str] = &["儿童", "宝宝", "婴儿", "幼儿", "小孩玩", "给孩子玩", "玩具"];
const ELECTRIC: &[&str] = &["夜灯", "台灯", "灯具", "插电", "充电", "USB 供电", "USB供电", "电池", "LED"];
const FOOD: &[&str] = &["餐具", "杯子", "水杯", "吸管", "食品", "入口", "饼干模", "模具", "奶瓶"];
const WEAPON: &[&str] = &["匕首", "刀具", "军刀", "弹弓", "飞镖", "仿真枪", "枪"];

fn hits<'a>(text: &str, words: &[&'a str]) -> Vec<&'a str> {
    let mut found: Vec<&str> = Vec::new();
    for w in words {
        if found.contains(w) {
            continue;
        }
        let ok = if *w == "第一" {
            // 每一处「第一」都要看后面跟的是什么
            text.match_indices("第一").any(|(at, _)| !FIRST_IS_ORDINAL.iter().any(|o| text[at..].starts_with(o)))
        } else {
            text.contains(w)
        };
        if ok {
            found.push(w);
        }
    }
    found
}

/// 连着 11 位、1 开头第二位 3~9:手机号。
fn has_phone_number(text: &str) -> bool {
    let digits: Vec<char> = text.chars().collect();
    let mut run = 0usize;
    let mut start = 0usize;
    for (i, c) in digits.iter().enumerate() {
        if c.is_ascii_digit() {
            if run == 0 {
                start = i;
            }
            run += 1;
        } else {
            if run == 11 && digits[start] == '1' && ('3'..='9').contains(&digits[start + 1]) {
                return true;
            }
            run = 0;
        }
    }
    run == 11 && digits[start] == '1' && ('3'..='9').contains(&digits[start + 1])
}

fn scan_text(text: &str, field: LintField, out: &mut Vec<LintIssue>) {
    let mut push = |code, severity, words: Vec<&str>| {
        for w in words {
            out.push(LintIssue {
                code,
                severity,
                field,
                hit: w.to_string(),
            });
        }
    };
    push(LintCode::Extreme, Severity::Block, hits(text, EXTREME));
    push(LintCode::Medical, Severity::Block, hits(text, MEDICAL));
    push(LintCode::IpName, Severity::Block, hits(text, IP_NAMES));
    push(LintCode::Diversion, Severity::Block, hits(text, DIVERSION));
    push(LintCode::FakeReview, Severity::Block, hits(text, FAKE_REVIEW));
    push(LintCode::CategoryKids, Severity::Warn, hits(text, KIDS).into_iter().take(1).collect());
    push(LintCode::CategoryElectric, Severity::Warn, hits(text, ELECTRIC).into_iter().take(1).collect());
    push(LintCode::CategoryFood, Severity::Warn, hits(text, FOOD).into_iter().take(1).collect());
    push(LintCode::CategoryWeapon, Severity::Warn, hits(text, WEAPON).into_iter().take(1).collect());
    if has_phone_number(text) {
        out.push(LintIssue {
            code: LintCode::Diversion,
            severity: Severity::Block,
            field,
            hit: "手机号".into(),
        });
    }
}

fn issue(code: LintCode, severity: Severity, field: LintField, hit: impl ToString) -> LintIssue {
    LintIssue {
        code,
        severity,
        field,
        hit: hit.to_string(),
    }
}

fn check_lengths(title: &str, body: &str, spec: &ContentSpec, out: &mut Vec<LintIssue>) {
    match char_len(title) {
        0 => out.push(issue(LintCode::TitleEmpty, Severity::Block, LintField::Title, "")),
        n if n > spec.title_max => out.push(issue(LintCode::TitleTooLong, Severity::Block, LintField::Title, format!("{n}/{}", spec.title_max))),
        _ => {}
    }
    match char_len(body) {
        0 => out.push(issue(LintCode::BodyEmpty, Severity::Block, LintField::Body, "")),
        n if n > spec.body_max => out.push(issue(LintCode::BodyTooLong, Severity::Block, LintField::Body, format!("{n}/{}", spec.body_max))),
        _ => {}
    }
}

fn check_images(ids: &[String], pool: &[PublishImage], spec: &ContentSpec, out: &mut Vec<LintIssue>) {
    let n = ids.len();
    if n < spec.images_min {
        out.push(issue(LintCode::TooFewImages, Severity::Block, LintField::Images, format!("{n}/{}", spec.images_min)));
    }
    if n > spec.images_max {
        out.push(issue(LintCode::TooManyImages, Severity::Block, LintField::Images, format!("{n}/{}", spec.images_max)));
    }
    let is_ai = |id: &String| pool.iter().find(|p| &p.asset_id == id).is_some_and(|p| p.ai_generated);
    if spec.first_image_must_be_photo && ids.first().is_some_and(is_ai) {
        out.push(issue(LintCode::FirstImageNotPhoto, Severity::Block, LintField::Images, ""));
    }
    let ai = ids.iter().filter(|id| is_ai(id)).count();
    if ai > 0 {
        out.push(issue(LintCode::AiImages, Severity::Warn, LintField::Images, ai));
    }
}

/// 检查一篇笔记。`pool`:这个项目里可以用的图(要知道哪些是 AI 生成的)。
pub fn lint_note(draft: &NoteDraft, pool: &[PublishImage]) -> Vec<LintIssue> {
    let spec = spec(draft.channel, DraftKind::Note);
    let mut out = Vec::new();
    check_lengths(&draft.title, &draft.body, &spec, &mut out);
    if draft.tags.len() > spec.tags_max {
        out.push(issue(LintCode::TooManyTags, Severity::Block, LintField::Tags, format!("{}/{}", draft.tags.len(), spec.tags_max)));
    }
    check_images(&draft.images, pool, &spec, &mut out);
    scan_text(&draft.title, LintField::Title, &mut out);
    scan_text(&draft.body, LintField::Body, &mut out);
    scan_text(&draft.tags.join(" "), LintField::Tags, &mut out);
    out.sort_by_key(|i| i.severity);
    out
}

/// 检查一条商品。`unit_cost`:成本定价器算出来的单位成本(有的话)。
pub fn lint_listing(draft: &ListingDraft, pool: &[PublishImage], unit_cost: Option<f64>) -> Vec<LintIssue> {
    let spec = spec(draft.channel, DraftKind::Listing);
    let mut out = Vec::new();
    let body = if draft.body.trim().is_empty() { draft.selling_points.join("\n") } else { draft.body.clone() };
    check_lengths(&draft.title, &body, &spec, &mut out);
    check_images(&draft.images, pool, &spec, &mut out);
    scan_text(&draft.title, LintField::Title, &mut out);
    scan_text(&format!("{}\n{}", draft.selling_points.join("\n"), draft.body), LintField::Body, &mut out);
    match draft.price_yuan {
        Some(p) if p > 0.0 => {
            if unit_cost.is_some_and(|c| p < c) {
                out.push(issue(LintCode::PriceBelowCost, Severity::Warn, LintField::Price, format!("{p:.2}")));
            }
        }
        _ => out.push(issue(LintCode::PriceMissing, Severity::Block, LintField::Price, "")),
    }
    match draft.model_license {
        None => out.push(issue(LintCode::LicenseMissing, Severity::Block, LintField::License, "")),
        Some(ModelLicense::NonCommercial) => out.push(issue(LintCode::LicenseNonCommercial, Severity::Block, LintField::License, "")),
        Some(_) => {}
    }
    out.sort_by_key(|i| i.severity);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, body: &str) -> NoteDraft {
        NoteDraft {
            title: title.into(),
            body: body.into(),
            tags: vec!["桌搭".into(), "3D打印".into()],
            images: vec!["a".into()],
            ..Default::default()
        }
    }

    fn pool() -> Vec<PublishImage> {
        vec![
            PublishImage { asset_id: "a".into(), ai_generated: false, source: "photo".into(), ..Default::default() },
            PublishImage { asset_id: "g".into(), ai_generated: true, source: "board".into(), ..Default::default() },
        ]
    }

    fn codes(issues: &[LintIssue]) -> Vec<(LintCode, &str)> {
        issues.iter().map(|i| (i.code, i.hit.as_str())).collect()
    }

    #[test]
    fn a_clean_note_passes() {
        let issues = lint_note(&note("桌面乱线终结者|磁吸线缆夹", "三道线槽,磁吸底座,桌面一下子就清爽了。第一步先把底座贴好,最后把线按进去。"), &pool());
        assert!(issues.is_empty(), "「第一步」「最后」不是极限词:{issues:?}");
    }

    #[test]
    fn lengths_are_counted_in_characters_not_bytes() {
        assert_eq!(char_len("  桌面乱线终结者 ✨ "), 9);
        let long = lint_note(&note(&"字".repeat(21), "正文"), &pool());
        assert_eq!(codes(&long), [(LintCode::TitleTooLong, "21/20")]);
        let empty = lint_note(&note("", " "), &pool());
        assert_eq!(empty.iter().map(|i| i.code).collect::<Vec<_>>(), [LintCode::TitleEmpty, LintCode::BodyEmpty]);
        assert!(has_blockers(&empty));
    }

    #[test]
    fn risky_words_are_found_with_the_word_that_hit() {
        let issues = lint_note(&note("全网第一的线缆夹", "加我微信 13812345678 领好评返现,皮卡丘同款,还能缓解疼痛"), &pool());
        let found = codes(&issues);
        for expected in [
            (LintCode::Extreme, "全网"),
            (LintCode::Extreme, "第一"),
            (LintCode::Diversion, "微信"),
            (LintCode::Diversion, "加我"),
            (LintCode::Diversion, "手机号"),
            (LintCode::FakeReview, "好评返现"),
            (LintCode::IpName, "皮卡丘"),
            (LintCode::Medical, "缓解疼痛"),
        ] {
            assert!(found.contains(&expected), "{expected:?} 应该被查出来:{found:?}");
        }
        assert!(issues.iter().filter(|i| i.code == LintCode::Extreme).all(|i| i.field == LintField::Title));
    }

    #[test]
    fn category_words_only_warn() {
        let issues = lint_note(&note("月球小夜灯", "USB 供电,放在宝宝房间。"), &pool());
        assert!(!has_blockers(&issues), "{issues:?}");
        let kinds: Vec<LintCode> = issues.iter().map(|i| i.code).collect();
        assert!(kinds.contains(&LintCode::CategoryElectric) && kinds.contains(&LintCode::CategoryKids));
    }

    #[test]
    fn ai_images_are_a_reminder_for_notes_but_cannot_lead_a_listing() {
        let mut n = note("磁吸线缆夹", "正文");
        n.images = vec!["g".into(), "a".into()];
        assert_eq!(codes(&lint_note(&n, &pool())), [(LintCode::AiImages, "1")], "笔记可以用 AI 图,只提醒勾选 AI 内容声明");

        let listing = ListingDraft {
            title: "磁吸线缆夹 三槽 桌面理线".into(),
            body: "PLA 打印,三道线槽。".into(),
            price_yuan: Some(29.9),
            model_license: Some(ModelLicense::Original),
            images: vec!["g".into(), "a".into(), "a".into()],
            ..Default::default()
        };
        let issues = lint_listing(&listing, &pool(), Some(12.0));
        assert!(issues.iter().any(|i| i.code == LintCode::FirstImageNotPhoto && i.severity == Severity::Block), "{issues:?}");
    }

    #[test]
    fn a_listing_needs_images_a_price_and_a_sellable_model() {
        let mut l = ListingDraft {
            title: "磁吸线缆夹".into(),
            selling_points: vec!["三道线槽".into()],
            images: vec!["a".into()],
            ..Default::default()
        };
        let kinds: Vec<LintCode> = lint_listing(&l, &pool(), None).iter().map(|i| i.code).collect();
        assert_eq!(kinds, [LintCode::TooFewImages, LintCode::PriceMissing, LintCode::LicenseMissing], "卖点可以顶替详情");

        l.images = vec!["a".into(); 3];
        l.price_yuan = Some(9.9);
        l.model_license = Some(ModelLicense::NonCommercial);
        let issues = lint_listing(&l, &pool(), Some(12.5));
        assert!(issues.iter().any(|i| i.code == LintCode::LicenseNonCommercial && i.severity == Severity::Block));
        assert!(issues.iter().any(|i| i.code == LintCode::PriceBelowCost && i.severity == Severity::Warn));

        l.model_license = Some(ModelLicense::Original);
        l.price_yuan = Some(29.9);
        assert!(lint_listing(&l, &pool(), Some(12.5)).is_empty());
    }

    #[test]
    fn tags_are_split_deduplicated_and_joined_for_pasting() {
        let tags = parse_tags("#桌搭 #3D打印,线缆收纳、桌搭 ;理线\u{ff0c}桌面\u{ff1b}收纳");
        assert_eq!(tags, ["桌搭", "3D打印", "线缆收纳", "理线", "桌面", "收纳"]);
        assert_eq!(tags_line(&tags[..4]), "#桌搭 #3D打印 #线缆收纳 #理线");
        assert_eq!(tags_line(&["#已经带了".into(), " ".into()]), "#已经带了");
        let mut n = note("磁吸线缆夹", "正文");
        n.tags = (0..6).map(|i| format!("话题{i}")).collect();
        assert_eq!(codes(&lint_note(&n, &pool())), [(LintCode::TooManyTags, "6/5")]);
    }

    #[test]
    fn phone_numbers_are_eleven_digits_starting_with_1() {
        assert!(has_phone_number("联系 13812345678"));
        assert!(has_phone_number("13812345678"));
        assert!(!has_phone_number("订单号 202609211234567"), "更长的数字串不是手机号");
        assert!(!has_phone_number("尺寸 12012345678"), "第二位不是 3~9");
    }

    #[test]
    fn drafts_travel_as_tagged_json() {
        let l = ListingDraft { ship: ShipMode::Presale { days: 7 }, model_license: Some(ModelLicense::Licensed), ..Default::default() };
        let wire = serde_json::to_value(&l).unwrap();
        assert_eq!(wire["ship"], serde_json::json!({"mode": "presale", "days": 7}));
        assert_eq!(wire["model_license"], "licensed");
        assert_eq!(serde_json::from_value::<ListingDraft>(wire).unwrap(), l);
        assert_eq!(NoteAngle::parse("backstage"), NoteAngle::Backstage);
        assert_eq!(NoteAngle::parse("???"), NoteAngle::Pain);
        assert_eq!(spec(Channel::Xhs, DraftKind::Note).title_max, 20);
        assert!(spec(Channel::Xhs, DraftKind::Listing).first_image_must_be_photo);
    }
}
