//! controller:每个业务域一个 `Copy` 结构体(做法照搬 velo)。
//! view 只读信号、只调这里的方法;方法内 `spawn_local` → `ipc::call` → 写回 `AppState` 的信号。

pub mod project;

pub use project::{ForcedMove, ProjectController};
