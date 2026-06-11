//! # Helper Types and Utility Functions
//!
//! Common types and functions used across all resource types.
//!
//! ## Key Exports
//!
//! - [`Quantity`] — resource quantity (e.g., "128Mi", "500m")
//! - [`IntOrString`] — value that can be int or string
//! - [`LabelSelector`] — label matching rules
//!
//! ## Utility Functions
//!
//! - [`now_rfc3339()`] — current time as RFC3339 string
//! - [`now_epoch_ms()`] — current time as epoch milliseconds
//! - [`parse_rfc3339_secs()`] — parse RFC3339 to seconds since epoch

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ── Quantity ──────────────────────────────────────────────────────────────

/// Resource quantity (e.g., "128Mi", "500m", "10Gi").
///
/// Follows Kubernetes quantity parsing rules:
/// - Binary: Ki, Mi, Gi, Ti (1024-based)
/// - Decimal: m, k, M, G, T (1000-based)
/// - CPU: "500m" = 0.5 cores
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Quantity(pub String);

impl Quantity {
    /// Parse as bytes (handles Ki, Mi, Gi suffixes).
    pub fn as_bytes(&self) -> Option<u64> {
        let s = self.0.trim();
        if s.is_empty() { return None; }
        if let Some(v) = s.strip_suffix("Ki") { v.parse().ok().map(|v: u64| v * 1024) }
        else if let Some(v) = s.strip_suffix("Mi") { v.parse().ok().map(|v: u64| v * 1024 * 1024) }
        else if let Some(v) = s.strip_suffix("Gi") { v.parse().ok().map(|v: u64| v * 1024 * 1024 * 1024) }
        else if let Some(v) = s.strip_suffix("Ti") { v.parse().ok().map(|v: u64| v * 1024 * 1024 * 1024 * 1024) }
        else if let Some(v) = s.strip_suffix("K") { v.parse().ok().map(|v: u64| v * 1000) }
        else if let Some(v) = s.strip_suffix("M") { v.parse().ok().map(|v: u64| v * 1000 * 1000) }
        else if let Some(v) = s.strip_suffix("G") { v.parse().ok().map(|v: u64| v * 1000 * 1000 * 1000) }
        else { s.parse().ok() }
    }

    /// Parse as CPU cores (handles "m" suffix for millicores).
    pub fn as_cpu_cores(&self) -> Option<f64> {
        let s = self.0.trim();
        if let Some(v) = s.strip_suffix('m') {
            v.parse::<f64>().ok().map(|v| v / 1000.0)
        } else {
            s.parse().ok()
        }
    }
}

impl std::fmt::Display for Quantity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── IntOrString ───────────────────────────────────────────────────────────

/// Value that can be an integer or a string.
/// Used for fields like `maxSurge` in rolling updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum IntOrString {
    Int(i32),
    String(String),
}

impl Default for IntOrString {
    fn default() -> Self {
        IntOrString::Int(0)
    }
}

impl std::fmt::Display for IntOrString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntOrString::Int(i) => write!(f, "{}", i),
            IntOrString::String(s) => write!(f, "{}", s),
        }
    }
}

// ── Labels ────────────────────────────────────────────────────────────────

/// Labels are key-value pairs for resource selection.
pub type Labels = BTreeMap<String, String>;

/// Annotations are arbitrary key-value metadata.
pub type Annotations = BTreeMap<String, String>;

// ── Time Utilities ────────────────────────────────────────────────────────

/// Get current time as RFC3339 string.
pub fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();

    // Simple RFC3339 format without external crate
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Approximate date from days since epoch
    let (year, month, day) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        year, month, day, hours, minutes, seconds, nanos)
}

/// Get current time as epoch milliseconds.
pub fn now_epoch_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Parse RFC3339 timestamp to seconds since epoch.
pub fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    // Simple parser: "2026-06-11T10:00:00Z" or "2026-06-11T10:00:00.000Z"
    let s = s.trim_end_matches('Z');
    let parts: Vec<&str> = s.split(['T', '-', ':', '.']).collect();
    if parts.len() < 6 { return None; }

    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    let hour: u32 = parts[3].parse().ok()?;
    let min: u32 = parts[4].parse().ok()?;
    let sec: u32 = parts[5].parse().ok()?;

    let days = ymd_to_days(year, month, day);
    let secs = days * 86400 + (hour as u64) * 3600 + (min as u64) * 60 + (sec as u64);
    Some(secs as i64)
}

/// Format epoch seconds as human-readable age (e.g., "5m", "2h", "3d").
pub fn age_from_epoch_secs(secs: i64) -> String {
    let now = now_epoch_ms() / 1000;
    let diff = now - secs;
    if diff < 0 { return "<unknown>".into(); }
    if diff < 60 { return format!("{}s", diff); }
    if diff < 3600 { return format!("{}m", diff / 60); }
    if diff < 86400 { return format!("{}h", diff / 3600); }
    format!("{}d", diff / 86400)
}

// ── Date Helpers ──────────────────────────────────────────────────────────

/// Convert days since epoch to year/month/day.
fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    // Unix epoch starts at 1970-01-01
    let remaining = days as i64 + 719163; // days from year 0 to 1970-01-01

    // Approximate year
    let year = (remaining as f64 / 365.2425).floor() as i64;
    let mut day_of_year = remaining - (year as f64 * 365.2425) as i64;
    if day_of_year < 0 { day_of_year += 365; }

    let leap = is_leap_year(year as u32);
    let (month, day) = day_of_year_to_md(day_of_year as u32, leap);
    (year as u32, month, day)
}

/// Convert year/month/day to days since epoch.
fn ymd_to_days(year: u32, month: u32, day: u32) -> u64 {
    let y = year as i64 - 1;
    let m = month as i64;
    let d = day as i64;

    let days = 365 * y + y / 4 - y / 100 + y / 400
        + (367 * m - 362) / 12
        + if m > 2 { if is_leap_year(year) { -1 } else { -2 } } else { 0 }
        + d - 719163;
    days as u64
}

fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn day_of_year_to_md(day_of_year: u32, leap: bool) -> (u32, u32) {
    let days_in_month = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut remaining = day_of_year;
    for (i, &days) in days_in_month.iter().enumerate() {
        if remaining < days {
            return ((i + 1) as u32, remaining + 1);
        }
        remaining -= days;
    }
    (12, remaining + 1)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantity_as_bytes() {
        assert_eq!(Quantity("128Mi".into()).as_bytes(), Some(128 * 1024 * 1024));
        assert_eq!(Quantity("1Gi".into()).as_bytes(), Some(1024 * 1024 * 1024));
        assert_eq!(Quantity("500".into()).as_bytes(), Some(500));
    }

    #[test]
    fn test_quantity_as_cpu() {
        assert_eq!(Quantity("500m".into()).as_cpu_cores(), Some(0.5));
        assert_eq!(Quantity("2".into()).as_cpu_cores(), Some(2.0));
    }

    #[test]
    fn test_now_rfc3339() {
        let t = now_rfc3339();
        assert!(t.ends_with('Z'));
        assert!(t.contains('T'));
    }

    #[test]
    fn test_now_epoch_ms() {
        let ms = now_epoch_ms();
        assert!(ms > 1700000000000); // After 2023
    }
}
