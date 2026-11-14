//! TUI 共享小助手：相对时间、日分组、宽度感知截断、spinner、状态配色、
//! 参数摘要等纯函数（无状态、无副作用，供各视图复用）。

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::style::Color;
use serde_json::Value;
use time::OffsetDateTime;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 当前 Unix 毫秒时间戳（时钟回拨时回退 0）。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// spinner 帧（随渲染 tick 推进）。
pub fn spinner(frame: u64) -> &'static str {
    SPINNER[(frame % SPINNER.len() as u64) as usize]
}

/// 时间戳对应的本地日期（无法确定本地时区时回退 None）。
pub fn local_date(ms: u64) -> Option<time::Date> {
    let secs = i64::try_from(ms / 1000).ok()?;
    let offset = time::UtcOffset::current_local_offset().ok()?;
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .map(|t| t.to_offset(offset).date())
}

/// 会话按更新时间的日分组：今天 / 昨天 / 更早。
pub fn day_group(ms: u64, now: u64) -> &'static str {
    let Some(day) = local_date(ms) else {
        return "更早";
    };
    let Some(today) = local_date(now) else {
        return "更早";
    };
    if day == today {
        "今天"
    } else if today.previous_day().is_some_and(|d| d == day) {
        "昨天"
    } else {
        "更早"
    }
}

/// 相对时间标签：刚刚 / N分钟前 / N小时前 / 昨天 / N天前 / MM-DD。
pub fn relative_time(ms: u64, now: u64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let diff = now.saturating_sub(ms) / 1000;
    match diff {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{} 分钟前", diff / 60),
        3600..=86_399 => format!("{} 小时前", diff / 3600),
        86_400..=172_799 => "昨天".into(),
        _ => {
            let days = diff / 86_400;
            if days <= 30 {
                format!("{days} 天前")
            } else {
                match local_date(ms) {
                    Some(day) => format!("{:02}-{:02}", u8::from(day.month()), day.day()),
                    None => "更早".into(),
                }
            }
        }
    }
}

/// 按显示宽度截断（省略号占 1 列，CJK/emoji 宽字符不截半）。
pub fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        width += w;
    }
    out.push('…');
    out
}

/// 工具参数摘要：字符串原样截断、对象取前 3 个 k=v、其余紧凑 JSON。
pub fn arg_summary(args: &Value, max: usize) -> String {
    let text = match args {
        Value::String(s) => s.clone(),
        Value::Object(map) => map
            .iter()
            .take(3)
            .map(|(key, value)| {
                let rendered = match value {
                    Value::String(s) => truncate(s, 24),
                    other => truncate(&other.to_string(), 24),
                };
                format!("{key}={rendered}")
            })
            .collect::<Vec<_>>()
            .join(" "),
        other => truncate(&other.to_string(), 60),
    };
    truncate(&text, max)
}

/// 任务/会话状态 → 颜色。
pub fn status_color(status: &str) -> Color {
    match status {
        "running" => Color::Green,
        "queued" => Color::Yellow,
        "completed" => Color::Green,
        "failed" => Color::Red,
        "cancelled" => Color::Rgb(128, 142, 151),
        _ => Color::Gray,
    }
}

/// 长标识符取前 8 位用于卡片展示。
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// `#rrggbb` → ratatui 颜色；非法输入回退 None。
pub fn hex_color(spec: &str) -> Option<Color> {
    let hex = spec.strip_prefix('#')?;
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Color::Rgb(r, g, b))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_is_display_width_aware() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("中文", 10), "中文");
        // 两个宽字符占 4 列，截到 3 列只留一个 + 省略号
        assert_eq!(truncate("中文", 3), "中…");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 3), "abc");
    }

    #[test]
    fn day_groups_use_local_calendar() {
        let now = now_ms();
        assert_eq!(day_group(now, now), "今天");
        assert_eq!(day_group(now - 86_400_000, now), "昨天");
        assert_eq!(day_group(now - 3 * 86_400_000, now), "更早");
        assert_eq!(day_group(0, now), "更早");
    }

    #[test]
    fn relative_time_buckets() {
        let now = now_ms();
        assert_eq!(relative_time(now, now), "刚刚");
        assert_eq!(relative_time(now - 120_000, now), "2 分钟前");
        assert_eq!(relative_time(now - 7_200_000, now), "2 小时前");
        assert_eq!(relative_time(now - 86_400_000, now), "昨天");
        assert_eq!(relative_time(now - 3 * 86_400_000, now), "3 天前");
        assert_eq!(relative_time(now + 5_000, now), "刚刚");
        assert_eq!(relative_time(0, now), "—");
    }

    #[test]
    fn arg_summary_compact_and_truncated() {
        assert_eq!(
            arg_summary(&serde_json::json!("直接字符串"), 100),
            "直接字符串"
        );
        // serde_json Map 键序不保证（无 preserve_order），断言与键序无关
        let object = serde_json::json!({"path": "src/lib.rs", "mode": "w", "extra": 1});
        let summary = arg_summary(&object, 100);
        for pair in ["path=src/lib.rs", "mode=w", "extra=1"] {
            assert!(summary.contains(pair), "{summary}");
        }
        let long = serde_json::json!({"path": "一个很长很很长很很长很很长很很长很长的路径"});
        assert!(arg_summary(&long, 20).ends_with('…'));
    }

    #[test]
    fn hex_color_parses_valid_and_rejects_invalid() {
        assert_eq!(hex_color("#ff0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(hex_color("ff0000"), None);
        assert_eq!(hex_color("#ff00"), None);
        assert_eq!(hex_color("#gggggg"), None);
    }
}
