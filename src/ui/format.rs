//! Text formatting shared by every screen: n/a handling, units, sparklines and status levels.

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Level {
    Ok,
    Warn,
    Crit,
    Unknown,
}

pub fn na<T>(v: Option<T>, f: impl Fn(T) -> String) -> String {
    v.map(f).unwrap_or_else(|| "n/a".to_string())
}

pub fn bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let exp = ((b as f64).log2() / 10.0).floor().min(4.0) as usize;
    format!("{:.1} {}", b as f64 / 1024f64.powi(exp as i32), UNITS[exp])
}

/// Thousands separators on the integer part, `digits` decimals.
pub fn num(f: f64, digits: usize) -> String {
    let s = format!("{f:.digits$}");
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(i, f)| (i, f));
    let (sign, int) = int.strip_prefix('-').map_or(("", int), |i| ("-", i));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if frac.is_empty() { format!("{sign}{out}") } else { format!("{sign}{out}.{frac}") }
}

pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{:02}s", secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Min–max scales `values` to 125..=1000 for the Sparkline widget (max 1000). 125 is one eighth of the scale,
/// so the minimum still draws as the smallest bar instead of a blank; `None` stays a gap, a flat series is all 125.
pub fn normalize(values: &[Option<f32>]) -> Vec<Option<u64>> {
    let present = values.iter().flatten();
    let max = present.clone().fold(f32::MIN, |a, &b| a.max(b));
    let min = present.fold(f32::MAX, |a, &b| a.min(b));
    let span = max - min;
    values
        .iter()
        .map(|v| v.map(|v| if span <= 0.0 { 125 } else { (125.0 + ((v - min) / span) * 875.0).round() as u64 }))
        .collect()
}

/// Squeezes `values` into `width` buckets keeping each bucket's max, so spikes survive a long window.
pub fn downsample(values: &[Option<f32>], width: usize) -> Vec<Option<f32>> {
    if values.len() <= width || width == 0 {
        return values.to_vec();
    }
    (0..width)
        .map(|i| {
            let (a, b) = (i * values.len() / width, (i + 1) * values.len() / width);
            values[a..b.max(a + 1)].iter().flatten().copied().reduce(f32::max)
        })
        .collect()
}

/// `ratio` 0..=1 as a fixed-width block bar: `████░░░░` (or `####....` in ASCII mode).
pub fn bar(ratio: f64, width: usize, ascii: bool) -> String {
    let filled = (ratio.clamp(0.0, 1.0) * width as f64).round() as usize;
    let (on, off) = if ascii { ('#', '.') } else { ('█', '░') };
    std::iter::repeat_n(on, filled).chain(std::iter::repeat_n(off, width - filled)).collect()
}

pub fn level(v: Option<f64>, warn: f64, crit: f64) -> Level {
    match v {
        None => Level::Unknown,
        Some(v) if v >= crit => Level::Crit,
        Some(v) if v >= warn => Level::Warn,
        Some(_) => Level::Ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_formats() {
        assert_eq!(bytes(17_079_123_968), "15.9 GB");
        assert_eq!(bytes(536_870_912), "512.0 MB");
        assert_eq!(bytes(0), "0 B");
    }

    #[test]
    fn uptime_formats() {
        assert_eq!(uptime(45), "45s");
        assert_eq!(uptime(3 * 3600 + 12 * 60), "3h12m");
        assert_eq!(uptime(2 * 86400 + 3 * 3600), "2d 3h");
    }

    #[test]
    fn normalize_min_max_to_thousand() {
        assert_eq!(normalize(&[Some(0.0), Some(0.5), Some(1.0)]), vec![Some(125), Some(563), Some(1000)]);
        assert_eq!(normalize(&[None, Some(1.0)]), vec![None, Some(125)]);
        assert_eq!(normalize(&[Some(3.0), Some(3.0)]), vec![Some(125), Some(125)]);
    }

    #[test]
    fn downsample_keeps_bucket_max() {
        let v = [Some(1.0), Some(5.0), Some(2.0), Some(3.0)];
        assert_eq!(downsample(&v, 2), vec![Some(5.0), Some(3.0)]);
        assert_eq!(downsample(&[None, None, Some(1.0)], 1), vec![Some(1.0)]);
        assert_eq!(downsample(&v, 8), v.to_vec());
    }

    #[test]
    fn level_thresholds() {
        assert_eq!(level(Some(79.9), 80.0, 95.0), Level::Ok);
        assert_eq!(level(Some(80.0), 80.0, 95.0), Level::Warn);
        assert_eq!(level(Some(95.0), 80.0, 95.0), Level::Crit);
        assert_eq!(level(None, 80.0, 95.0), Level::Unknown);
    }

    #[test]
    fn na_and_num() {
        assert_eq!(na(None::<f64>, |v| num(v, 1)), "n/a");
        assert_eq!(na(Some(1234.56), |v| num(v, 1)), "1,234.6");
        assert_eq!(num(7.0, 0), "7");
    }
}
