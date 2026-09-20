//! i18n 护栏(velo 的做法)。leptos_i18n 已经保证「代码里用到的键在默认语言里存在」;
//! 这里再守住两件它不管的事:各语言的键集合完全一致、非中文语言里没有混进中文占位。
#![cfg(test)]

use std::collections::BTreeSet;

const ZH: &str = include_str!("../locales/zh.json");
const EN: &str = include_str!("../locales/en.json");

fn flatten(prefix: &str, v: &serde_json::Value, out: &mut BTreeSet<String>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, child) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, child, out);
            }
        }
        _ => {
            out.insert(prefix.to_string());
        }
    }
}

fn keys(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    flatten("", &serde_json::from_str(text).expect("locale JSON 可解析"), &mut out);
    out
}

fn strings(text: &str) -> Vec<(String, String)> {
    fn walk(prefix: &str, v: &serde_json::Value, out: &mut Vec<(String, String)>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, child) in map {
                    walk(&format!("{prefix}.{k}"), child, out);
                }
            }
            serde_json::Value::String(s) => out.push((prefix.trim_start_matches('.').to_string(), s.clone())),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk("", &serde_json::from_str(text).unwrap(), &mut out);
    out
}

#[test]
fn every_locale_has_exactly_the_same_keys() {
    let (zh, en) = (keys(ZH), keys(EN));
    let missing_in_en: Vec<_> = zh.difference(&en).collect();
    let extra_in_en: Vec<_> = en.difference(&zh).collect();
    assert!(missing_in_en.is_empty(), "en.json 缺少: {missing_in_en:?}");
    assert!(extra_in_en.is_empty(), "en.json 多出: {extra_in_en:?}");
}

#[test]
fn english_has_no_untranslated_chinese_left_in_it() {
    let leftovers: Vec<_> = strings(EN)
        .into_iter()
        .filter(|(_, s)| s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
        .collect();
    assert!(leftovers.is_empty(), "en.json 里还有中文: {leftovers:?}");
}

#[test]
fn interpolation_variables_match_across_locales() {
    // 「已停留 {{ days }} 天」翻成英文时把变量名写错,运行时才会发现——这里提前拦住
    fn vars(s: &str) -> BTreeSet<String> {
        s.split("{{")
            .skip(1)
            .filter_map(|rest| rest.split("}}").next())
            .map(|v| v.trim().to_string())
            .collect()
    }
    let en: std::collections::BTreeMap<_, _> = strings(EN).into_iter().collect();
    for (key, zh_text) in strings(ZH) {
        let en_text = en.get(&key).expect("键集合一致性由上一个测试保证");
        assert_eq!(vars(&zh_text), vars(en_text), "变量不一致: {key}");
    }
}
