//! 网格读写。读进来的网格一律:单位毫米、Z 朝上、已焊接(可以直接 `analyze`)。

mod glb;
mod stl;
mod threemf;

pub use glb::read_glb;
pub use stl::{read_stl, write_stl};
pub use threemf::write_3mf;

use crate::mesh::Mesh;

#[derive(Debug, Clone, PartialEq)]
pub enum IoError {
    /// 扩展名不认识
    UnsupportedFormat(String),
    /// 文件内容不是合法的该格式
    Malformed(String),
    /// 文件合法,但里面没有三角形
    Empty,
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::UnsupportedFormat(ext) => write!(f, "unsupported format: {ext}"),
            IoError::Malformed(why) => write!(f, "malformed file: {why}"),
            IoError::Empty => write!(f, "file contains no triangles"),
        }
    }
}

impl std::error::Error for IoError {}

/// 按扩展名读入,并做好后续分析需要的准备(焊接)。
pub fn read_mesh(bytes: &[u8], ext: &str) -> Result<Mesh, IoError> {
    let mut mesh = match ext.to_ascii_lowercase().as_str() {
        "stl" => read_stl(bytes)?,
        "glb" => read_glb(bytes)?,
        other => return Err(IoError::UnsupportedFormat(other.to_string())),
    };
    mesh.weld_auto();
    mesh.compact();
    if mesh.is_empty() {
        return Err(IoError::Empty);
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, shapes};

    #[test]
    fn read_mesh_welds_so_analysis_is_meaningful() {
        let bytes = write_stl(&shapes::icosphere(10.0, 2));
        let mesh = read_mesh(&bytes, "STL").unwrap();
        assert!(analyze(&mesh).manifold());
    }

    #[test]
    fn unknown_extension_and_empty_files_are_distinct_errors() {
        assert_eq!(
            read_mesh(b"whatever", "step"),
            Err(IoError::UnsupportedFormat("step".into()))
        );
        let empty = write_stl(&Mesh::default());
        assert_eq!(read_mesh(&empty, "stl"), Err(IoError::Empty));
    }
}
