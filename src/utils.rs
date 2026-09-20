//! 纯函数工具。能在宿主机上 `cargo test` 的逻辑尽量放这里,而不是埋在组件里。

/// 当前时间(毫秒)。浏览器里用 `Date.now()`。
pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// 毫秒时间戳 → `MM-DD HH:mm`(本地时区由调用方给出偏移,便于测试)。
pub fn format_ts(ms: i64, tz_offset_minutes: i32) -> String {
    let local = ms + tz_offset_minutes as i64 * 60_000;
    let days = local.div_euclid(86_400_000);
    let rem = local.rem_euclid(86_400_000);
    let (hour, minute) = (rem / 3_600_000, rem % 3_600_000 / 60_000);
    let (_, month, day) = civil_from_days(days);
    format!("{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// 浏览器当前时区相对 UTC 的分钟偏移(东八区 = +480)。
pub fn local_tz_offset_minutes() -> i32 {
    -(js_sys::Date::new_0().get_timezone_offset() as i32)
}

/// 自 1970-01-01 起的天数 → (年, 月, 日)。Howard Hinnant 的 civil_from_days 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// 毫米数 → 界面上的字符串:小于 100 留一位小数,否则取整。
pub fn fmt_mm(v: f64) -> String {
    if v.abs() < 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

/// 千分位整数:`182340` → `182,340`。
pub fn fmt_int(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// 解析用户在输入框里敲的正数(容忍全角数字、逗号小数点、首尾空格)。
pub fn parse_positive(text: &str) -> Option<f64> {
    let normalized: String = text
        .trim()
        .chars()
        .map(|c| match c {
            // 中文输入法下容易敲出全角数字
            '\u{FF10}'..='\u{FF19}' => char::from(b'0' + (c as u32 - 0xFF10) as u8),
            ',' | '。' | '\u{FF0E}' | '\u{FF0C}' => '.',
            other => other,
        })
        .collect();
    normalized.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_points() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29)); // 闰日
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn timestamps_format_in_the_given_timezone() {
        // 2026-09-20 06:30:00 UTC
        let ms = 1_789_885_800_000;
        assert_eq!(format_ts(ms, 0), "09-20 06:30");
        assert_eq!(format_ts(ms, 480), "09-20 14:30");
        // 跨日:UTC 20 点在东八区已经是第二天
        assert_eq!(format_ts(ms + 14 * 3_600_000, 480), "09-21 04:30");
    }

    #[test]
    fn millimeters_and_counts_are_readable() {
        assert_eq!(fmt_mm(62.04), "62.0");
        assert_eq!(fmt_mm(256.4), "256");
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1_000), "1,000");
        assert_eq!(fmt_int(1_310_720), "1,310,720");
    }

    #[test]
    fn user_typed_numbers_are_parsed_leniently_but_must_be_positive() {
        assert_eq!(parse_positive(" 60 "), Some(60.0));
        assert_eq!(parse_positive("2,5"), Some(2.5));
        assert_eq!(parse_positive("12.5"), Some(12.5));
        assert_eq!(parse_positive("６０。５"), Some(60.5), "全角数字与中文句号");
        assert_eq!(parse_positive("0"), None);
        assert_eq!(parse_positive("-3"), None);
        assert_eq!(parse_positive("abc"), None);
        assert_eq!(parse_positive(""), None);
        assert_eq!(parse_positive("inf"), None);
    }
}
