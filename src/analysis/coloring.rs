use crate::model::Track;
use egui::Color32;

/// Range of a metric used to normalize colors, with a small guard against zero span.
#[derive(Clone, Copy, Debug)]
pub struct MetricRange {
    pub min: f64,
    pub max: f64,
}

impl MetricRange {
    pub fn normalize(&self, v: f64) -> f64 {
        if (self.max - self.min).abs() < 1e-9 {
            0.5
        } else {
            ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }
}

/// Per-segment pace values (length == points-1), in seconds per km.
pub fn segment_metric(track: &Track) -> Vec<f64> {
    track
        .segment_speeds()
        .into_iter()
        .map(|s| if s > 0.01 { 1000.0 / s } else { f64::INFINITY })
        .collect()
}

/// Robust color range from the 5th/95th percentiles, ignoring non-finite values.
pub fn auto_range(values: &[f64]) -> MetricRange {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return MetricRange { min: 0.0, max: 1.0 };
    }
    v.sort_unstable_by(f64::total_cmp);
    let pick = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    MetricRange {
        min: pick(0.05),
        max: pick(0.95),
    }
}

/// Map "quickness" (0 = slow, 1 = quick) to a color: blue (slow) → red (quick).
pub fn quickness_color(quickness: f64, alpha: u8) -> Color32 {
    // Hue sweeps 240° (blue) down to 0° (red) as quickness rises.
    let hue = 240.0 * (1.0 - quickness.clamp(0.0, 1.0));
    let (r, g, b) = hsv_to_rgb(hue, 0.85, 0.95);
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// Map a pace value (seconds per km) to a semi-transparent color: slow = blue,
/// quick = red. A higher pace value is slower.
pub fn color_for(value: f64, range: MetricRange) -> Color32 {
    if !value.is_finite() {
        return Color32::from_rgba_unmultiplied(128, 128, 128, 140);
    }
    quickness_color(1.0 - range.normalize(value), 190)
}

/// HSV (h in degrees, s/v in 0..1) to 8-bit RGB.
pub fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let hp = (h / 60.0).clamp(0.0, 6.0);
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to8 = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to8(r1), to8(g1), to8(b1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_is_blue_quick_is_red() {
        let range = MetricRange {
            min: 0.0,
            max: 10.0,
        };
        let slow = color_for(10.0, range); // high pace = slow = blue
        let quick = color_for(0.0, range);
        assert!(slow.b() > slow.r(), "slow should be blue: {slow:?}");
        assert!(quick.r() > quick.b(), "quick should be red: {quick:?}");
    }

    #[test]
    fn normalize_clamps_and_handles_degenerate_range() {
        let range = MetricRange { min: 2.0, max: 4.0 };
        assert_eq!(range.normalize(3.0), 0.5);
        assert_eq!(range.normalize(-100.0), 0.0);
        assert_eq!(range.normalize(100.0), 1.0);
        // Zero-span range maps everything to the middle instead of dividing by 0.
        let flat = MetricRange { min: 5.0, max: 5.0 };
        assert_eq!(flat.normalize(5.0), 0.5);
        assert_eq!(flat.normalize(99.0), 0.5);
    }

    #[test]
    fn auto_range_uses_percentiles_and_ignores_non_finite() {
        assert_eq!(auto_range(&[]).min, 0.0);
        assert_eq!(auto_range(&[]).max, 1.0);
        // Infinities (stationary segments) must not blow up the range.
        let mut values: Vec<f64> = (0..=100).map(f64::from).collect();
        values.push(f64::INFINITY);
        values.push(f64::NAN);
        let r = auto_range(&values);
        assert_eq!(r.min, 5.0);
        assert_eq!(r.max, 95.0);
    }

    #[test]
    fn segment_metric_is_pace_with_infinity_when_stationary() {
        use crate::model::Waypoint;
        use chrono::{TimeZone, Utc};
        let wp = |lat: f64, secs: i64| Waypoint {
            time: Some(Utc.timestamp_opt(1_600_000_000 + secs, 0).unwrap()),
            lat,
            lon: 18.0,
            ele: None,
            hr: None,
        };
        // Moving segment then a stationary one (same position => speed 0).
        let track = Track {
            points: vec![wp(59.0, 0), wp(59.001, 10), wp(59.001, 20)],
        };
        let m = segment_metric(&track);
        assert_eq!(m.len(), 2);
        assert!((m[0] - 1000.0 / 11.12).abs() < 1.0, "pace {}", m[0]); // ~90 s/km
        assert!(m[1].is_infinite());
    }

    #[test]
    fn non_finite_pace_is_gray() {
        let range = MetricRange { min: 0.0, max: 1.0 };
        let c = color_for(f64::NAN, range);
        // Neutral gray: all channels equal (premultiplied by egui internally).
        assert!(c.r() == c.g() && c.g() == c.b(), "not gray: {c:?}");
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0)); // red
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), (0, 255, 0)); // green
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), (0, 0, 255)); // blue
        assert_eq!(hsv_to_rgb(0.0, 0.0, 1.0), (255, 255, 255)); // no saturation = white
        assert_eq!(hsv_to_rgb(180.0, 1.0, 0.0), (0, 0, 0)); // no value = black
    }
}
