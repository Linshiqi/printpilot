use std::collections::HashMap;

use pp_common::mesh::MeshReport;

use crate::mesh::{cross, norm, sub, Mesh};

/// 度量一张网格。**调用前应先 `weld`**:没有共享顶点的网格(刚读进来的 STL)每条边都是
/// 「边界边」,水密性判断没有意义。
pub fn analyze(mesh: &Mesh) -> MeshReport {
    let (lo, hi) = mesh.bbox();
    let mut area = 0.0;
    let mut degenerate = 0u32;

    // 边 → (使用次数, 方向差)。方向差:沿 小→大 走记 +1,反向记 −1;
    // 两个绕向一致的三角形共用一条边时,一个正向一个反向,差为 0。
    let mut edges: HashMap<u64, (u32, i32)> = HashMap::with_capacity(mesh.triangles.len() * 3 / 2);
    let mut uf = UnionFind::new(mesh.positions.len());
    let mut used = vec![false; mesh.positions.len()];

    for t in &mesh.triangles {
        let (a, b, c) = mesh.tri(*t);
        let twice_area = norm(cross(sub(b, a), sub(c, a)));
        area += twice_area / 2.0;
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] || twice_area == 0.0 {
            degenerate += 1;
        }
        for k in 0..3 {
            let (i, j) = (t[k], t[(k + 1) % 3]);
            let key = ((i.min(j) as u64) << 32) | i.max(j) as u64;
            let e = edges.entry(key).or_insert((0, 0));
            e.0 += 1;
            e.1 += if i < j { 1 } else { -1 };
        }
        uf.union(t[0], t[1]);
        uf.union(t[1], t[2]);
        for v in t {
            used[*v as usize] = true;
        }
    }

    let (mut boundary, mut non_manifold, mut flipped) = (0u32, 0u32, 0u32);
    for (count, balance) in edges.values() {
        match count {
            1 => boundary += 1,
            2 if *balance != 0 => flipped += 1,
            2 => {}
            _ => non_manifold += 1,
        }
    }

    let mut roots = std::collections::HashSet::new();
    for (v, is_used) in used.iter().enumerate() {
        if *is_used {
            roots.insert(uf.find(v as u32));
        }
    }

    MeshReport {
        triangles: mesh.triangles.len() as u32,
        vertices: used.iter().filter(|u| **u).count() as u32,
        bbox_min: lo,
        bbox_max: hi,
        volume_mm3: mesh.signed_volume().abs(),
        area_mm2: area,
        components: roots.len() as u32,
        boundary_edges: boundary,
        non_manifold_edges: non_manifold,
        flipped_edges: flipped,
        degenerate_triangles: degenerate,
    }
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n as u32).collect(),
        }
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let grand = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = grand; // 路径减半
            x = grand;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb) as usize] = ra.min(rb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes;

    #[test]
    fn empty_mesh_reports_zeroes_and_is_not_watertight() {
        let r = analyze(&Mesh::default());
        assert_eq!(r.triangles, 0);
        assert!(!r.watertight(), "空网格不能算水密");
    }

    #[test]
    fn a_box_without_its_lid_has_a_four_edge_hole() {
        let mut m = shapes::cuboid([1.0, 1.0, 1.0]);
        m.triangles.retain(|t| {
            // 去掉 +Z 面:三个顶点的 z 都是 1
            !t.iter().all(|v| m.positions[*v as usize][2] == 1.0)
        });
        let r = analyze(&m);
        assert_eq!(r.boundary_edges, 4);
        assert!(!r.watertight());
        assert_eq!(r.non_manifold_edges, 0);
    }

    #[test]
    fn unwelded_stl_style_soup_looks_fully_open_until_welded() {
        let closed = shapes::cuboid([1.0, 1.0, 1.0]);
        let mut soup = Mesh::default();
        for t in &closed.triangles {
            let base = soup.positions.len() as u32;
            for v in t {
                soup.positions.push(closed.positions[*v as usize]);
            }
            soup.triangles.push([base, base + 1, base + 2]);
        }
        assert_eq!(analyze(&soup).boundary_edges, 36, "每个三角形三条边都没有邻居");
        soup.weld_auto();
        assert!(analyze(&soup).manifold());
    }

    #[test]
    fn one_inverted_triangle_shows_up_as_flipped_edges() {
        let mut m = shapes::cuboid([1.0, 1.0, 1.0]);
        m.triangles[0].swap(1, 2);
        let r = analyze(&m);
        assert_eq!(r.boundary_edges, 0);
        assert_eq!(r.flipped_edges, 3);
        assert!(r.watertight() && !r.manifold());
    }

    #[test]
    fn a_fin_sharing_an_edge_is_non_manifold() {
        let mut m = shapes::cuboid([1.0, 1.0, 1.0]);
        // 在 0-1 这条棱上再挂一片三角形:这条边被 3 个面共用
        m.positions.push([0.5, -1.0, -1.0]);
        let fin = (m.positions.len() - 1) as u32;
        m.triangles.push([0, 1, fin]);
        let r = analyze(&m);
        assert_eq!(r.non_manifold_edges, 1);
        assert!(!r.manifold());
    }

    #[test]
    fn floating_debris_counts_as_a_separate_component() {
        let mut m = shapes::icosphere(10.0, 1);
        let mut crumb = shapes::cuboid([0.2, 0.2, 0.2]);
        crumb.translate([30.0, 0.0, 0.0]);
        let offset = m.positions.len() as u32;
        m.positions.extend(crumb.positions);
        m.triangles
            .extend(crumb.triangles.iter().map(|t| t.map(|v| v + offset)));
        let r = analyze(&m);
        assert_eq!(r.components, 2);
        assert!(r.manifold(), "两个封闭体各自都是流形");
    }

    #[test]
    fn unreferenced_vertices_do_not_count() {
        let mut m = shapes::cuboid([1.0, 1.0, 1.0]);
        m.positions.push([100.0, 100.0, 100.0]);
        let r = analyze(&m);
        assert_eq!(r.vertices, 8);
        assert_eq!(r.components, 1);
    }
}
