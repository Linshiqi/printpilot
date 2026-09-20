use super::IoError;
use crate::mesh::{cross, norm, sub, Mesh};

const HEADER: usize = 80;
const RECORD: usize = 50;

/// 读 STL(二进制或 ASCII)。返回的网格**没有共享顶点**,调用方需要 `weld`(`read_mesh` 会做)。
pub fn read_stl(bytes: &[u8]) -> Result<Mesh, IoError> {
    // 先按二进制的长度公式判断:有些二进制 STL 的文件头也以 "solid" 开头,不能只看开头
    if bytes.len() >= HEADER + 4 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        if n.checked_mul(RECORD).and_then(|b| b.checked_add(HEADER + 4)) == Some(bytes.len()) {
            return Ok(read_binary(bytes, n));
        }
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| IoError::Malformed("neither binary STL (length mismatch) nor text".into()))?;
    if !text.trim_start().starts_with("solid") {
        return Err(IoError::Malformed("missing 'solid' header".into()));
    }
    read_ascii(text)
}

fn read_binary(bytes: &[u8], n: usize) -> Mesh {
    let f = |off: usize| f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]) as f64;
    let mut mesh = Mesh {
        positions: Vec::with_capacity(n * 3),
        triangles: Vec::with_capacity(n),
    };
    for i in 0..n {
        let rec = HEADER + 4 + i * RECORD;
        let base = mesh.positions.len() as u32;
        for v in 0..3 {
            let off = rec + 12 + v * 12; // 跳过 12 字节的法线
            mesh.positions.push([f(off), f(off + 4), f(off + 8)]);
        }
        mesh.triangles.push([base, base + 1, base + 2]);
    }
    mesh
}

fn read_ascii(text: &str) -> Result<Mesh, IoError> {
    let mut mesh = Mesh::default();
    let mut pending: Vec<[f64; 3]> = Vec::with_capacity(3);
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("vertex") {
            continue;
        }
        let mut p = [0.0; 3];
        for slot in &mut p {
            *slot = words
                .next()
                .and_then(|w| w.parse::<f64>().ok())
                .ok_or_else(|| IoError::Malformed(format!("bad vertex line: {}", line.trim())))?;
        }
        pending.push(p);
        if pending.len() == 3 {
            let base = mesh.positions.len() as u32;
            mesh.positions.append(&mut pending);
            mesh.triangles.push([base, base + 1, base + 2]);
        }
    }
    if !pending.is_empty() {
        return Err(IoError::Malformed("vertex count is not a multiple of 3".into()));
    }
    Ok(mesh)
}

/// 写二进制 STL(单位毫米)。法线按绕向现算——有些切片软件会校验法线与绕向是否一致。
pub fn write_stl(mesh: &Mesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + 4 + mesh.triangles.len() * RECORD);
    let mut header = [0u8; HEADER];
    // 文件头不能以 "solid" 开头,否则会被一些读取器误判成 ASCII
    let tag = b"PrintPilot binary STL (mm)";
    header[..tag.len()].copy_from_slice(tag);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(mesh.triangles.len() as u32).to_le_bytes());
    for t in &mesh.triangles {
        let (a, b, c) = mesh.tri(*t);
        let mut n = cross(sub(b, a), sub(c, a));
        let len = norm(n);
        if len > 0.0 {
            n = [n[0] / len, n[1] / len, n[2] / len];
        }
        for v in [n, a, b, c] {
            for x in v {
                out.extend_from_slice(&(x as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, shapes};

    #[test]
    fn binary_roundtrip_preserves_shape() {
        let src = shapes::torus(20.0, 5.0, 48, 24);
        let bytes = write_stl(&src);
        assert_eq!(bytes.len(), 84 + src.triangles.len() * 50);
        let mut back = read_stl(&bytes).unwrap();
        assert_eq!(back.triangles.len(), src.triangles.len());
        back.weld_auto();
        let (a, b) = (analyze(&src), analyze(&back));
        assert!(b.manifold());
        assert!((a.volume_mm3 - b.volume_mm3).abs() / a.volume_mm3 < 1e-5, "f32 精度内一致");
    }

    #[test]
    fn written_normals_point_outward() {
        let bytes = write_stl(&shapes::cuboid([1.0, 1.0, 1.0]));
        // 第一个三角形在 −Z 面上
        let nz = f32::from_le_bytes(bytes[84 + 8..84 + 12].try_into().unwrap());
        assert_eq!(nz, -1.0);
    }

    #[test]
    fn ascii_is_parsed() {
        let text = "solid t\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1.5 0 0\n   vertex 0 2e0 0\n  endloop\n endfacet\nendsolid t\n";
        let m = read_stl(text.as_bytes()).unwrap();
        assert_eq!(m.triangles, vec![[0, 1, 2]]);
        assert_eq!(m.positions[1], [1.5, 0.0, 0.0]);
        assert_eq!(m.positions[2], [0.0, 2.0, 0.0]);
    }

    #[test]
    fn binary_file_whose_header_says_solid_is_still_read_as_binary() {
        let mut bytes = write_stl(&shapes::cuboid([1.0, 1.0, 1.0]));
        bytes[..5].copy_from_slice(b"solid");
        assert_eq!(read_stl(&bytes).unwrap().triangles.len(), 12);
    }

    #[test]
    fn garbage_and_truncated_files_are_rejected() {
        assert!(matches!(read_stl(b"not an stl"), Err(IoError::Malformed(_))));
        let mut bytes = write_stl(&shapes::cuboid([1.0, 1.0, 1.0]));
        bytes.truncate(bytes.len() - 7);
        assert!(matches!(read_stl(&bytes), Err(IoError::Malformed(_))));
        let bad_ascii = "solid x\n vertex 0 0\n";
        assert!(matches!(read_stl(bad_ascii.as_bytes()), Err(IoError::Malformed(_))));
    }
}
