//! Pure formatting helpers.

/// `1536` → `1.5 KB`.
pub fn bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{value:.0} {}", units[unit])
    }
}

/// Whole-number percentage change, negative when smaller.
pub fn percent_change(before: u64, after: u64) -> i64 {
    if before == 0 {
        0
    } else {
        (((after as f64 - before as f64) / before as f64) * 100.0).round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_changes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(10 * 1024 * 1024), "10 MB");
        assert_eq!(percent_change(1000, 400), -60);
        assert_eq!(percent_change(0, 5), 0);
    }
}
