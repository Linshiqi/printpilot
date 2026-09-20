//! 设计规格的编辑器。视觉模型「看图」出来的规格先摆在这里给人审、给人改,确认了再去生成代码——
//! 这一步是质量的第一道闸:尺寸和特征清单错了,后面写得再好也是错的。

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos_i18n::t_string;
use pp_common::cad::{DesignFeature, DesignSpec};

use crate::i18n::use_i18n;
use crate::icon::IconKind;
use crate::ui::{Badge, Button, ButtonVariant, Field, IconButton, TextArea, TextInput, Tone};
use crate::utils::{fmt_mm, parse_positive};

/// 一个特征的可编辑形态。尺寸用多行文本(`名称 = 数值`)编辑——比一堆小输入框好改,也好增删。
#[derive(Clone, Copy)]
pub struct FeatureDraft {
    pub key: u32,
    pub name: RwSignal<String>,
    pub description: RwSignal<String>,
    pub dims: RwSignal<String>,
}

/// 规格的可编辑形态:每个字段一个信号,直接绑到输入框上。
#[derive(Clone, Copy)]
pub struct SpecDraft {
    pub name: RwSignal<String>,
    pub summary: RwSignal<String>,
    pub size: [RwSignal<String>; 3],
    pub features: RwSignal<Vec<FeatureDraft>>,
    pub print_notes: RwSignal<String>,
    assumptions: StoredValue<Vec<String>>,
    unsuitable_reason: StoredValue<Option<String>>,
    next_key: StoredValue<u32>,
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

pub fn dims_to_text(dims: &BTreeMap<String, f64>) -> String {
    dims.iter().map(|(k, v)| format!("{k} = {}", fmt_num(*v))).collect::<Vec<_>>().join("\n")
}

/// `名称 = 数值` 每行一个(也接受冒号、全角符号)。看不懂的那一行作为 `Err` 返回,让用户知道是哪一行。
pub fn parse_dims(text: &str) -> Result<BTreeMap<String, f64>, String> {
    let mut out = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let parsed = line.split_once(['=', ':', '=', ':']).and_then(|(k, v)| {
            let key = k.trim().replace(' ', "_");
            // 数值后面可以带单位(「6 mm」「3 个」):只取开头的数字部分
            let number: String = v
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | ',' | '\u{FF10}'..='\u{FF19}'))
                .collect();
            let value = if number == "0" { Some(0.0) } else { parse_positive(&number) };
            (!key.is_empty()).then_some(key).zip(value)
        });
        match parsed {
            Some((k, v)) => {
                out.insert(k, v);
            }
            None => return Err(line.to_string()),
        }
    }
    Ok(out)
}

/// 草稿为什么还不能拿去生成。
#[derive(Debug, Clone, PartialEq)]
pub enum SpecProblem {
    BadSize,
    NoFeatures,
    BadDim(String),
}

impl SpecDraft {
    pub fn from_spec(spec: &DesignSpec) -> Self {
        let mut key = 0;
        let features = spec
            .features
            .iter()
            .map(|f| {
                key += 1;
                FeatureDraft {
                    key,
                    name: RwSignal::new(f.name.clone()),
                    description: RwSignal::new(f.description.clone()),
                    dims: RwSignal::new(dims_to_text(&f.dimensions)),
                }
            })
            .collect();
        Self {
            name: RwSignal::new(spec.name.clone()),
            summary: RwSignal::new(spec.summary.clone()),
            size: spec.overall_mm.map(|v| RwSignal::new(fmt_num(v))),
            features: RwSignal::new(features),
            print_notes: RwSignal::new(spec.print_notes.clone()),
            assumptions: StoredValue::new(spec.assumptions.clone()),
            unsuitable_reason: StoredValue::new((!spec.suitable).then(|| spec.unsuitable_reason.clone())),
            next_key: StoredValue::new(key + 1),
        }
    }

    fn add_feature(self) {
        let key = self.next_key.get_value();
        self.next_key.set_value(key + 1);
        self.features.update(|list| {
            list.push(FeatureDraft {
                key,
                name: RwSignal::new(String::new()),
                description: RwSignal::new(String::new()),
                dims: RwSignal::new(String::new()),
            })
        });
    }

    /// 草稿 → 规格。空白的特征行(名字和描述都没填)直接丢掉。
    pub fn to_spec(self) -> Result<DesignSpec, SpecProblem> {
        let mut overall = [0.0; 3];
        for (slot, text) in overall.iter_mut().zip(self.size) {
            *slot = text.with_untracked(|t| parse_positive(t)).ok_or(SpecProblem::BadSize)?;
        }
        let mut features = Vec::new();
        for f in self.features.get_untracked() {
            let name = f.name.get_untracked().trim().to_string();
            let description = f.description.get_untracked().trim().to_string();
            if name.is_empty() && description.is_empty() {
                continue;
            }
            let dimensions = f.dims.with_untracked(|d| parse_dims(d)).map_err(SpecProblem::BadDim)?;
            features.push(DesignFeature {
                name: if name.is_empty() { format!("feature_{}", features.len() + 1) } else { name },
                description,
                dimensions,
            });
        }
        if features.is_empty() {
            return Err(SpecProblem::NoFeatures);
        }
        Ok(DesignSpec {
            name: self.name.get_untracked().trim().to_string(),
            summary: self.summary.get_untracked().trim().to_string(),
            suitable: true, // 走到「生成」这一步,就是用户决定要试
            unsuitable_reason: String::new(),
            overall_mm: overall,
            features,
            assumptions: self.assumptions.get_value(),
            print_notes: self.print_notes.get_untracked().trim().to_string(),
        })
    }
}

#[component]
pub fn SpecEditor(draft: SpecDraft, build_mm: [f64; 3]) -> impl IntoView {
    let i18n = use_i18n();
    let assumptions = draft.assumptions.get_value();
    let unsuitable = draft.unsuitable_reason.get_value();
    let too_big = move || {
        let size: Vec<f64> = draft.size.iter().filter_map(|s| s.with(|t| parse_positive(t))).collect();
        size.len() == 3
            && !(size[2] <= build_mm[2]
                && ((size[0] <= build_mm[0] && size[1] <= build_mm[1]) || (size[0] <= build_mm[1] && size[1] <= build_mm[0])))
    };
    let remove = move |key: u32| draft.features.update(|list| list.retain(|f| f.key != key));

    view! {
        <div class="space-y-3">
            {unsuitable.map(|reason| view! {
                <div class="rounded-lg border border-amber-300 bg-amber-50 text-amber-800 dark:bg-amber-500/10 dark:border-amber-500/40 dark:text-amber-200 px-3 py-2 text-xs leading-relaxed">
                    {move || t_string!(i18n, cad.spec_unsuitable, reason = reason.clone()).to_string()}
                </div>
            })}
            <Field label=move || t_string!(i18n, cad.spec_name)>
                <TextInput value=draft.name/>
            </Field>
            <Field label=move || t_string!(i18n, cad.spec_overall)>
                <div class="flex items-center gap-1.5">
                    <TextInput value=draft.size[0] numeric=true/>
                    <span class="text-gray-400">"×"</span>
                    <TextInput value=draft.size[1] numeric=true/>
                    <span class="text-gray-400">"×"</span>
                    <TextInput value=draft.size[2] numeric=true/>
                </div>
            </Field>
            <Show when=too_big>
                <Badge tone=Tone::Red>
                    {move || t_string!(i18n, cad.spec_too_big)}
                    {format!(" · {} × {} × {}", fmt_mm(build_mm[0]), fmt_mm(build_mm[1]), fmt_mm(build_mm[2]))}
                </Badge>
            </Show>

            <div class="space-y-2">
                <div class="text-xs font-medium text-gray-600 dark:text-gray-300">{move || t_string!(i18n, cad.spec_features)}</div>
                <For each=move || draft.features.get() key=|f| f.key let:f>
                    <div class="rounded-lg border border-gray-200 dark:border-gray-700 p-2 space-y-1.5">
                        <div class="flex items-center gap-1.5">
                            <TextInput value=f.name placeholder=move || t_string!(i18n, cad.spec_feature_name)/>
                            <IconButton icon=IconKind::Trash label=move || t_string!(i18n, common.delete) on_click=move || remove(f.key)/>
                        </div>
                        <TextArea value=f.description rows=2 placeholder=move || t_string!(i18n, cad.spec_feature_desc)/>
                        <TextArea value=f.dims rows=3 placeholder=move || t_string!(i18n, cad.spec_dims_hint)/>
                    </div>
                </For>
                <Button small=true variant=ButtonVariant::Ghost icon=IconKind::Plus on_click=move || draft.add_feature()>
                    {move || t_string!(i18n, cad.add_feature)}
                </Button>
            </div>

            {(!assumptions.is_empty()).then(|| view! {
                <div class="space-y-1">
                    <div class="text-xs font-medium text-amber-700 dark:text-amber-300">{move || t_string!(i18n, cad.spec_assumptions)}</div>
                    <ul class="list-disc pl-4 space-y-0.5 text-xs leading-relaxed text-gray-600 dark:text-gray-300 selectable">
                        {assumptions.iter().map(|a| view! { <li>{a.clone()}</li> }).collect_view()}
                    </ul>
                </div>
            })}
            <Field label=move || t_string!(i18n, cad.spec_notes)>
                <TextArea value=draft.print_notes rows=2/>
            </Field>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_lines_tolerate_units_colons_and_full_width_input() {
        let dims = parse_dims("length = 60\nslot_width: 6 mm\ncount = 3 个\n\ncorner radius = 4.5\n深度=9").unwrap();
        assert_eq!(dims["length"], 60.0);
        assert_eq!(dims["slot_width"], 6.0);
        assert_eq!(dims["count"], 3.0);
        assert_eq!(dims["corner_radius"], 4.5, "名字里的空格换成下划线");
        assert_eq!(dims["深度"], 9.0);
        assert_eq!(parse_dims("offset = 0").unwrap()["offset"], 0.0, "0 是合法的尺寸");
    }

    #[test]
    fn a_line_that_makes_no_sense_is_named_in_the_error() {
        assert_eq!(parse_dims("length = 60\n大概这么宽\n"), Err("大概这么宽".to_string()));
        assert_eq!(parse_dims("width = abc"), Err("width = abc".to_string()));
        assert!(parse_dims("").unwrap().is_empty());
    }

    #[test]
    fn dimensions_roundtrip_through_the_text_form() {
        let mut dims = BTreeMap::new();
        dims.insert("length".to_string(), 60.0);
        dims.insert("fillet".to_string(), 2.5);
        let text = dims_to_text(&dims);
        assert_eq!(text, "fillet = 2.5\nlength = 60");
        assert_eq!(parse_dims(&text).unwrap(), dims);
    }
}
