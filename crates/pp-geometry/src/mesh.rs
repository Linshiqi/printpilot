use std::collections::HashMap;

pub type Vec3 = [f64; 3];

/// 索引三角网格。位置用 f64:STL/GLB 读进来是 f32,但切割时的交点与体积累加用 f64 才稳。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// 包围盒 `(min, max)`;空网格返回两个零点。
    pub fn bbox(&self) -> (Vec3, Vec3) {
        let mut it = self.positions.iter();
        let Some(first) = it.next() else {
            return ([0.0; 3], [0.0; 3]);
        };
        let (mut lo, mut hi) = (*first, *first);
        for p in it {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    pub fn size(&self) -> Vec3 {
        let (lo, hi) = self.bbox();
        sub(hi, lo)
    }

    pub fn diagonal(&self) -> f64 {
        norm(self.size())
    }

    pub fn translate(&mut self, t: Vec3) {
        for p in &mut self.positions {
            for k in 0..3 {
                p[k] += t[k];
            }
        }
    }

    pub fn scale(&mut self, s: f64) {
        for p in &mut self.positions {
            for v in p.iter_mut() {
                *v *= s;
            }
        }
        if s < 0.0 {
            self.flip_winding();
        }
    }

    /// 等比缩放到「最长边 = `mm`」。AI 生成的模型没有单位,这是给它定尺寸的入口。
    /// 返回实际用的缩放系数;网格没有尺寸时不动并返回 1。
    pub fn scale_to_longest(&mut self, mm: f64) -> f64 {
        let s = self.size();
        let longest = s[0].max(s[1]).max(s[2]);
        if longest <= 0.0 || mm <= 0.0 {
            return 1.0;
        }
        let k = mm / longest;
        self.scale(k);
        k
    }

    /// 行主序 3×3 矩阵。行列式为负(含镜像)时自动翻转绕向,保持外表面朝外。
    pub fn transform3(&mut self, m: [[f64; 3]; 3]) {
        for p in &mut self.positions {
            let v = *p;
            for k in 0..3 {
                p[k] = m[k][0] * v[0] + m[k][1] * v[1] + m[k][2] * v[2];
            }
        }
        if dot(m[0], cross(m[1], m[2])) < 0.0 {
            self.flip_winding();
        }
    }

    /// glTF 是 Y 朝上,打印是 Z 朝上:`(x, y, z) → (x, -z, y)`,即绕 X 轴转 +90°。
    pub fn y_up_to_z_up(&mut self) {
        self.transform3([[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]]);
    }

    /// 镜像(沿 X)。绕向同步翻转。
    pub fn mirror_x(&mut self) {
        self.transform3([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    /// 旋转网格,让给定的面法线朝下(−Z)——「以此面贴床」。
    pub fn orient_normal_down(&mut self, normal: Vec3) {
        let len = norm(normal);
        if len <= 0.0 {
            return;
        }
        let n = [normal[0] / len, normal[1] / len, normal[2] / len];
        let target = [0.0, 0.0, -1.0];
        let c = dot(n, target);
        if c > 1.0 - 1e-12 {
            return; // 已经朝下
        }
        if c < -1.0 + 1e-12 {
            // 正好朝上:绕 X 轴转 180°(此时旋转轴不唯一,任取一个)
            self.transform3([[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]]);
            return;
        }
        // Rodrigues:R = I + [v]× + [v]×² / (1 + c),v = n × target
        let v = cross(n, target);
        let k = 1.0 / (1.0 + c);
        let m = [
            [
                1.0 - k * (v[1] * v[1] + v[2] * v[2]),
                -v[2] + k * v[0] * v[1],
                v[1] + k * v[0] * v[2],
            ],
            [
                v[2] + k * v[0] * v[1],
                1.0 - k * (v[0] * v[0] + v[2] * v[2]),
                -v[0] + k * v[1] * v[2],
            ],
            [
                -v[1] + k * v[0] * v[2],
                v[0] + k * v[1] * v[2],
                1.0 - k * (v[0] * v[0] + v[1] * v[1]),
            ],
        ];
        self.transform3(m);
    }

    /// 贴床:最低点落到 z = 0,XY 方向以包围盒中心对准原点。
    pub fn place_on_bed(&mut self) {
        let (lo, hi) = self.bbox();
        self.translate([-(lo[0] + hi[0]) / 2.0, -(lo[1] + hi[1]) / 2.0, -lo[2]]);
    }

    pub fn flip_winding(&mut self) {
        for t in &mut self.triangles {
            t.swap(1, 2);
        }
    }

    /// 焊接重合顶点 + 丢弃退化三角形。
    ///
    /// STL 没有共享顶点(每个三角形自带三个点),GLB 会在法线/UV 接缝处拆点——不先焊接,
    /// 每条边都只属于一个三角形,水密/流形分析就毫无意义。`eps` 是合并容差(毫米),
    /// 同一来源的重合点坐标逐位相同,用很小的容差即可。
    pub fn weld(&mut self, eps: f64) {
        let eps = eps.max(f64::MIN_POSITIVE);
        let key = |p: &Vec3| {
            (
                (p[0] / eps).round() as i64,
                (p[1] / eps).round() as i64,
                (p[2] / eps).round() as i64,
            )
        };
        let mut seen: HashMap<(i64, i64, i64), u32> = HashMap::with_capacity(self.positions.len());
        let mut remap: Vec<u32> = Vec::with_capacity(self.positions.len());
        let mut kept: Vec<Vec3> = Vec::new();
        for p in &self.positions {
            let idx = *seen.entry(key(p)).or_insert_with(|| {
                kept.push(*p);
                (kept.len() - 1) as u32
            });
            remap.push(idx);
        }
        self.positions = kept;
        self.triangles = self
            .triangles
            .iter()
            .map(|t| [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]])
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .collect();
    }

    /// 适合这张网格的焊接容差:包围盒对角线的千万分之一。
    pub fn weld_auto(&mut self) {
        let eps = (self.diagonal() * 1e-7).max(1e-9);
        self.weld(eps);
    }

    /// 丢掉没有被任何三角形引用的顶点(切割、去碎片之后用)。
    pub fn compact(&mut self) {
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut kept = Vec::new();
        for t in &mut self.triangles {
            for v in t.iter_mut() {
                let old = *v as usize;
                if remap[old] == u32::MAX {
                    remap[old] = kept.len() as u32;
                    kept.push(self.positions[old]);
                }
                *v = remap[old];
            }
        }
        self.positions = kept;
    }

    pub(crate) fn tri(&self, t: [u32; 3]) -> (Vec3, Vec3, Vec3) {
        (
            self.positions[t[0] as usize],
            self.positions[t[1] as usize],
            self.positions[t[2] as usize],
        )
    }

    /// 带符号体积:绕向朝外时为正。
    pub fn signed_volume(&self) -> f64 {
        self.triangles
            .iter()
            .map(|t| {
                let (a, b, c) = self.tri(*t);
                dot(a, cross(b, c))
            })
            .sum::<f64>()
            / 6.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, shapes};

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn scale_to_longest_sets_the_longest_edge() {
        let mut m = shapes::cuboid([2.0, 5.0, 1.0]);
        let k = m.scale_to_longest(60.0);
        assert!(close(k, 12.0, 1e-12));
        let s = m.size();
        assert!(close(s[0], 24.0, 1e-9) && close(s[1], 60.0, 1e-9) && close(s[2], 12.0, 1e-9));
    }

    #[test]
    fn scale_to_longest_ignores_degenerate_input() {
        let mut empty = Mesh::default();
        assert_eq!(empty.scale_to_longest(10.0), 1.0);
        let mut m = shapes::cuboid([1.0, 1.0, 1.0]);
        assert_eq!(m.scale_to_longest(0.0), 1.0);
        assert!(close(m.size()[0], 1.0, 1e-12));
    }

    #[test]
    fn place_on_bed_centers_xy_and_zeroes_min_z() {
        let mut m = shapes::cuboid([10.0, 20.0, 30.0]);
        m.translate([7.0, -3.0, 11.0]);
        m.place_on_bed();
        let (lo, hi) = m.bbox();
        assert!(close(lo[2], 0.0, 1e-9));
        assert!(close(lo[0] + hi[0], 0.0, 1e-9) && close(lo[1] + hi[1], 0.0, 1e-9));
    }

    #[test]
    fn mirror_keeps_the_surface_facing_outward() {
        let mut m = shapes::icosphere(10.0, 2);
        let before = m.signed_volume();
        m.mirror_x();
        assert!(close(m.signed_volume(), before, 1e-6), "镜像后体积应保持为正");
        assert_eq!(analyze(&m).flipped_edges, 0);
    }

    #[test]
    fn y_up_to_z_up_maps_axes_and_preserves_volume() {
        let mut m = shapes::cuboid([1.0, 2.0, 3.0]); // x=1, y=2(高), z=3
        let v = m.signed_volume();
        m.y_up_to_z_up();
        let s = m.size();
        assert!(close(s[0], 1.0, 1e-12) && close(s[1], 3.0, 1e-12) && close(s[2], 2.0, 1e-12));
        assert!(close(m.signed_volume(), v, 1e-9));
    }

    #[test]
    fn orient_normal_down_handles_general_and_antiparallel_normals() {
        // 一般方向:+X 面朝下后,原来 X 方向的尺寸变成高度
        let mut m = shapes::cuboid([4.0, 2.0, 1.0]);
        m.orient_normal_down([1.0, 0.0, 0.0]);
        assert!(close(m.size()[2], 4.0, 1e-9));
        assert!(m.signed_volume() > 0.0);

        // 正好朝上(旋转轴不唯一的特例)
        let mut up = shapes::cuboid([4.0, 2.0, 1.0]);
        up.translate([0.0, 0.0, 5.0]);
        let top_before = up.bbox().1[2];
        up.orient_normal_down([0.0, 0.0, 1.0]);
        assert!(close(up.bbox().0[2], -top_before, 1e-9), "顶面应翻到最下面");
        assert!(up.signed_volume() > 0.0);

        // 已经朝下:不动
        let mut down = shapes::cuboid([1.0, 1.0, 1.0]);
        let snapshot = down.clone();
        down.orient_normal_down([0.0, 0.0, -2.0]);
        assert_eq!(down, snapshot);
    }

    #[test]
    fn weld_merges_duplicate_vertices_and_drops_degenerates() {
        // 两个共用一条边的三角形,写成 STL 式的 6 个独立顶点
        let mut m = Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                // 再加一个三点重合的退化三角形
                [5.0, 5.0, 5.0],
                [5.0, 5.0, 5.0],
                [5.0, 5.0, 5.0],
            ],
            triangles: vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]],
        };
        m.weld(1e-9);
        assert_eq!(m.triangles.len(), 2);
        m.compact();
        assert_eq!(m.positions.len(), 4);
    }

    #[test]
    fn compact_drops_unreferenced_vertices() {
        let mut m = shapes::cuboid([1.0, 1.0, 1.0]);
        m.positions.push([9.0, 9.0, 9.0]);
        m.compact();
        assert_eq!(m.positions.len(), 8);
        assert!(close(m.signed_volume(), 1.0, 1e-12));
    }
}
