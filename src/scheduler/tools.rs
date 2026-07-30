//! 定时任务模块工具函数。
//!
//! 提供 cron 表达式解析、短 ID 生成等工具函数。

use chrono::{DateTime, Datelike, Timelike, Utc};

/// 生成指定长度的短 ID（基于时间戳）。
pub fn generate_short_id(len: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let hex = format!("{:x}", nanos);
    hex.chars().take(len).collect()
}

/// 简单解析标准 5 字段 cron 表达式，计算下一次执行时间。
///
/// 格式: "分 时 日 月 周"
/// 字段范围:
/// - 分: 0-59
/// - 时: 0-23
/// - 日: 1-31
/// - 月: 1-12
/// - 周: 0-6 (0=周日)
///
/// 支持:
/// - 精确值: `0`
/// - 通配符: `*`
/// - 步长: `*/5`（每5分钟）
/// - 值列表: `1,3,5`
/// - 范围: `1-5`
///
/// 注意：这是一个简化实现，不支持 `L`、`W`、`#` 等特殊字符。
pub fn parse_cron_next(expr: &str, base_time: i64) -> Option<i64> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }

    let base = DateTime::from_timestamp(base_time, 0)?;
    let mut next = base.checked_add_signed(chrono::Duration::minutes(1))?; // 至少1分钟后

    // 最多搜索 365 天
    for _ in 0..(365 * 24 * 60) {
        if matches_cron(&parts, next) {
            return Some(next.timestamp());
        }
        next = next.checked_add_signed(chrono::Duration::minutes(1))?;
    }

    None
}

/// 检查给定的时间是否匹配 cron 表达式。
fn matches_cron(parts: &[&str], dt: DateTime<Utc>) -> bool {
    if parts.len() != 5 {
        return false;
    }

    let minute = dt.minute() as i32;
    let hour = dt.hour() as i32;
    let day = dt.day() as i32;
    let month = dt.month() as i32;
    let weekday = dt.weekday().num_days_from_sunday() as i32;

    field_matches(parts[0], minute)
        && field_matches(parts[1], hour)
        && field_matches(parts[2], day)
        && field_matches(parts[3], month)
        && field_matches(parts[4], weekday)
}

/// 判断单个 cron 字段是否匹配给定值。
fn field_matches(field: &str, value: i32) -> bool {
    // 处理逗号分隔的列表
    if field.contains(',') {
        return field.split(',').any(|part| field_matches(part.trim(), value));
    }

    // 处理步长: */N 或 A-B/N
    if let Some((range_part, step_str)) = field.split_once('/') {
        let step: i32 = step_str.parse().unwrap_or(1);
        let (low, high) = parse_range(range_part, value);
        if low > high {
            return false;
        }
        if value >= low && value <= high && (value - low) % step == 0 {
            return true;
        }
        return false;
    }

    // 处理范围: A-B
    if let Some((low_str, high_str)) = field.split_once('-') {
        let low: i32 = low_str.parse().unwrap_or(0);
        let high: i32 = high_str.parse().unwrap_or(59);
        return value >= low && value <= high;
    }

    // 处理通配符
    if field == "*" {
        return true;
    }

    // 处理精确值
    if let Ok(val) = field.parse::<i32>() {
        return value == val;
    }

    false
}

/// 解析范围部分，返回 (low, high)。
fn parse_range(range_part: &str, _default: i32) -> (i32, i32) {
    if range_part == "*" {
        return (0, 59);
    }

    if let Some((low_str, high_str)) = range_part.split_once('-') {
        let low: i32 = low_str.parse().unwrap_or(0);
        let high: i32 = high_str.parse().unwrap_or(59);
        return (low, high);
    }

    // 单个值
    let val: i32 = range_part.parse().unwrap_or(0);
    (val, val)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_generate_short_id() {
        let id = generate_short_id(8);
        assert_eq!(id.len(), 8);
    }

    #[test]
    fn test_parse_cron_every_minute() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap().timestamp();
        let next = parse_cron_next("* * * * *", base);
        assert!(next.is_some());
        assert!(next.unwrap() >= base + 60);
    }

    #[test]
    fn test_parse_cron_hourly() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 30, 0).unwrap().timestamp();
        let next = parse_cron_next("0 * * * *", base);
        assert!(next.is_some());
        let next_dt = DateTime::from_timestamp(next.unwrap(), 0).unwrap();
        assert_eq!(next_dt.minute(), 0);
        assert_eq!(next_dt.hour(), 1);
    }

    #[test]
    fn test_parse_cron_daily() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap().timestamp();
        let next = parse_cron_next("30 8 * * *", base);
        assert!(next.is_some());
        let next_dt = DateTime::from_timestamp(next.unwrap(), 0).unwrap();
        assert_eq!(next_dt.hour(), 8);
        assert_eq!(next_dt.minute(), 30);
    }

    #[test]
    fn test_parse_cron_every_5_minutes() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 3, 0).unwrap().timestamp();
        let next = parse_cron_next("*/5 * * * *", base);
        assert!(next.is_some());
        let next_dt = DateTime::from_timestamp(next.unwrap(), 0).unwrap();
        assert_eq!(next_dt.minute(), 5);
    }

    #[test]
    fn test_parse_cron_specific_times() {
        let base = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap().timestamp();
        let next = parse_cron_next("5,15,25 * * * *", base);
        assert!(next.is_some());
        let next_dt = DateTime::from_timestamp(next.unwrap(), 0).unwrap();
        assert_eq!(next_dt.minute(), 5);
        assert_eq!(next_dt.hour(), 0);
    }

    #[test]
    fn test_invalid_cron_expression() {
        assert!(parse_cron_next("* * * *", 0).is_none());
        assert!(parse_cron_next("", 0).is_none());
        assert!(parse_cron_next("* * * * * *", 0).is_none());
    }

    #[test]
    fn test_field_matches_wildcard() {
        assert!(field_matches("*", 0));
        assert!(field_matches("*", 59));
        assert!(field_matches("*", 30));
    }

    #[test]
    fn test_field_matches_exact() {
        assert!(field_matches("5", 5));
        assert!(!field_matches("5", 4));
        assert!(!field_matches("5", 6));
    }

    #[test]
    fn test_field_matches_range() {
        assert!(field_matches("1-5", 3));
        assert!(field_matches("1-5", 1));
        assert!(field_matches("1-5", 5));
        assert!(!field_matches("1-5", 0));
        assert!(!field_matches("1-5", 6));
    }

    #[test]
    fn test_field_matches_list() {
        assert!(field_matches("1,3,5", 1));
        assert!(field_matches("1,3,5", 3));
        assert!(field_matches("1,3,5", 5));
        assert!(!field_matches("1,3,5", 2));
        assert!(!field_matches("1,3,5", 4));
    }

    #[test]
    fn test_field_matches_step() {
        assert!(field_matches("*/5", 0));
        assert!(field_matches("*/5", 5));
        assert!(field_matches("*/5", 10));
        assert!(!field_matches("*/5", 3));
        assert!(!field_matches("*/5", 7));
    }
}