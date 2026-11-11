//! Formatting and small helpers shared by all views.

/// `m:ss.d` (or `h:mm:ss.d` above an hour).
pub fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let tenths = (secs * 10.0).floor() as u64;
    let (t, s) = (tenths % 10, (tenths / 10) % 60);
    let m = (tenths / 600) % 60;
    let h = tenths / 36_000;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{t}")
    } else {
        format!("{m}:{s:02}.{t}")
    }
}

/// Whole seconds: `m:ss` / `h:mm:ss` (clocks).
pub fn fmt_clock(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Local wall clock `HH:MM:SS`.
pub fn wall_clock() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

pub fn lin_to_db(lin: f32) -> f32 {
    if lin <= 0.000_001 {
        -120.0
    } else {
        20.0 * lin.log10()
    }
}

pub fn fmt_db(db: f32) -> String {
    if db <= -119.9 {
        "-∞ dB".into()
    } else {
        format!("{db:+.1} dB")
    }
}

/// Parse `1.5`, `1:30`, `1:02:03.5` into seconds.
pub fn parse_time(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for part in t.split(':') {
        let v: f64 = part.trim().replace(',', ".").parse().ok()?;
        if v.is_nan() || v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_time(0.0), "0:00.0");
        assert_eq!(fmt_time(83.27), "1:23.2");
        assert_eq!(fmt_time(3725.0), "1:02:05.0");
        assert_eq!(fmt_clock(3725.9), "1:02:05");
        assert_eq!(fmt_clock(59.0), "00:59");
        assert_eq!(fmt_db(-120.0), "-∞ dB");
        assert_eq!(fmt_db(-6.04), "-6.0 dB");
        assert_eq!(wall_clock().len(), 8);
        assert!((lin_to_db(0.5) + 6.02).abs() < 0.01);
    }

    #[test]
    fn parses() {
        assert_eq!(parse_time("1.5"), Some(1.5));
        assert_eq!(parse_time("1:30"), Some(90.0));
        assert_eq!(parse_time("1:02:03,5"), Some(3723.5));
        assert_eq!(parse_time("x"), None);
        assert_eq!(parse_time("-1"), None);
        assert_eq!(parse_time(""), None);
    }
}
