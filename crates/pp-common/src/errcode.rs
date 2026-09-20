//! 后端错误约定(照搬 velo):命令一律返回 `Result<T, String>`,错误串格式为
//! **`#错误码#细节`**。前端 `i18n_util::localize_backend_err` 按码翻成当前语言再弹 toast;
//! 没有 `#` 前缀的串原样显示。新增错误码时:这里加常量 + locales/*.json 的 `backend` 段加文案
//! + 前端 match 加分支。

pub const NOT_FOUND: &str = "not_found";
pub const INVALID_INPUT: &str = "invalid_input";
pub const DB_FAILED: &str = "db_failed";
pub const IO_FAILED: &str = "io_failed";
pub const KEYRING_FAILED: &str = "keyring_failed";
pub const MESH_UNREADABLE: &str = "mesh_unreadable";
pub const MESH_NOT_CLOSED: &str = "mesh_not_closed";
pub const MESH_EMPTY_RESULT: &str = "mesh_empty_result";
// 供应商与 Agent 的错误码由 pp-providers / pp-agent 给出(ProviderError::code / AgentError::code),
// 这里列出来是为了让前端的本地化 match 有常量可用
pub const PROVIDER_MISSING_KEY: &str = "provider_missing_key";
pub const PROVIDER_AUTH: &str = "provider_auth";
pub const PROVIDER_BALANCE: &str = "provider_balance";
pub const PROVIDER_RATE_LIMITED: &str = "provider_rate_limited";
pub const PROVIDER_HTTP: &str = "provider_http";
pub const PROVIDER_NETWORK: &str = "provider_network";
pub const PROVIDER_BAD_RESPONSE: &str = "provider_bad_response";
pub const AGENT_BAD_OUTPUT: &str = "agent_bad_output";
// 代码式 CAD(pp_cad::CadError::code)
pub const CAD_ENGINE_MISSING: &str = "cad_engine_missing";
pub const CAD_ENGINE_FAILED: &str = "cad_engine_failed";
pub const CAD_TIMEOUT: &str = "cad_timeout";
pub const CAD_SCRIPT_ERROR: &str = "cad_script_error";
pub const IMAGE_UNREADABLE: &str = "image_unreadable";

/// 拼出 `#code#detail`。
pub fn err(code: &str, detail: impl std::fmt::Display) -> String {
    format!("#{code}#{detail}")
}

/// 拆出 `(code, detail)`;不是约定格式则返回 `None`。
pub fn split(raw: &str) -> Option<(&str, &str)> {
    raw.strip_prefix('#')?.split_once('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let e = err(NOT_FOUND, "project 42");
        assert_eq!(e, "#not_found#project 42");
        assert_eq!(split(&e), Some((NOT_FOUND, "project 42")));
    }

    #[test]
    fn plain_string_is_not_a_code() {
        assert_eq!(split("boom"), None);
        assert_eq!(split("#only_one_hash"), None);
    }

    #[test]
    fn detail_may_contain_hash() {
        assert_eq!(split("#io_failed#C:\\a#b"), Some((IO_FAILED, "C:\\a#b")));
    }
}
