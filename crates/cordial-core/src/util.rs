//! Small shared helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Truncates `text` to at most `max` characters on a char boundary, appending a
/// short marker when content was dropped.
pub fn truncate_chars(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    for (count, ch) in text.chars().enumerate() {
        if count >= max {
            out.push_str("\n\n[cut: content was longer]");
            return out;
        }
        out.push(ch);
    }
    out
}

/// Collapses whitespace runs into single spaces and trims.
pub fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// Formats a byte count the way the download UI shows it.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_char_boundaries() {
        let text = "héllo wörld";
        let out = truncate_chars(text, 3);
        assert!(out.starts_with("hél"));
        assert!(out.contains("cut"));
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(collapse_whitespace("  a \n\t b  "), "a b");
    }

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_100_000_000), "1.0 GB");
    }
}

/// Civil date (YYYY-MM-DD) for a Unix timestamp in seconds.
///
/// Implemented directly so the harness does not need a calendar crate.
pub fn iso_date(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Today's date in the machine's local timezone, as YYYY-MM-DD.
pub fn iso_date_now() -> String {
    // Respect the local offset by shifting the UTC instant the way the OS does.
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso_date(now + local_offset_seconds())
}

/// Best-effort local UTC offset.
fn local_offset_seconds() -> i64 {
    #[cfg(unix)]
    {
        // `date +%z` is portable across macOS and Linux and avoids a libc binding.
        if let Ok(output) = std::process::Command::new("date").arg("+%z").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            let text = text.trim();
            if text.len() >= 5 {
                let sign = if text.starts_with('-') { -1 } else { 1 };
                let hours: i64 = text[1..3].parse().unwrap_or(0);
                let minutes: i64 = text[3..5].parse().unwrap_or(0);
                return sign * (hours * 3600 + minutes * 60);
            }
        }
        0
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Howard Hinnant's days-to-civil conversion.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod date_tests {
    use super::*;

    #[test]
    fn epoch_is_the_first_of_january_1970() {
        assert_eq!(iso_date(0), "1970-01-01");
    }

    #[test]
    fn known_dates_are_correct() {
        assert_eq!(iso_date(1_700_000_000), "2023-11-14");
        assert_eq!(iso_date(951_782_400), "2000-02-29");
    }
}
