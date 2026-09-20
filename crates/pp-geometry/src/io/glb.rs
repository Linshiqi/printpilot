use super::IoError;
use crate::mesh::Mesh;

/// 读 GLB(二进制 glTF):AI 3D 生成接口的通用输出格式。只取几何,忽略材质与贴图。
///
/// - 节点变换(缩放/旋转/平移)会被烘进顶点;
/// - glTF 是 Y 朝上,这里转成 Z 朝上;
/// - 不支持 Draco / meshopt 压缩(`gltf` crate 解析时会因为缺少必需扩展而报错)——
///   向供应商要未压缩的 GLB,或者直接要 STL。
pub fn read_glb(bytes: &[u8]) -> Result<Mesh, IoError> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| IoError::Malformed(e.to_string()))?;
    let blob = gltf.blob.as_deref();
    let mut mesh = Mesh::default();

    for scene in gltf.document.scenes() {
        for node in scene.nodes() {
            visit(&node, IDENTITY, blob, &mut mesh)?;
        }
    }
    mesh.y_up_to_z_up();
    Ok(mesh)
}

type Mat4 = [[f32; 4]; 4]; // 列主序,与 glTF 一致

const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn mul(a: Mat4, b: Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (col, out_col) in out.iter_mut().enumerate() {
        for (row, cell) in out_col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    out
}

fn apply(m: Mat4, p: [f32; 3]) -> [f64; 3] {
    let mut out = [0.0f64; 3];
    for (row, cell) in out.iter_mut().enumerate() {
        *cell = (m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row]) as f64;
    }
    out
}

fn determinant3(m: Mat4) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[2][1] * m[1][2]) - m[1][0] * (m[0][1] * m[2][2] - m[2][1] * m[0][2])
        + m[2][0] * (m[0][1] * m[1][2] - m[1][1] * m[0][2])
}

fn visit(node: &gltf::Node<'_>, parent: Mat4, blob: Option<&[u8]>, out: &mut Mesh) -> Result<(), IoError> {
    let world = mul(parent, node.transform().matrix());
    if let Some(m) = node.mesh() {
        for prim in m.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            // GLB 只有一个内嵌缓冲区(index 0);引用外部 .bin 的不是自包含 GLB,不支持
            let reader = prim.reader(|buffer| if buffer.index() == 0 { blob } else { None });
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let base = out.positions.len() as u32;
            out.positions.extend(positions.map(|p| apply(world, p)));
            let count = out.positions.len() as u32 - base;

            let indices: Vec<u32> = match reader.read_indices() {
                Some(idx) => idx.into_u32().collect(),
                None => (0..count).collect(),
            };
            if indices.iter().any(|i| *i >= count) {
                return Err(IoError::Malformed("index out of range".into()));
            }
            // 带镜像的节点变换会把绕向翻过来,这里翻回去,保证外表面朝外
            let mirrored = determinant3(world) < 0.0;
            for t in indices.chunks_exact(3) {
                let (a, b, c) = (base + t[0], base + t[1], base + t[2]);
                out.triangles.push(if mirrored { [a, c, b] } else { [a, b, c] });
            }
        }
    }
    for child in node.children() {
        visit(&child, world, blob, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, shapes};

    /// 手工拼一个最小的 GLB:一个网格 + 一个带变换的节点。
    fn glb_of(mesh: &Mesh, node_extra: &str) -> Vec<u8> {
        let mut bin = Vec::new();
        for p in &mesh.positions {
            for v in p {
                bin.extend_from_slice(&(*v as f32).to_le_bytes());
            }
        }
        let pos_len = bin.len();
        for t in &mesh.triangles {
            for v in t {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        let idx_len = bin.len() - pos_len;
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let (lo, hi) = mesh.bbox();
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[{{"mesh":0{node_extra}}}],
"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"mode":4}}]}}],
"buffers":[{{"byteLength":{total}}}],
"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{pos_len},"target":34962}},
               {{"buffer":0,"byteOffset":{pos_len},"byteLength":{idx_len},"target":34963}}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":{nv},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}},
             {{"bufferView":1,"componentType":5125,"count":{ni},"type":"SCALAR"}}]}}"#,
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
            total = bin.len(),
            nv = mesh.positions.len(),
            ni = mesh.triangles.len() * 3,
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn reads_geometry_and_converts_to_z_up() {
        // 在 glTF 坐标里:宽 1、**高 2(Y)**、深 3
        let src = shapes::cuboid([1.0, 2.0, 3.0]);
        let mut m = read_glb(&glb_of(&src, "")).unwrap();
        m.weld_auto();
        let s = m.size();
        assert!((s[0] - 1.0).abs() < 1e-6 && (s[1] - 3.0).abs() < 1e-6 && (s[2] - 2.0).abs() < 1e-6);
        assert!(analyze(&m).manifold());
        assert!(m.signed_volume() > 0.0);
    }

    #[test]
    fn node_scale_is_baked_into_vertices() {
        let src = shapes::cuboid([1.0, 1.0, 1.0]);
        let m = read_glb(&glb_of(&src, r#","scale":[10,20,30]"#)).unwrap();
        let s = m.size();
        // Y(20)变成高度 Z,Z(30)变成深度 Y
        assert!((s[0] - 10.0).abs() < 1e-5 && (s[1] - 30.0).abs() < 1e-5 && (s[2] - 20.0).abs() < 1e-5);
    }

    #[test]
    fn mirrored_node_still_faces_outward() {
        let src = shapes::icosphere(5.0, 1);
        let mut m = read_glb(&glb_of(&src, r#","scale":[-1,1,1]"#)).unwrap();
        m.weld_auto();
        assert!(m.signed_volume() > 0.0, "镜像变换不应该把模型翻成内外颠倒");
        assert_eq!(analyze(&m).flipped_edges, 0);
    }

    #[test]
    fn non_glb_bytes_are_rejected() {
        assert!(matches!(read_glb(b"definitely not a glb"), Err(IoError::Malformed(_))));
    }
}
