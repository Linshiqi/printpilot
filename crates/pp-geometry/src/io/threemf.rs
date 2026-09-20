use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

use crate::mesh::Mesh;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>
"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"#;

/// 写 3MF(核心规范,单个对象,单位毫米)。比 STL 的好处:自带单位、共享顶点、体积小一半以上,
/// 拓竹系切片软件的原生格式。
pub fn write_3mf(mesh: &Mesh, title: &str) -> Result<Vec<u8>, String> {
    let mut model = String::with_capacity(64 * mesh.positions.len() + 48 * mesh.triangles.len() + 512);
    model.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    model.push('\n');
    model.push_str(r#"<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">"#);
    model.push('\n');
    model.push_str(&format!(" <metadata name=\"Title\">{}</metadata>\n", xml_escape(title)));
    model.push_str(" <metadata name=\"Application\">PrintPilot</metadata>\n");
    model.push_str(" <resources>\n  <object id=\"1\" type=\"model\">\n   <mesh>\n    <vertices>\n");
    for p in &mesh.positions {
        // 6 位小数 = 纳米级,远超打印精度;避免科学计数法(规范不允许)
        model.push_str(&format!(
            "     <vertex x=\"{:.6}\" y=\"{:.6}\" z=\"{:.6}\"/>\n",
            p[0], p[1], p[2]
        ));
    }
    model.push_str("    </vertices>\n    <triangles>\n");
    for t in &mesh.triangles {
        model.push_str(&format!(
            "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>\n",
            t[0], t[1], t[2]
        ));
    }
    model.push_str("    </triangles>\n   </mesh>\n  </object>\n </resources>\n <build>\n  <item objectid=\"1\"/>\n </build>\n</model>\n");

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, body) in [
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", RELS),
        ("3D/3dmodel.model", model.as_str()),
    ] {
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes;
    use std::io::Read;

    fn entry(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn package_has_the_three_required_parts() {
        let bytes = write_3mf(&shapes::cuboid([10.0, 20.0, 30.0]), "盒子").unwrap();
        let zip = zip::ZipArchive::new(Cursor::new(&bytes[..])).unwrap();
        let mut names: Vec<_> = zip.file_names().map(str::to_string).collect();
        names.sort();
        assert_eq!(names, ["3D/3dmodel.model", "[Content_Types].xml", "_rels/.rels"]);
        assert!(entry(&bytes, "_rels/.rels").contains("/3D/3dmodel.model"));
    }

    #[test]
    fn model_declares_millimeters_and_lists_every_vertex_and_triangle() {
        let bytes = write_3mf(&shapes::cuboid([10.0, 20.0, 30.0]), "box").unwrap();
        let model = entry(&bytes, "3D/3dmodel.model");
        assert!(model.contains(r#"unit="millimeter""#));
        assert_eq!(model.matches("<vertex ").count(), 8);
        assert_eq!(model.matches("<triangle ").count(), 12);
        assert!(model.contains(r#"<vertex x="10.000000" y="20.000000" z="30.000000"/>"#));
        assert!(model.contains(r#"<item objectid="1"/>"#));
    }

    #[test]
    fn title_is_escaped_and_tiny_coordinates_avoid_scientific_notation() {
        let mut m = shapes::cuboid([1.0, 1.0, 1.0]);
        m.positions[0] = [1e-9, 0.0, 0.0];
        let model = entry(&write_3mf(&m, "a<b>&\"c\"").unwrap(), "3D/3dmodel.model");
        assert!(model.contains("a&lt;b&gt;&amp;&quot;c&quot;"));
        assert!(!model.contains("e-"), "3MF 规范不允许科学计数法");
    }
}
