//! 网格分析结果:后端 pp-geometry 产出,前端 3D 工作室展示。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshReport {
    pub triangles: u32,
    pub vertices: u32,
    /// 包围盒(毫米)
    pub bbox_min: [f64; 3],
    pub bbox_max: [f64; 3],
    /// 体积(立方毫米)。网格不封闭时这个数不可信,界面要同时看 `watertight`。
    pub volume_mm3: f64,
    /// 表面积(平方毫米)
    pub area_mm2: f64,
    /// 连通块数量:>1 说明有悬浮碎片或多个零件
    pub components: u32,
    /// 只被一个三角形使用的边 = 孔洞边界
    pub boundary_edges: u32,
    /// 被三个及以上三角形共用的边
    pub non_manifold_edges: u32,
    /// 相邻三角形绕向不一致的边(法线翻转)
    pub flipped_edges: u32,
    pub degenerate_triangles: u32,
}

impl MeshReport {
    pub fn size(&self) -> [f64; 3] {
        [
            self.bbox_max[0] - self.bbox_min[0],
            self.bbox_max[1] - self.bbox_min[1],
            self.bbox_max[2] - self.bbox_min[2],
        ]
    }

    /// 水密:没有孔洞边界。
    pub fn watertight(&self) -> bool {
        self.triangles > 0 && self.boundary_edges == 0
    }

    /// 流形且绕向一致:切片软件能直接切、布尔运算稳定的前提。
    pub fn manifold(&self) -> bool {
        self.watertight() && self.non_manifold_edges == 0 && self.flipped_edges == 0
    }

    pub fn volume_cm3(&self) -> f64 {
        self.volume_mm3 / 1000.0
    }

    /// 是否放得进给定的成型空间(毫米)。
    pub fn fits(&self, build: [f64; 3]) -> bool {
        let s = self.size();
        s[0] <= build[0] && s[1] <= build[1] && s[2] <= build[2]
    }
}

/// 体积法打印估算(docs/03-architecture.md §8.2):没有切片软件时的兜底。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PrintEstimateParams {
    /// 外壁总厚度(毫米),典型 2 圈 × 0.42 ≈ 0.84
    pub wall_mm: f64,
    /// 填充率 0~1
    pub infill: f64,
    /// 材料密度 g/cm³(PLA ≈ 1.24)
    pub density: f64,
    /// 每小时出料克数——用用户自己的打印记录校准
    pub grams_per_hour: f64,
}

impl Default for PrintEstimateParams {
    fn default() -> Self {
        Self {
            wall_mm: 0.84,
            infill: 0.15,
            density: 1.24,
            grams_per_hour: 28.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PrintEstimate {
    pub grams: f64,
    pub hours: f64,
}

/// 一次网格操作(导入 / 缩放 / 切平底…)的结果:新资产 + 它的分析报告 + 耗时。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshOpResult {
    pub asset: crate::asset::Asset,
    pub report: MeshReport,
    pub elapsed_ms: u64,
}

impl MeshReport {
    /// `克数 ≈ (壳体积 + 内部体积 × 填充率) × 密度`,壳体积 ≈ 表面积 × 壁厚(不超过总体积)。
    pub fn estimate(&self, p: PrintEstimateParams) -> PrintEstimate {
        let total = self.volume_mm3.max(0.0);
        let shell = (self.area_mm2 * p.wall_mm).min(total);
        let inner = total - shell;
        let solid_mm3 = shell + inner * p.infill.clamp(0.0, 1.0);
        let grams = solid_mm3 / 1000.0 * p.density;
        let hours = if p.grams_per_hour > 0.0 {
            grams / p.grams_per_hour
        } else {
            0.0
        };
        PrintEstimate { grams, hours }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(side: f64) -> MeshReport {
        MeshReport {
            triangles: 12,
            vertices: 8,
            bbox_min: [0.0; 3],
            bbox_max: [side; 3],
            volume_mm3: side * side * side,
            area_mm2: 6.0 * side * side,
            components: 1,
            ..Default::default()
        }
    }

    #[test]
    fn solid_infill_weighs_volume_times_density() {
        let r = cube(20.0); // 8 cm³
        let e = r.estimate(PrintEstimateParams {
            infill: 1.0,
            ..Default::default()
        });
        assert!((e.grams - 8.0 * 1.24).abs() < 1e-9);
    }

    #[test]
    fn sparse_infill_is_lighter_but_never_below_the_shell() {
        let r = cube(40.0);
        let p = PrintEstimateParams::default();
        let e = r.estimate(p);
        let solid = r.volume_cm3() * p.density;
        let shell_only = r.area_mm2 * p.wall_mm / 1000.0 * p.density;
        assert!(e.grams < solid);
        assert!(e.grams > shell_only);
    }

    #[test]
    fn tiny_part_is_all_shell() {
        // 2mm 方块:表面积×壁厚 > 体积,壳体积要被体积封顶,不能估出比实心还重
        let r = cube(2.0);
        let e = r.estimate(PrintEstimateParams::default());
        assert!((e.grams - r.volume_cm3() * 1.24).abs() < 1e-9);
    }

    #[test]
    fn fits_checks_each_axis() {
        let r = cube(100.0);
        assert!(r.fits([256.0, 256.0, 256.0]));
        assert!(!r.fits([256.0, 90.0, 256.0]));
    }

    #[test]
    fn open_mesh_is_not_watertight() {
        let mut r = cube(10.0);
        assert!(r.manifold());
        r.boundary_edges = 4;
        assert!(!r.watertight());
        assert!(!r.manifold());
    }
}
