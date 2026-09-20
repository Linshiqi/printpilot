//! 代码契约(docs/adr/0003):参数集中在 PARAMS 段、每个特征一段、最终形体赋给 `result`。
//! 这里只做文本层面的检查与分段;真正的安全检查(AST 白名单)在 Python 执行器里。

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// `PARAMS` / `RESULT` / 特征名(`FEATURE: mounting_holes` → `mounting_holes`)
    pub name: String,
    pub is_feature: bool,
    /// 段头所在行与段的最后一行(从 1 起,闭区间)
    pub start_line: u32,
    pub end_line: u32,
}

fn header_name(line: &str) -> Option<String> {
    let t = line.trim_start().strip_prefix("# ----")?;
    // `# ---- FEATURE: base ----  说明` → 取两组横线之间的部分
    let name = t.split("----").next().unwrap_or("").trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub fn sections(code: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let total = code.lines().count() as u32;
    for (i, line) in code.lines().enumerate() {
        let Some(raw) = header_name(line) else { continue };
        let line_no = i as u32 + 1;
        if let Some(prev) = out.last_mut() {
            prev.end_line = line_no - 1;
        }
        let (name, is_feature) = match raw.split_once(':') {
            Some((kind, rest)) if kind.trim().eq_ignore_ascii_case("FEATURE") => (rest.trim().to_string(), true),
            _ => (raw.to_ascii_uppercase(), false),
        };
        out.push(Section {
            name,
            is_feature,
            start_line: line_no,
            end_line: total,
        });
    }
    out
}

/// 不符合契约的地方。返回的句子会原样喂给模型,所以写成祈使句。
pub fn contract_issues(code: &str) -> Vec<String> {
    let secs = sections(code);
    let mut issues = Vec::new();
    if !secs.iter().any(|s| s.name == "PARAMS") {
        issues.push("Add a `# ---- PARAMS ----` section at the top with one `name = number  # unit | label | [min, max]` line per dimension.".to_string());
    }
    if !secs.iter().any(|s| s.is_feature) {
        issues.push("Split the model into `# ---- FEATURE: <name> ----` sections, one per feature.".to_string());
    }
    let assigns_result = code.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("result =") || t.starts_with("result=")
    });
    if !assigns_result {
        issues.push("Assign the final shape to a variable named `result`.".to_string());
    }
    issues
}

/// 两版代码之间,哪些段的内容变了(含新增与删除的段)。用来回答「这次局部修改到底动了哪里」,
/// 也用来检查模型有没有老实地只改相关的那一段。
pub fn changed_sections(old: &str, new: &str) -> Vec<String> {
    fn bodies(code: &str) -> Vec<(String, String)> {
        let lines: Vec<&str> = code.lines().collect();
        let mut out = Vec::new();
        let secs = sections(code);
        // 第一个段头之前的内容(import 等)
        let first = secs.first().map(|s| s.start_line as usize - 1).unwrap_or(lines.len());
        out.push(("(header)".to_string(), lines[..first].join("\n").trim().to_string()));
        for s in secs {
            let body = lines[s.start_line as usize..s.end_line as usize].join("\n");
            out.push((s.name, body.trim().to_string()));
        }
        out
    }
    let (old, new) = (bodies(old), bodies(new));
    let mut changed = Vec::new();
    for (name, body) in &new {
        match old.iter().find(|(n, _)| n == name) {
            Some((_, old_body)) if old_body == body => {}
            _ => changed.push(name.clone()),
        }
    }
    for (name, _) in &old {
        if !new.iter().any(|(n, _)| n == name) {
            changed.push(format!("-{name}"));
        }
    }
    changed.retain(|n| n != "(header)" || old[0].1 != new[0].1);
    changed
}

/// 模型常把代码包在 ```python 围栏里,或者前后加几句话。取出代码本体。
pub fn extract_code(answer: &str) -> String {
    let text = answer.trim();
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let after = after.strip_prefix("python").or_else(|| after.strip_prefix("py")).unwrap_or(after);
        let body = match after.find("```") {
            Some(end) => &after[..end],
            None => after,
        };
        return body.trim_matches(['\r', '\n']).to_string() + "\n";
    }
    text.to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "from build123d import *\n\
# ---- PARAMS ----  每行一个\n\
w = 10.0  # mm | 宽\n\
# ---- FEATURE: base ----\n\
base = Box(w, w, 2)\n\
# ---- feature: holes ---- 两个孔\n\
holes = Cylinder(1, 2)\n\
# ---- RESULT ----\n\
result = base - holes\n";

    #[test]
    fn sections_are_found_with_their_line_ranges() {
        let s = sections(CODE);
        let names: Vec<_> = s.iter().map(|x| (x.name.as_str(), x.is_feature)).collect();
        assert_eq!(names, [("PARAMS", false), ("base", true), ("holes", true), ("RESULT", false)]);
        assert_eq!((s[1].start_line, s[1].end_line), (4, 5));
        assert_eq!((s[3].start_line, s[3].end_line), (8, 9));
    }

    #[test]
    fn a_conforming_script_has_no_issues() {
        assert!(contract_issues(CODE).is_empty());
    }

    #[test]
    fn each_missing_piece_is_reported_separately() {
        let issues = contract_issues("from build123d import *\npart = Box(1, 1, 1)\n");
        assert_eq!(issues.len(), 3);
        assert!(issues[0].contains("PARAMS") && issues[1].contains("FEATURE") && issues[2].contains("result"));
        // `result == x` 之类的不算赋值
        assert_eq!(contract_issues("# ---- PARAMS ----\n# ---- FEATURE: a ----\nif result == 1: pass\n").len(), 1);
    }

    #[test]
    fn changed_sections_pinpoints_a_local_edit() {
        assert!(changed_sections(CODE, CODE).is_empty());

        let bigger_hole = CODE.replace("Cylinder(1, 2)", "Cylinder(1.5, 2)");
        assert_eq!(changed_sections(CODE, &bigger_hole), ["holes"]);

        let new_param = CODE.replace("w = 10.0  # mm | 宽", "w = 12.0  # mm | 宽");
        assert_eq!(changed_sections(CODE, &new_param), ["PARAMS"]);

        let added = CODE.replace("# ---- RESULT ----", "# ---- FEATURE: rib ----\nrib = Box(1, w, 1)\n# ---- RESULT ----");
        assert_eq!(changed_sections(CODE, &added), ["rib"]);

        let removed = CODE.replace("# ---- feature: holes ---- 两个孔\nholes = Cylinder(1, 2)\n", "").replace("base - holes", "base");
        assert_eq!(changed_sections(CODE, &removed), ["RESULT", "-holes"]);

        // 只是空白 / 缩进不同,不算改动
        let reflowed = CODE.replace("base = Box(w, w, 2)\n", "base = Box(w, w, 2)\n\n");
        assert!(changed_sections(CODE, &reflowed).is_empty());
    }

    #[test]
    fn code_is_pulled_out_of_markdown_fences_and_chatter() {
        assert_eq!(extract_code("好的,代码如下:\n```python\nx = 1\nresult = x\n```\n希望有帮助"), "x = 1\nresult = x\n");
        assert_eq!(extract_code("```\nx = 1\n```"), "x = 1\n");
        assert_eq!(extract_code("x = 1\n"), "x = 1\n");
        assert_eq!(extract_code("```python\nx = 1\n"), "x = 1\n", "围栏没闭合也要能取出来");
    }
}
