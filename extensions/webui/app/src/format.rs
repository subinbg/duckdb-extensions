//! Numbers and durations, spelled the way a person reads them.

/// Group the digits of a count in threes: 1234567 becomes 1,234,567.
pub fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let groups: Vec<&str> =
        digits.as_bytes().rchunks(3).rev().map(|group| std::str::from_utf8(group).unwrap()).collect();
    groups.join(",")
}

/// Spell a count in a few characters: 950, 12K, 4.1M, 600M.
pub fn compact(count: u64) -> String {
    let scaled = |value: f64, unit: &str| match value < 10.0 {
        true => format!("{value:.1}{unit}"),
        false => format!("{value:.0}{unit}"),
    };
    match count {
        0..1000 => count.to_string(),
        1000..1_000_000 => scaled(count as f64 / 1e3, "K"),
        1_000_000..1_000_000_000 => scaled(count as f64 / 1e6, "M"),
        _ => scaled(count as f64 / 1e9, "B"),
    }
}

/// Spell a duration: 164 ms, 3.9 s, 1 m 36 s.
pub fn duration(milliseconds: u64) -> String {
    match milliseconds {
        0..1000 => format!("{milliseconds} ms"),
        1000..60_000 => format!("{:.1} s", milliseconds as f64 / 1000.0),
        _ => format!("{} m {} s", milliseconds / 60_000, milliseconds % 60_000 / 1000),
    }
}

/// Spell a share of a whole as a percentage; nothing when the whole is empty.
pub fn percent(part: u64, whole: u64) -> String {
    match whole {
        0 => String::new(),
        whole => format!("{:.1}%", part as f64 * 100.0 / whole as f64),
    }
}
