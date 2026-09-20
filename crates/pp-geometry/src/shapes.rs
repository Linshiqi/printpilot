//! 基本形体生成器。两个用途:
//! 1. 几何回归测试的固定样例(体积有解析解,能验证度量与切割);
//! 2. 演示模式 / 预研页面的样例模型(不花钱就能把 3D 管线走一遍)。
//!
//! 全部是封闭、共享顶点、绕向朝外的索引网格。

use std::collections::HashMap;
use std::f64::consts::PI;

use crate::mesh::{Mesh, Vec3};

/// 长方体,最小角在原点。
pub fn cuboid(size: Vec3) -> Mesh {
    let [sx, sy, sz] = size;
    // 顶点编号 = x + 2y + 4z(x,y,z ∈ {0,1})
    let positions = (0..8)
        .map(|i| {
            [
                if i & 1 != 0 { sx } else { 0.0 },
                if i & 2 != 0 { sy } else { 0.0 },
                if i & 4 != 0 { sz } else { 0.0 },
            ]
        })
        .collect();
    // 每个面 4 个角,从外面看逆时针
    let quads: [[u32; 4]; 6] = [
        [0, 2, 3, 1], // -Z
        [4, 5, 7, 6], // +Z
        [0, 1, 5, 4], // -Y
        [2, 6, 7, 3], // +Y
        [0, 4, 6, 2], // -X
        [1, 3, 7, 5], // +X
    ];
    let triangles = quads
        .iter()
        .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
        .collect();
    Mesh {
        positions,
        triangles,
    }
}

/// 二十面体细分球,球心在原点。`subdivisions` 每加 1 面数 ×4:0→20,3→1280,6→81920。
pub fn icosphere(radius: f64, subdivisions: u32) -> Mesh {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut positions: Vec<Vec3> = vec![
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ];
    let mut triangles: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];

    for _ in 0..subdivisions {
        let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
        let mut midpoint = |a: u32, b: u32, positions: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *mid.entry(key).or_insert_with(|| {
                let (p, q) = (positions[a as usize], positions[b as usize]);
                positions.push([(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0]);
                (positions.len() - 1) as u32
            })
        };
        let mut next = Vec::with_capacity(triangles.len() * 4);
        for [a, b, c] in triangles {
            let ab = midpoint(a, b, &mut positions);
            let bc = midpoint(b, c, &mut positions);
            let ca = midpoint(c, a, &mut positions);
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        triangles = next;
    }

    for p in &mut positions {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        for v in p.iter_mut() {
            *v *= radius / len;
        }
    }
    Mesh {
        positions,
        triangles,
    }
}

/// 环面,轴线 = Z 轴,中心在原点。`major` 是环的半径,`minor` 是管的半径。
/// 亏格为 1——水平切开得到「带孔的环形截面」,竖直切开得到两个分离的圆,是封盖算法的好样例。
pub fn torus(major: f64, minor: f64, seg_major: u32, seg_minor: u32) -> Mesh {
    let (nu, nv) = (seg_major.max(3), seg_minor.max(3));
    let mut positions = Vec::with_capacity((nu * nv) as usize);
    for i in 0..nu {
        let u = 2.0 * PI * i as f64 / nu as f64;
        for j in 0..nv {
            let v = 2.0 * PI * j as f64 / nv as f64;
            let ring = major + minor * v.cos();
            positions.push([ring * u.cos(), ring * u.sin(), minor * v.sin()]);
        }
    }
    let at = |i: u32, j: u32| (i % nu) * nv + (j % nv);
    let mut triangles = Vec::with_capacity((nu * nv * 2) as usize);
    for i in 0..nu {
        for j in 0..nv {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
    }
    Mesh {
        positions,
        triangles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;

    #[test]
    fn cuboid_is_a_closed_outward_box_with_exact_volume() {
        let m = cuboid([2.0, 3.0, 4.0]);
        let r = analyze(&m);
        assert!(r.manifold(), "{r:?}");
        assert_eq!((r.triangles, r.vertices, r.components), (12, 8, 1));
        assert!((m.signed_volume() - 24.0).abs() < 1e-12, "绕向朝外 → 带符号体积为正");
        assert!((r.area_mm2 - 52.0).abs() < 1e-12);
    }

    #[test]
    fn icosphere_converges_to_the_analytic_sphere() {
        let r = 10.0;
        let m = icosphere(r, 4);
        let rep = analyze(&m);
        assert!(rep.manifold(), "{rep:?}");
        assert_eq!(rep.triangles, 20 * 4u32.pow(4));
        let (v, a) = (4.0 / 3.0 * PI * r * r * r, 4.0 * PI * r * r);
        assert!(m.signed_volume() > 0.0);
        assert!((rep.volume_mm3 - v).abs() / v < 0.01);
        assert!((rep.area_mm2 - a).abs() / a < 0.01);
    }

    #[test]
    fn torus_matches_pappus() {
        let (big, small) = (20.0, 5.0);
        let m = torus(big, small, 96, 48);
        let rep = analyze(&m);
        assert!(rep.manifold(), "{rep:?}");
        assert!(m.signed_volume() > 0.0);
        let v = 2.0 * PI * PI * big * small * small;
        assert!((rep.volume_mm3 - v).abs() / v < 0.01);
    }
}
