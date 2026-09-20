//! `# ---- PARAMS ----` 段的解析与改写——「局部修改」里最可靠、也不花钱的那一种:
//! 用户在参数面板上改一个数,我们只替换那一行的数字字面量,然后重跑代码。全程不经过模型。
//!
//! 每个参数一行(代码契约见 docs/adr/0003):
//! ```text
//! width = 60.0        # mm | 总宽 | [20, 200]
//! hole_count = 4      # 个 | 安装孔数量 | [2, 8]
//! ```

use pp_common::cad::CadParam;

#[derive(Debug, Clone, PartialEq)]
pub enum ParamError {
    UnknownParam(String),
    OutOfRange { name: String, value: f64, min: Option<f64>, max: Option<f64> },
    NotFinite,
}

impl std::fmt::Display for ParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParamError::UnknownParam(n) => write!(f, "no parameter named {n}"),
            ParamError::OutOfRange { name, value, min, max } => {
                write!(f, "{name} = {value} is outside [{min:?}, {max:?}]")
            }
            ParamError::NotFinite => write!(f, "value is not a finite number"),
        }
    }
}

impl std::error::Error for ParamError {}

fn is_section_header(line: &str) -> bool {
    line.trim_start().starts_with("# ----")
}

fn is_params_header(line: &str) -> bool {
    is_section_header(line) && line.to_ascii_uppercase().contains("PARAMS")
}

/// 一行 `name = 数字  # …` 拆成 (名字, 数字字面量的字节范围, 注释)。不是参数行返回 None。
fn split_param_line(line: &str) -> Option<(&str, std::ops::Range<usize>, &str)> {
    let (code, comment) = match line.find('#') {
        Some(i) => (&line[..i], line[i + 1..].trim()),
        None => (line, ""),
    };
    let eq = code.find('=')?;
    let name = code[..eq].trim();
    let valid_name = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    if !valid_name {
        return None;
    }
    let rhs = &code[eq + 1..];
    let literal = rhs.trim();
    if literal.is_empty() || literal.parse::<f64>().is_err() {
        return None; // 右边不是纯数字(表达式、字符串、元组…):不当参数,原样保留
    }
    let start = eq + 1 + (rhs.len() - rhs.trim_start().len());
    Some((name, start..start + literal.len(), comment))
}

fn parse_range(text: &str) -> (Option<f64>, Option<f64>) {
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    let mut parts = inner.split(',').map(|p| p.trim().parse::<f64>().ok());
    (parts.next().flatten(), parts.next().flatten())
}

pub fn parse_params(code: &str) -> Vec<CadParam> {
    let mut out = Vec::new();
    let mut in_block = false;
    for (i, line) in code.lines().enumerate() {
        if is_section_header(line) {
            in_block = is_params_header(line);
            continue;
        }
        if !in_block {
            continue;
        }
        let Some((name, range, comment)) = split_param_line(line) else {
            continue;
        };
        let literal = &line[range];
        let mut fields = comment.split('|').map(str::trim);
        let unit = fields.next().unwrap_or("").to_string();
        let label = fields.next().unwrap_or("").to_string();
        let (min, max) = fields.next().map(parse_range).unwrap_or((None, None));
        out.push(CadParam {
            name: name.to_string(),
            value: literal.parse().unwrap_or(0.0),
            unit,
            label,
            min,
            max,
            line: i as u32 + 1,
            integer: !literal.contains(['.', 'e', 'E']),
        });
    }
    out
}

fn format_value(value: f64, integer: bool) -> String {
    if integer && value.fract() == 0.0 {
        return format!("{}", value as i64);
    }
    // 最多 4 位小数,去掉多余的 0,但保留一位小数(60.0 而不是 60——让它继续是浮点数)
    let s = format!("{value:.4}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    }
}

/// 把参数 `name` 改成 `value`,返回新代码。只动那一行的数字,注释、对齐、其它行原样保留。
pub fn set_param(code: &str, name: &str, value: f64) -> Result<String, ParamError> {
    if !value.is_finite() {
        return Err(ParamError::NotFinite);
    }
    let param = parse_params(code)
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| ParamError::UnknownParam(name.to_string()))?;
    if param.min.is_some_and(|m| value < m) || param.max.is_some_and(|m| value > m) {
        return Err(ParamError::OutOfRange {
            name: name.to_string(),
            value,
            min: param.min,
            max: param.max,
        });
    }
    let mut out = String::with_capacity(code.len() + 8);
    for (i, line) in code.split_inclusive('\n').enumerate() {
        if i as u32 + 1 != param.line {
            out.push_str(line);
            continue;
        }
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        let (_, range, _) = split_param_line(body).expect("line was parsed as a param a moment ago");
        let new_literal = format_value(value, param.integer);
        // 数字变长 / 变短时,吃掉或补上后面的空格,让行尾注释尽量保持对齐
        let after = &body[range.end..];
        let pad_before = after.len() - after.trim_start_matches(' ').len();
        let old_len = range.len();
        let pad = (pad_before + old_len).saturating_sub(new_literal.len()).max(if after.trim().is_empty() { 0 } else { 2 });
        out.push_str(&body[..range.start]);
        out.push_str(&new_literal);
        if !after.trim().is_empty() {
            out.push_str(&" ".repeat(pad));
        }
        out.push_str(after.trim_start_matches(' '));
        out.push_str(ending);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "from build123d import *\n\
\n\
# ---- PARAMS ----  每行一个\n\
width = 60.0        # mm | 总宽 | [20, 200]\n\
hole_count = 4      # 个 | 安装孔数量 | [2, 8]\n\
wall = 2.4          # mm | 壁厚\n\
label = \"PP\"        # 不是数字,不算参数\n\
ratio = width / 2   # 表达式,不算参数\n\
\n\
# ---- FEATURE: base ----\n\
height = 10.0       # 不在 PARAMS 段里,不算参数\n\
base = Box(width, 40, height)\n\
\n\
# ---- RESULT ----\n\
result = base\n";

    #[test]
    fn parses_only_numeric_lines_inside_the_params_block() {
        let ps = parse_params(CODE);
        let names: Vec<_> = ps.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["width", "hole_count", "wall"]);

        assert_eq!(ps[0].value, 60.0);
        assert_eq!((ps[0].unit.as_str(), ps[0].label.as_str()), ("mm", "总宽"));
        assert_eq!((ps[0].min, ps[0].max), (Some(20.0), Some(200.0)));
        assert_eq!(ps[0].line, 4);
        assert!(!ps[0].integer);

        assert!(ps[1].integer, "4 是整数字面量");
        assert_eq!((ps[2].min, ps[2].max), (None, None), "没写范围就是不限");
    }

    #[test]
    fn set_param_touches_only_that_number() {
        let new = set_param(CODE, "width", 85.5).unwrap();
        assert!(new.contains("width = 85.5        # mm | 总宽 | [20, 200]"), "{new}");
        // 其它每一行都原样
        for (a, b) in CODE.lines().zip(new.lines()) {
            if !a.starts_with("width") {
                assert_eq!(a, b);
            }
        }
        assert_eq!(parse_params(&new)[0].value, 85.5);
    }

    #[test]
    fn integers_stay_integers_and_floats_stay_floats() {
        let new = set_param(CODE, "hole_count", 6.0).unwrap();
        assert!(new.contains("hole_count = 6      # 个"), "{new}");
        assert!(parse_params(&new)[1].integer);

        let new = set_param(CODE, "width", 100.0).unwrap();
        assert!(new.contains("width = 100.0       # mm"), "整数值的浮点参数要写成 100.0: {new}");
        assert!(!parse_params(&new)[0].integer);
    }

    #[test]
    fn out_of_range_unknown_and_non_finite_values_are_refused() {
        assert!(matches!(set_param(CODE, "width", 500.0), Err(ParamError::OutOfRange { .. })));
        assert!(matches!(set_param(CODE, "width", 5.0), Err(ParamError::OutOfRange { .. })));
        assert_eq!(set_param(CODE, "height", 5.0), Err(ParamError::UnknownParam("height".into())));
        assert_eq!(set_param(CODE, "wall", f64::NAN), Err(ParamError::NotFinite));
        assert!(set_param(CODE, "wall", 9999.0).is_ok(), "没写范围的参数不设限");
    }

    #[test]
    fn crlf_files_and_lines_without_comments_survive() {
        let code = "# ---- PARAMS ----\r\nw = 10\r\nd = 2.50\r\n# ---- RESULT ----\r\nresult = None\r\n";
        let new = set_param(code, "w", 12.0).unwrap();
        assert_eq!(new, "# ---- PARAMS ----\r\nw = 12\r\nd = 2.50\r\n# ---- RESULT ----\r\nresult = None\r\n");
        let new = set_param(code, "d", 3.125).unwrap();
        assert!(new.contains("d = 3.125\r\n"));
    }

    #[test]
    fn repeated_edits_are_stable() {
        let mut code = CODE.to_string();
        for v in [61.0, 123.4567, 20.0, 60.0] {
            code = set_param(&code, "width", v).unwrap();
        }
        assert_eq!(code, CODE, "改一圈再改回来,文件应当逐字节还原");
    }

    #[test]
    fn code_without_a_params_block_has_no_params() {
        assert!(parse_params("x = 1\nresult = Box(x, x, x)\n").is_empty());
    }
}
