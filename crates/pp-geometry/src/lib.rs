//! 网格内核。
//!
//! 分工(docs/03-architecture.md §8):three.js 只做**预览**,真实的几何运算全部在这里——
//! 用户在 3D 工作室里点「应用」,后端用这个 crate 算出新网格、存成新的资产版本、重新分析。
//!
//! 约定:坐标单位 = 毫米;Z 轴朝上(与切片软件一致);三角形逆时针 = 外表面。

mod analyze;
mod cut;
pub mod io;
mod mesh;
pub mod shapes;

pub use analyze::analyze;
pub use cut::{cut_below, CutError};
pub use mesh::{Mesh, Vec3};

pub use pp_common::mesh::{MeshReport, PrintEstimate, PrintEstimateParams};
