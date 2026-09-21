//! 每个业务域一个文件。约定(照搬 velo):
//! - `#[tauri::command(rename_all = "snake_case")]`,命令名与前端 `src/ipc.rs` 的 `cmd` 常量表一一对应;
//! - 一律返回 `Result<T, String>`,错误串为 `#错误码#细节`(pp_common::errcode);
//! - 耗时操作(网格、文件、网络)放进 `spawn_blocking` 或异步任务,不占住命令线程。

pub mod asset;
pub mod cad;
pub mod design;
pub mod mesh;
pub mod project;
pub mod provider;
pub mod research;
pub mod system;

use pp_common::errcode;

/// `spawn_blocking` 的 JoinError(任务 panic)→ 约定的错误串。
pub(crate) fn join_err(e: impl std::fmt::Display) -> String {
    errcode::err(errcode::IO_FAILED, format!("background task failed: {e}"))
}
