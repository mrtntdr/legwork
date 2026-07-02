use crate::model::Track;

/// Computed metrics for one leg (between two consecutive controls).
#[derive(Clone, Debug)]
#[allow(dead_code)] // from_index/to_index/straight_distance are part of the leg record
pub struct Leg {
    pub from_index: usize,
    pub to_index: usize,
    /// Leg duration in seconds, if timestamps are present.
    pub duration_secs: Option<f64>,
    /// Distance actually run along the track, in meters.
    pub route_length: f64,
    /// Straight-line ("crow flies") distance, in meters.
    pub straight_distance: f64,
    /// Extra distance run vs the straight line, as a percentage.
    pub detour_pct: f64,
    /// Pace in seconds per km, if duration is available.
    pub pace_s_per_km: Option<f64>,
}

/// Build the ordered list of control indices from the placed controls.
/// Always includes the track start and finish as implicit controls.
pub fn control_indices(track: &Track, controls: &[usize]) -> Vec<usize> {
    if track.is_empty() {
        return Vec::new();
    }
    let last = track.len() - 1;
    let mut idx: Vec<usize> = std::iter::once(0)
        .chain(controls.iter().copied())
        .chain(std::iter::once(last))
        .filter(|&i| i <= last)
        .collect();
    idx.sort_unstable();
    idx.dedup();
    idx
}

/// Compute per-leg metrics for the given controls.
pub fn legs(track: &Track, controls: &[usize]) -> Vec<Leg> {
    let controls = control_indices(track, controls);
    controls
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let route_length = track.route_length(a, b);
            let straight = track.straight_distance(a, b);
            let detour_pct = if straight > 1e-6 {
                (route_length / straight - 1.0) * 100.0
            } else {
                0.0
            };
            let duration = track.duration_between(a, b);
            let pace = duration.and_then(|d| {
                if route_length > 1.0 {
                    Some(d / (route_length / 1000.0))
                } else {
                    None
                }
            });
            Leg {
                from_index: a,
                to_index: b,
                duration_secs: duration,
                route_length,
                straight_distance: straight,
                detour_pct,
                pace_s_per_km: pace,
            }
        })
        .collect()
}

/// Format seconds as `m:ss` (or `h:mm:ss`).
pub fn fmt_duration(secs: f64) -> String {
    let total = secs.round() as i64;
    let (sign, total) = if total < 0 {
        ("-", -total)
    } else {
        ("", total)
    };
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{sign}{h}:{m:02}:{s:02}")
    } else {
        format!("{sign}{m}:{s:02}")
    }
}

/// Format a pace (seconds per km) as `m:ss /km`.
pub fn fmt_pace(s_per_km: f64) -> String {
    format!("{} /km", fmt_duration(s_per_km))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Waypoint;
    use chrono::{TimeZone, Utc};

    fn wp(lat: f64, lon: f64, secs: i64) -> Waypoint {
        Waypoint {
            time: Some(Utc.timestamp_opt(1_600_000_000 + secs, 0).unwrap()),
            lat,
            lon,
            ele: None,
            hr: None,
        }
    }

    #[test]
    fn straight_leg_has_zero_detour() {
        // Three collinear points along a meridian.
        let track = Track {
            points: vec![
                wp(59.0, 18.0, 0),
                wp(59.001, 18.0, 30),
                wp(59.002, 18.0, 60),
            ],
        };
        let legs = legs(&track, &[]);
        assert_eq!(legs.len(), 1);
        assert!(
            legs[0].detour_pct.abs() < 0.5,
            "detour {}",
            legs[0].detour_pct
        );
        assert_eq!(legs[0].duration_secs, Some(60.0));
    }

    #[test]
    fn one_control_creates_two_legs() {
        let track = Track {
            points: vec![
                wp(59.0, 18.0, 0),
                wp(59.001, 18.0, 30),
                wp(59.002, 18.0, 60),
            ],
        };
        let legs = legs(&track, &[1]);
        assert_eq!(legs.len(), 2);
    }

    #[test]
    fn control_indices_include_endpoints_sorted_and_deduped() {
        let track = Track {
            points: (0..5i64)
                .map(|i| wp(59.0 + i as f64 * 0.001, 18.0, i * 10))
                .collect(),
        };
        // Unsorted, duplicated, and including the implicit endpoints.
        assert_eq!(control_indices(&track, &[3, 1, 3, 0, 4]), vec![0, 1, 3, 4]);
        // Out-of-range controls are dropped.
        assert_eq!(control_indices(&track, &[99]), vec![0, 4]);
        // Empty track yields no controls at all.
        assert!(control_indices(&Track::default(), &[1]).is_empty());
    }

    #[test]
    fn detour_is_positive_for_a_dogleg() {
        // Out-and-up dogleg: route via a corner is longer than the straight line.
        let track = Track {
            points: vec![
                wp(59.0, 18.0, 0),
                wp(59.0, 18.002, 30),
                wp(59.001, 18.002, 60),
            ],
        };
        let legs = legs(&track, &[]);
        assert_eq!(legs.len(), 1);
        assert!(legs[0].detour_pct > 10.0, "detour {}", legs[0].detour_pct);
        assert!(legs[0].route_length > legs[0].straight_distance);
    }

    #[test]
    fn missing_timestamps_leave_duration_and_pace_empty() {
        let no_time = Waypoint {
            lat: 59.001,
            lon: 18.0,
            ..Waypoint::default()
        };
        let track = Track {
            points: vec![wp(59.0, 18.0, 0), no_time],
        };
        let legs = legs(&track, &[]);
        assert_eq!(legs[0].duration_secs, None);
        assert_eq!(legs[0].pace_s_per_km, None);
    }

    #[test]
    fn zero_length_leg_has_no_pace() {
        // Two samples at the same spot: duration exists but pace is meaningless.
        let track = Track {
            points: vec![wp(59.0, 18.0, 0), wp(59.0, 18.0, 30)],
        };
        let legs = legs(&track, &[]);
        assert_eq!(legs[0].duration_secs, Some(30.0));
        assert_eq!(legs[0].pace_s_per_km, None);
        assert_eq!(legs[0].detour_pct, 0.0); // degenerate straight line guarded
    }

    #[test]
    fn pace_is_duration_over_distance() {
        let track = Track {
            points: vec![wp(59.0, 18.0, 0), wp(59.009, 18.0, 300)], // ~1 km in 5 min
        };
        let legs = legs(&track, &[]);
        let pace = legs[0].pace_s_per_km.unwrap();
        assert!((pace - 300.0).abs() < 5.0, "pace {pace}");
    }

    #[test]
    fn fmt_duration_formats() {
        assert_eq!(fmt_duration(0.0), "0:00");
        assert_eq!(fmt_duration(65.0), "1:05");
        assert_eq!(fmt_duration(59.6), "1:00"); // rounds
        assert_eq!(fmt_duration(3665.0), "1:01:05");
        assert_eq!(fmt_duration(-65.0), "-1:05");
    }

    #[test]
    fn fmt_pace_appends_unit() {
        assert_eq!(fmt_pace(330.0), "5:30 /km");
    }
}
