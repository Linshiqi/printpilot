//! 全局状态(做法照搬 velo):一个 `Copy` 的扁平结构体,字段全是 `RwSignal`。
//! `App` 里 `provide_context` 一次,各处 `use_context::<AppState>()`;因为是 `Copy`,闭包里直接 move。
//!
//! 协作约定:view 只读信号、只调 controller 方法;controller 方法里 `spawn_local` → `ipc::call` → 写回信号。

use std::collections::HashMap;

use leptos::prelude::*;
use pp_common::gate::ProjectFacts;
use pp_common::provider::ProviderStatus;
use pp_common::{AppInfo, Project};

use crate::theme::Theme;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Dashboard,
    Projects,
    /// 图片工作台(接图片模型出图,对话式改图)
    Images,
    /// 建模工作室(看图 → 代码建模,对话式修改)
    Studio,
    /// 打样与定价(成本模型、三档建议价、打样记录)
    Pricing,
    Ideas,
    Calendar,
    Orders,
    Analytics,
    Settings,
    Lab,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Route::Dashboard => "dashboard",
            Route::Projects => "projects",
            Route::Images => "images",
            Route::Studio => "studio",
            Route::Pricing => "pricing",
            Route::Ideas => "ideas",
            Route::Calendar => "calendar",
            Route::Orders => "orders",
            Route::Analytics => "analytics",
            Route::Settings => "settings",
            Route::Lab => "lab",
        }
    }

    pub fn parse(s: &str) -> Option<Route> {
        [
            Route::Dashboard,
            Route::Projects,
            Route::Images,
            Route::Studio,
            Route::Pricing,
            Route::Ideas,
            Route::Calendar,
            Route::Orders,
            Route::Analytics,
            Route::Settings,
            Route::Lab,
        ]
        .into_iter()
        .find(|r| r.as_str() == s)
    }
}

/// 模型正在说的话(流式):后端把片段攒 50 毫秒发一次,这里接起来。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveAnswer {
    /// 思考模式下的思维链(先到)
    pub reasoning: String,
    /// 正文:一句话,后面可能跟着一整段 ```python 代码
    pub content: String,
}

impl LiveAnswer {
    /// 正文拆成「给人看的话」和「代码写到第几行了」。代码围栏之前的是话;围栏之后的只数行数——
    /// 对话栏很窄,把几百行代码刷过去没有意义,让人知道它在写、写了多少就够了。
    pub fn split(&self) -> (String, Option<usize>) {
        match self.content.find("```") {
            None => (self.content.trim().to_string(), None),
            Some(at) => {
                let code = &self.content[at..];
                let lines = code.lines().count().saturating_sub(1); // 围栏那一行不算
                (self.content[..at].trim().to_string(), Some(lines))
            }
        }
    }

    /// 思维链只留最后一小段:它可能有几千字,而且是给模型自己看的
    pub fn reasoning_tail(&self, max_chars: usize) -> String {
        let chars: Vec<char> = self.reasoning.chars().collect();
        let start = chars.len().saturating_sub(max_chars);
        let tail: String = chars[start..].iter().collect();
        if start > 0 {
            format!("…{}", tail.trim_start())
        } else {
            tail
        }
    }
}

/// 从项目里发起一件事、跳到某个工作台时带过去的东西。目标页面挂载时取走(`take`),只用一次。
#[derive(Clone, Debug, PartialEq)]
pub enum Handoff {
    /// 去「调研」:为这个项目做一次调研,主题先填好
    Research { project_id: String, topic: String },
    /// 去「调研」:打开一份已有的报告
    ResearchRun { run_id: String },
    /// 去「图片」:打开这个画板,输入框里先放一句草稿(可以为空)
    Board { board_id: String, draft: String },
    /// 去「建模」:打开这个设计,输入框里先放一句草稿(可以为空)
    Design { design_id: String, draft: String },
    /// 去「定价」:打开这个项目的成本模型
    Pricing { project_id: String },
}

#[derive(Copy, Clone)]
pub struct AppState {
    pub route: RwSignal<Route>,
    pub theme: RwSignal<Theme>,
    /// 错误提示(红)与普通提示(绿),各自定时消失
    pub toast: RwSignal<Option<String>>,
    pub info_toast: RwSignal<Option<String>>,
    pub app_info: RwSignal<Option<AppInfo>>,
    pub projects: RwSignal<Vec<Project>>,
    pub projects_loaded: RwSignal<bool>,
    /// 右侧抽屉里打开的项目
    pub open_project: RwSignal<Option<String>>,
    /// 每个项目名下的产出计数(调研 / 图 / 模型):看板卡片、阶段门清单、拖拽时要不要写原因都靠它
    pub project_facts: RwSignal<HashMap<String, ProjectFacts>>,
    /// 从项目跳到工作台时带过去的东西
    pub handoff: RwSignal<Option<Handoff>>,
    /// 「现在」的毫秒时间戳,每分钟刷新一次:看板上的「已停留 N 天」靠它更新
    pub now_ms: RwSignal<i64>,
    /// 各供应商有没有配密钥(只有「有 / 没有」,密钥本身永远不到前端)
    pub providers: RwSignal<Vec<ProviderStatus>>,
    /// 正在运行的调研走到了哪一步:`(step, done, total)`,由后端的 research-progress 事件驱动
    pub research_progress: RwSignal<Option<(String, u32, u32)>>,
    /// 正在运行的建模走到了哪一步:`(step, attempt, problems)`,由后端的 cad-progress 事件驱动
    pub cad_progress: RwSignal<Option<(String, u32, u32)>>,
    /// 建模对话里模型正在说的话(流式),由 cad-stream 事件驱动;一轮结束就清空
    pub cad_stream: RwSignal<LiveAnswer>,
    /// 引擎包解包进度:`(phase, done, total)`,由 cad-engine-progress 事件驱动
    pub cad_engine_progress: RwSignal<Option<(String, u64, u64)>>,
    /// 正在出图的那一轮走到了哪一步:`(phase, provider, count)`,由 image-progress 事件驱动
    pub image_progress: RwSignal<Option<(String, String, u32)>>,
    /// 开着的右键菜单(`ui::context_menu`)。元素上用 `state.open_menu(&ev, 菜单项)` 打开
    pub context_menu: RwSignal<Option<crate::ui::ContextMenu>>,
}

impl AppState {
    pub fn new() -> Self {
        // 上次停留的页面(界面偏好,存 localStorage)
        let route = crate::theme::get_pref("route")
            .and_then(|s| Route::parse(&s))
            .unwrap_or(Route::Projects);
        Self {
            route: RwSignal::new(route),
            theme: RwSignal::new(Theme::init()),
            toast: RwSignal::new(None),
            info_toast: RwSignal::new(None),
            app_info: RwSignal::new(None),
            projects: RwSignal::new(Vec::new()),
            projects_loaded: RwSignal::new(false),
            open_project: RwSignal::new(None),
            project_facts: RwSignal::new(HashMap::new()),
            handoff: RwSignal::new(None),
            now_ms: RwSignal::new(crate::utils::now_ms()),
            providers: RwSignal::new(Vec::new()),
            research_progress: RwSignal::new(None),
            cad_progress: RwSignal::new(None),
            cad_stream: RwSignal::new(LiveAnswer::default()),
            cad_engine_progress: RwSignal::new(None),
            image_progress: RwSignal::new(None),
            context_menu: RwSignal::new(None),
        }
    }

    /// 重新数一遍各项目名下的产出。工作台里做了可能改变清单的事(出图、采用、建出模型、关联项目…)之后调用。
    pub fn reload_project_facts(self) {
        leptos::task::spawn_local(async move {
            // 读不到就保持原样:这只影响小图标和提示,不值得为它弹错误
            if let Ok(map) = crate::ipc::call_no_args::<HashMap<String, ProjectFacts>>(crate::ipc::cmd::PROJECT_FACTS_ALL).await {
                self.project_facts.set(map);
            }
        });
    }

    /// 某个项目的产出计数(还没读到时是一组 0)。在响应式上下文里调用会被跟踪。
    pub fn facts_of(self, project_id: &str) -> ProjectFacts {
        self.project_facts.with(|m| m.get(project_id).copied().unwrap_or_default())
    }

    /// 重新读一遍各供应商的密钥状态(启动时、保存或删除密钥之后)。
    pub fn reload_providers(self) {
        leptos::task::spawn_local(async move {
            match crate::ipc::call_no_args::<Vec<ProviderStatus>>(crate::ipc::cmd::PROVIDER_STATUS).await {
                Ok(list) => self.providers.set(list),
                Err(e) => self.notify_error(e),
            }
        });
    }

    /// 取消正在跑的一轮。`scope`:`design:<id>` / `board:<id>` / `research`。结果由那一轮自己的调用返回(「已停止」)。
    pub fn cancel_turn(self, scope: String) {
        leptos::task::spawn_local(async move {
            let _ = crate::ipc::call::<_, bool>(crate::ipc::cmd::CANCEL_TURN, &serde_json::json!({ "scope": scope })).await;
        });
    }

    pub fn notify_error(self, msg: impl Into<String>) {
        let text = msg.into();
        // 用户自己点的「停止」不是故障:不弹红色的错误,轻轻说一声就行
        if crate::i18n_util::is_cancelled(&text) {
            self.notify_info(text);
            return;
        }
        web_sys::console::error_1(&text.clone().into());
        self.toast.set(Some(text));
        let toast = self.toast;
        set_timeout(move || toast.set(None), std::time::Duration::from_millis(6000));
    }

    pub fn notify_info(self, msg: impl Into<String>) {
        self.info_toast.set(Some(msg.into()));
        let toast = self.info_toast;
        set_timeout(move || toast.set(None), std::time::Duration::from_millis(2500));
    }

    pub fn go(self, route: Route) {
        self.route.set(route);
        crate::theme::set_pref("route", route.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_strings_roundtrip() {
        for r in [
            Route::Dashboard,
            Route::Projects,
            Route::Images,
            Route::Studio,
            Route::Pricing,
            Route::Ideas,
            Route::Calendar,
            Route::Orders,
            Route::Analytics,
            Route::Settings,
            Route::Lab,
        ] {
            assert_eq!(Route::parse(r.as_str()), Some(r));
        }
        assert_eq!(Route::parse("nowhere"), None);
    }

    #[test]
    fn a_live_answer_shows_the_words_and_only_counts_the_code() {
        let mut live = LiveAnswer::default();
        assert_eq!(live.split(), (String::new(), None));
        live.content = "线槽加宽到 8 mm".into();
        assert_eq!(live.split(), ("线槽加宽到 8 mm".into(), None), "还没写到代码");
        live.content.push_str(",其余不变。\n\n```python\nslot_w = 8\nresult = base");
        assert_eq!(live.split(), ("线槽加宽到 8 mm,其余不变。".into(), Some(2)), "围栏那一行不算代码");

        live.reasoning = "先看线槽的宽度参数在哪一段".into();
        assert_eq!(live.reasoning_tail(100), "先看线槽的宽度参数在哪一段");
        assert_eq!(live.reasoning_tail(4), "…在哪一段", "按字符截,不会把汉字切坏");
    }
}
