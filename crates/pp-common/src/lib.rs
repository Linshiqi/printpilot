//! 前后端共享的类型。前端(wasm)与后端都依赖这个 crate,所以:
//! - 只放 DTO、枚举、错误码和纯函数;
//! - 不放任何 IO、时间源、重依赖。

pub mod asset;
pub mod cad;
pub mod cost;
pub mod design;
pub mod errcode;
pub mod gate;
pub mod imagery;
pub mod mesh;
pub mod project;
pub mod provider;
pub mod research;
pub mod stage;

pub use asset::*;
pub use project::*;
pub use stage::*;
