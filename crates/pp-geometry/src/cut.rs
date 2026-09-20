//! 切平底:用水平面 `z = z0` 切掉下面的部分,并把切口**封上**,结果仍然水密。
//!
//! AI 生成的模型很少有一个能贴床的平底;切一刀是最常用的补救。做法:
//! 1. 逐三角形裁剪。跨过平面的边上求交点,**按边缓存**——相邻两个三角形共用同一个新顶点,
//!    这是裁剪后侧面之间不漏缝的前提;
//! 2. 收集落在平面上的轮廓边(方向相反的成对边互相抵消:表面只是从上方碰到平面、并没有被切开);
//! 3. 用约束 Delaunay 三角剖分给轮廓封盖,按奇偶规则判定内外——带孔截面(环面)、
//!    多个分离截面、孔里的岛都自然成立,不需要自己分辨谁是外圈谁是孔;
//! 4. 盖子的三角形朝下(−Z),与侧面的绕向衔接。

use std::collections::{BTreeMap, HashMap, VecDeque};

use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use crate::mesh::Mesh;

#[derive(Debug, Clone, PartialEq)]
pub enum CutError {
    /// 平面以上什么都没有(切得太高)
    NothingAbove,
    /// 切口轮廓没有闭合:网格在切面附近有孔洞或非流形边。先修复,或改用生成时的「平底」参数
    OpenContour { loose_ends: usize },
    /// 切口轮廓自相交(网格自交)或坐标非法,无法封盖
    Triangulation(String),
}

impl std::fmt::Display for CutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CutError::NothingAbove => write!(f, "nothing above the cut plane"),
            CutError::OpenContour { loose_ends } => {
                write!(f, "cut contour is not closed ({loose_ends} loose ends)")
            }
            CutError::Triangulation(why) => write!(f, "cannot cap the cut: {why}"),
        }
    }
}

impl std::error::Error for CutError {}

/// 平面上的有向轮廓边(多重集)。加入 `u→v` 时若已有 `v→u`,两者抵消。
#[derive(Default)]
struct Rim {
    edges: BTreeMap<(u32, u32), u32>,
}

impl Rim {
    fn add(&mut self, u: u32, v: u32) {
        if u == v {
            return;
        }
        if let Some(n) = self.edges.get_mut(&(v, u)) {
            *n -= 1;
            if *n == 0 {
                self.edges.remove(&(v, u));
            }
        } else {
            *self.edges.entry((u, v)).or_insert(0) += 1;
        }
    }
}

struct Builder<'a> {
    src: &'a Mesh,
    /// 到平面的带符号距离;容差内的记为 0(视为正好在平面上)
    dist: Vec<f64>,
    z0: f64,
    out: Mesh,
    kept: Vec<u32>,
    crossing: HashMap<u64, u32>,
}

impl Builder<'_> {
    fn keep(&mut self, i: u32) -> u32 {
        let slot = self.kept[i as usize];
        if slot != u32::MAX {
            return slot;
        }
        let mut p = self.src.positions[i as usize];
        if self.dist[i as usize] == 0.0 {
            p[2] = self.z0; // 吸附到平面上,盖子才是真的平
        }
        self.out.positions.push(p);
        let idx = (self.out.positions.len() - 1) as u32;
        self.kept[i as usize] = idx;
        idx
    }

    /// 边 i–j 与平面的交点。按「小号→大号」的固定方向计算并缓存,相邻三角形拿到的是同一个顶点。
    fn cross(&mut self, i: u32, j: u32) -> u32 {
        let (lo, hi) = (i.min(j), i.max(j));
        let key = ((lo as u64) << 32) | hi as u64;
        if let Some(idx) = self.crossing.get(&key) {
            return *idx;
        }
        let (a, b) = (self.src.positions[lo as usize], self.src.positions[hi as usize]);
        let (da, db) = (self.dist[lo as usize], self.dist[hi as usize]);
        let t = da / (da - db);
        self.out
            .positions
            .push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), self.z0]);
        let idx = (self.out.positions.len() - 1) as u32;
        self.crossing.insert(key, idx);
        idx
    }
}

/// 保留 `z ≥ z0` 的部分并封底。输入应当已经 `weld` 过、封闭且绕向朝外。
pub fn cut_below(mesh: &Mesh, z0: f64) -> Result<Mesh, CutError> {
    let eps = mesh.diagonal() * 1e-9;
    let dist: Vec<f64> = mesh
        .positions
        .iter()
        .map(|p| {
            let d = p[2] - z0;
            if d.abs() <= eps {
                0.0
            } else {
                d
            }
        })
        .collect();

    let mut b = Builder {
        src: mesh,
        dist,
        z0,
        out: Mesh::default(),
        kept: vec![u32::MAX; mesh.positions.len()],
        crossing: HashMap::new(),
    };
    let mut rim = Rim::default();

    for t in &mesh.triangles {
        let d = [b.dist[t[0] as usize], b.dist[t[1] as usize], b.dist[t[2] as usize]];
        let above = d.iter().filter(|v| **v > 0.0).count();
        let below = d.iter().filter(|v| **v < 0.0).count();
        if above == 0 {
            continue; // 整个在平面上或平面下:丢弃(贴在平面上的面由盖子重新生成)
        }

        // 裁剪后的多边形(3 或 4 个点)及每个点是否在平面上
        let mut poly: Vec<(u32, bool)> = Vec::with_capacity(4);
        for k in 0..3 {
            let (i, j) = (t[k], t[(k + 1) % 3]);
            let (di, dj) = (d[k], d[(k + 1) % 3]);
            if di >= 0.0 {
                poly.push((b.keep(i), di == 0.0));
            }
            if below > 0 && di * dj < 0.0 {
                poly.push((b.cross(i, j), true));
            }
        }

        for k in 1..poly.len() - 1 {
            b.out.triangles.push([poly[0].0, poly[k].0, poly[k + 1].0]);
        }
        for k in 0..poly.len() {
            let (u, v) = (poly[k], poly[(k + 1) % poly.len()]);
            if u.1 && v.1 {
                rim.add(u.0, v.0);
            }
        }
    }

    if b.out.triangles.is_empty() {
        return Err(CutError::NothingAbove);
    }
    let mut out = b.out;
    if rim.edges.is_empty() {
        return Ok(out); // 平面在模型下方:没有切到任何东西
    }

    // 闭合检查:封闭表面被切开后,轮廓上每个顶点的出边数 = 入边数
    let mut balance: BTreeMap<u32, i64> = BTreeMap::new();
    for ((u, v), n) in &rim.edges {
        *balance.entry(*u).or_insert(0) += *n as i64;
        *balance.entry(*v).or_insert(0) -= *n as i64;
    }
    let loose_ends = balance.values().filter(|v| **v != 0).count();
    if loose_ends > 0 {
        return Err(CutError::OpenContour { loose_ends });
    }

    cap(&mut out, &rim)?;

    // 两个轮廓在一点相接时,那个位置会有两个坐标相同的顶点;焊掉,顺带丢弃没用到的点
    out.weld_auto();
    out.compact();
    Ok(out)
}

fn cap(out: &mut Mesh, rim: &Rim) -> Result<(), CutError> {
    let mut cdt = ConstrainedDelaunayTriangulation::<Point2<f64>>::new();
    // spade 顶点序号 → 网格顶点序号
    let mut mesh_index: Vec<u32> = Vec::new();
    let mut handle_of = HashMap::new();

    for (u, v) in rim.edges.keys() {
        for m in [*u, *v] {
            if handle_of.contains_key(&m) {
                continue;
            }
            let p = out.positions[m as usize];
            let h = cdt
                .insert(Point2::new(p[0], p[1]))
                .map_err(|e| CutError::Triangulation(format!("{e:?}")))?;
            if h.index() >= mesh_index.len() {
                mesh_index.resize(h.index() + 1, m);
            }
            handle_of.insert(m, h);
        }
    }
    for (u, v) in rim.edges.keys() {
        let (hu, hv) = (handle_of[u], handle_of[v]);
        if hu == hv {
            continue; // 两个网格顶点 XY 重合(稍后由 weld 合并)
        }
        if !cdt.can_add_constraint(hu, hv) {
            return Err(CutError::Triangulation("contour intersects itself".into()));
        }
        cdt.add_constraint(hu, hv);
    }

    // 奇偶填充:从凸包外面出发,每跨过一条轮廓边,内外翻转一次
    let mut inside: Vec<Option<bool>> = vec![None; cdt.num_inner_faces() + 1];
    let mut queue = VecDeque::new();
    for edge in cdt.convex_hull() {
        let crossed = edge.is_constraint_edge();
        for side in [edge, edge.rev()] {
            if let Some(face) = side.face().as_inner() {
                let idx = face.fix().index();
                if inside[idx].is_none() {
                    inside[idx] = Some(crossed);
                    queue.push_back(face.fix());
                }
            }
        }
    }
    while let Some(fixed) = queue.pop_front() {
        let here = inside[fixed.index()].unwrap_or(false);
        for edge in cdt.face(fixed).adjacent_edges() {
            let Some(next) = edge.rev().face().as_inner() else {
                continue;
            };
            let idx = next.fix().index();
            if inside[idx].is_none() {
                inside[idx] = Some(here ^ edge.is_constraint_edge());
                queue.push_back(next.fix());
            }
        }
    }

    for face in cdt.inner_faces() {
        if inside[face.fix().index()] != Some(true) {
            continue;
        }
        let v = face.vertices().map(|h| mesh_index[h.fix().index()]);
        // spade 的面在 XY 平面里是逆时针(法线 +Z);底盖要朝下,换一下绕向
        out.triangles.push([v[0], v[2], v[1]]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, shapes};
    use std::f64::consts::PI;

    fn assert_solid(m: &Mesh) {
        let r = analyze(m);
        assert!(r.manifold(), "切完应当仍是封闭流形: {r:?}");
        assert!(m.signed_volume() > 0.0, "绕向应当朝外");
    }

    #[test]
    fn cube_cut_keeps_the_upper_part_with_exact_volume() {
        // 侧面的对角线也会被切到:每条侧边上有 3 个共线交点——丢共线点的剖分器过不了这一关
        let m = shapes::cuboid([10.0, 10.0, 10.0]);
        let cut = cut_below(&m, 4.0).unwrap();
        assert_solid(&cut);
        assert!((cut.signed_volume() - 600.0).abs() < 1e-9);
        let (lo, hi) = cut.bbox();
        assert_eq!((lo[2], hi[2]), (4.0, 10.0));
    }

    #[test]
    fn sphere_cut_through_existing_vertices_gives_a_hemisphere() {
        // 二十面体有顶点正好落在 z = 0 上:走的是「顶点在平面上」的分支,而不是求交点
        let m = shapes::icosphere(10.0, 4);
        let full = m.signed_volume();
        let cut = cut_below(&m, 0.0).unwrap();
        assert_solid(&cut);
        assert!((cut.signed_volume() - full / 2.0).abs() / full < 1e-9);
        assert_eq!(cut.bbox().0[2], 0.0);
    }

    #[test]
    fn sphere_cap_volume_matches_the_formula() {
        let r = 10.0;
        let m = shapes::icosphere(r, 5);
        let cut = cut_below(&m, 6.0).unwrap();
        assert_solid(&cut);
        let h = r - 6.0;
        let cap = PI * h * h * (3.0 * r - h) / 3.0;
        assert!((cut.signed_volume() - cap).abs() / cap < 0.01);
    }

    #[test]
    fn flat_torus_cut_leaves_a_ring_shaped_cap_with_a_hole() {
        let m = shapes::torus(20.0, 5.0, 64, 32);
        let full = m.signed_volume();
        for z in [0.0, 2.0] {
            let cut = cut_below(&m, z).unwrap();
            assert_solid(&cut);
            if z == 0.0 {
                assert!((cut.signed_volume() - full / 2.0).abs() / full < 1e-9);
            }
            // 盖子不能把中间的孔也填上:环心正上方不应该有任何盖子三角形
            let covers_center = cut.triangles.iter().any(|t| {
                let (a, b, c) = cut.tri(*t);
                a[2] == z && b[2] == z && c[2] == z && point_in_tri([0.0, 0.0], a, b, c)
            });
            assert!(!covers_center, "z = {z}: 孔被盖住了");
        }
    }

    #[test]
    fn standing_torus_cut_leaves_two_separate_caps() {
        let mut m = shapes::torus(20.0, 5.0, 64, 32);
        m.orient_normal_down([1.0, 0.0, 0.0]); // 让环立起来
        let full = m.signed_volume();
        let cut = cut_below(&m, 0.0).unwrap();
        assert_solid(&cut);
        assert!((cut.signed_volume() - full / 2.0).abs() / full < 1e-6);
        assert_eq!(analyze(&cut).components, 1, "上半个环仍然是一整块");
    }

    #[test]
    fn two_bodies_are_both_capped() {
        let mut m = shapes::cuboid([10.0, 10.0, 10.0]);
        let mut other = shapes::cuboid([4.0, 4.0, 20.0]);
        other.translate([30.0, 0.0, 0.0]);
        let offset = m.positions.len() as u32;
        m.positions.extend(other.positions);
        m.triangles
            .extend(other.triangles.iter().map(|t| t.map(|v| v + offset)));
        let cut = cut_below(&m, 5.0).unwrap();
        assert_solid(&cut);
        assert!((cut.signed_volume() - (500.0 + 4.0 * 4.0 * 15.0)).abs() < 1e-9);
        assert_eq!(analyze(&cut).components, 2);
    }

    #[test]
    fn plane_below_the_model_changes_nothing() {
        let m = shapes::icosphere(10.0, 2);
        let cut = cut_below(&m, -50.0).unwrap();
        assert_eq!(cut.triangles.len(), m.triangles.len());
        assert!((cut.signed_volume() - m.signed_volume()).abs() < 1e-9);
    }

    #[test]
    fn plane_touching_the_bottom_face_changes_nothing_either() {
        // 平面正好贴着立方体底面:底面三角形全在平面上,会被丢弃再由盖子重建
        let m = shapes::cuboid([10.0, 10.0, 10.0]);
        let cut = cut_below(&m, 0.0).unwrap();
        assert_solid(&cut);
        assert!((cut.signed_volume() - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn plane_above_the_model_is_an_error() {
        let m = shapes::cuboid([10.0, 10.0, 10.0]);
        assert_eq!(cut_below(&m, 10.0), Err(CutError::NothingAbove));
        assert_eq!(cut_below(&m, 99.0), Err(CutError::NothingAbove));
    }

    #[test]
    fn open_mesh_is_refused_instead_of_producing_a_leaky_result() {
        let mut m = shapes::cuboid([10.0, 10.0, 10.0]);
        // 去掉 +X 侧面,切面会穿过这个缺口
        m.triangles
            .retain(|t| !t.iter().all(|v| m.positions[*v as usize][0] == 10.0));
        assert!(matches!(
            cut_below(&m, 5.0),
            Err(CutError::OpenContour { loose_ends: 2 })
        ));
    }

    fn point_in_tri(p: [f64; 2], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> bool {
        let side = |u: [f64; 3], v: [f64; 3]| (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0]);
        let (s1, s2, s3) = (side(a, b), side(b, c), side(c, a));
        (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
    }
}
