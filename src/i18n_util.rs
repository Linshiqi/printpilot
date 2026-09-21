//! i18n 辅助(做法照搬 velo)。
//!
//! 响应式上下文(组件里)用 `t!` / `t_string!(i18n, ns.key)`;**非响应式上下文**(事件回调、
//! 异步任务、纯函数)拿不到 i18n 上下文,用 `td_string!(current_locale(), ns.key)`。
//! 当前语言存在 thread_local 里,由 App 在语言变化时同步。

use std::cell::Cell;

use leptos_i18n::td_string;
use pp_common::gate::GateKey;
use pp_common::{errcode, ProjectStatus, Stage};

use crate::i18n::Locale;

thread_local! {
    static CURRENT: Cell<Locale> = const { Cell::new(Locale::zh) };
}

pub fn set_current_locale(locale: Locale) {
    CURRENT.with(|c| c.set(locale));
}

pub fn current_locale() -> Locale {
    CURRENT.with(|c| c.get())
}

/// 后端错误串 `#错误码#细节` → 当前语言的提示。没有 `#` 前缀的原样返回。
/// 新增错误码:pp_common::errcode 加常量 + locales/*.json 的 backend 段加文案 + 这里加分支。
pub fn localize_backend_err(raw: &str) -> String {
    let Some((code, detail)) = errcode::split(raw) else {
        return raw.to_string();
    };
    let l = current_locale();
    let msg = match code {
        errcode::NOT_FOUND => td_string!(l, backend.not_found),
        errcode::INVALID_INPUT => td_string!(l, backend.invalid_input),
        errcode::DB_FAILED => td_string!(l, backend.db_failed),
        errcode::IO_FAILED => td_string!(l, backend.io_failed),
        errcode::KEYRING_FAILED => td_string!(l, backend.keyring_failed),
        errcode::MESH_UNREADABLE => td_string!(l, backend.mesh_unreadable),
        errcode::MESH_NOT_CLOSED => td_string!(l, backend.mesh_not_closed),
        errcode::MESH_EMPTY_RESULT => td_string!(l, backend.mesh_empty_result),
        errcode::PROVIDER_MISSING_KEY => td_string!(l, backend.provider_missing_key),
        errcode::PROVIDER_AUTH => td_string!(l, backend.provider_auth),
        errcode::PROVIDER_BALANCE => td_string!(l, backend.provider_balance),
        errcode::PROVIDER_RATE_LIMITED => td_string!(l, backend.provider_rate_limited),
        errcode::PROVIDER_HTTP => td_string!(l, backend.provider_http),
        errcode::PROVIDER_NETWORK => td_string!(l, backend.provider_network),
        errcode::PROVIDER_BAD_RESPONSE => td_string!(l, backend.provider_bad_response),
        errcode::AGENT_BAD_OUTPUT => td_string!(l, backend.agent_bad_output),
        errcode::CAD_ENGINE_MISSING => td_string!(l, backend.cad_engine_missing),
        errcode::CAD_ENGINE_FAILED => td_string!(l, backend.cad_engine_failed),
        errcode::CAD_TIMEOUT => td_string!(l, backend.cad_timeout),
        errcode::CAD_SCRIPT_ERROR => td_string!(l, backend.cad_script_error),
        errcode::IMAGE_UNREADABLE => td_string!(l, backend.image_unreadable),
        errcode::LINT_BLOCKED => td_string!(l, backend.lint_blocked),
        errcode::UPDATE_CHECK_FAILED => td_string!(l, backend.update_check_failed),
        errcode::UPDATE_INSTALL_FAILED => td_string!(l, backend.update_install_failed),
        // 用户自己点的停止:不带细节,`is_cancelled` 靠整句相等来认它
        errcode::CANCELLED => return td_string!(l, backend.cancelled).to_string(),
        _ => td_string!(l, backend.unknown),
    };
    // 细节是给排查问题用的英文/路径,附在后面;对用户有用的那一半已经在 msg 里了
    if detail.is_empty() {
        msg.to_string()
    } else {
        format!("{msg}({detail})")
    }
}

/// 这条(已经本地化过的)错误是不是「用户自己点了停止」。
pub fn is_cancelled(localized: &str) -> bool {
    localized == td_string!(current_locale(), backend.cancelled)
}

/// 阶段门清单里的一项(「离开这个阶段之前该有什么」)。
pub fn gate_label(l: Locale, key: GateKey) -> &'static str {
    match key {
        GateKey::Hypothesis => td_string!(l, project.gate_hypothesis),
        GateKey::Research => td_string!(l, project.gate_research),
        GateKey::AdoptedImage => td_string!(l, project.gate_adopted_image),
        GateKey::Model => td_string!(l, project.gate_model),
        GateKey::PrintRun => td_string!(l, project.gate_print_run),
        GateKey::Price => td_string!(l, project.gate_price),
        GateKey::PublishPack => td_string!(l, project.gate_publish_pack),
        GateKey::Published => td_string!(l, project.gate_published),
    }
}

pub fn stage_name(l: Locale, s: Stage) -> &'static str {
    match s {
        Stage::Idea => td_string!(l, stage.idea),
        Stage::Research => td_string!(l, stage.research),
        Stage::Concept => td_string!(l, stage.concept),
        Stage::Model => td_string!(l, stage.model),
        Stage::Prototype => td_string!(l, stage.prototype),
        Stage::Listing => td_string!(l, stage.listing),
        Stage::Operating => td_string!(l, stage.operating),
        Stage::Review => td_string!(l, stage.review),
    }
}

pub fn status_name(l: Locale, s: ProjectStatus) -> &'static str {
    match s {
        ProjectStatus::Active => td_string!(l, status.active),
        ProjectStatus::Paused => td_string!(l, status.paused),
        ProjectStatus::Killed => td_string!(l, status.killed),
        ProjectStatus::Done => td_string!(l, status.done),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coded_errors_are_localized_and_keep_the_detail() {
        set_current_locale(Locale::zh);
        assert_eq!(localize_backend_err("#not_found#project 42"), "找不到对应的记录(project 42)");
        set_current_locale(Locale::en);
        assert_eq!(localize_backend_err("#not_found#"), "Record not found");
    }

    #[test]
    fn unknown_codes_fall_back_and_plain_strings_pass_through() {
        set_current_locale(Locale::zh);
        assert_eq!(localize_backend_err("#brand_new_code#x"), "出错了(x)");
        assert_eq!(localize_backend_err("plain message"), "plain message");
    }

    #[test]
    fn every_stage_and_status_has_a_name_in_every_locale() {
        for l in [Locale::zh, Locale::en] {
            for s in Stage::ALL {
                assert!(!stage_name(l, s).is_empty());
            }
            for st in [
                ProjectStatus::Active,
                ProjectStatus::Paused,
                ProjectStatus::Killed,
                ProjectStatus::Done,
            ] {
                assert!(!status_name(l, st).is_empty());
            }
        }
    }
}
