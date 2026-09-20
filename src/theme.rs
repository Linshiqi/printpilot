//! 主题:`.dark` 类挂在 <html> 上(Tailwind v4 的 dark 变体跟着它走),偏好存 localStorage。

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "localStorage"], js_name = getItem)]
    fn ls_get(key: &str) -> Option<String>;

    #[wasm_bindgen(js_namespace = ["window", "localStorage"], js_name = setItem)]
    fn ls_set(key: &str, value: &str);
}

/// localStorage 读写。只放**界面偏好**(主题、语言、上次停留的页面);业务数据一律进后端数据库。
pub fn get_pref(key: &str) -> Option<String> {
    ls_get(key)
}

pub fn set_pref(key: &str, value: &str) {
    ls_set(key, value);
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn toggle(self) -> Self {
        match self {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Light,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "dark" => Theme::Dark,
            _ => Theme::Light,
        }
    }

    /// 用户选过就用用户的;没选过跟随系统。
    pub fn init() -> Self {
        match get_pref("theme") {
            Some(saved) => Theme::parse(&saved),
            None if system_prefers_dark() => Theme::Dark,
            None => Theme::Light,
        }
    }
}

fn system_prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|m| m.matches())
}

pub fn apply_theme(theme: Theme) {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.class_list().toggle_with_force("dark", theme == Theme::Dark);
    }
    set_pref("theme", theme.as_str());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_lenient_and_roundtrips() {
        assert_eq!(Theme::parse(Theme::Dark.as_str()), Theme::Dark);
        assert_eq!(Theme::parse(Theme::Light.as_str()), Theme::Light);
        assert_eq!(Theme::parse("solarized"), Theme::Light);
        assert_eq!(Theme::Dark.toggle().toggle(), Theme::Dark);
    }
}
