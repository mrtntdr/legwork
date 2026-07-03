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

/// Per-leg metrics between consecutive boundaries (start, matched controls…,
/// finish). One entry per consecutive pair; `None` where either endpoint is
/// unmatched or out of order.
pub fn legs_between(track: &Track, boundaries: &[Option<usize>]) -> Vec<Option<Leg>> {
    boundaries
        .windows(2)
        .map(|w| {
            let (Some(a), Some(b)) = (w[0], w[1]) else {
                return None;
            };
            if a > b || b >= track.len() {
                return None;
            }
            Some(make_leg(track, a, b))
        })
        .collect()
}

fn make_leg(track: &Track, a: usize, b: usize) -> Leg {
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

    /// Boundaries spanning the whole track with no controls in between.
    fn full(track: &Track) -> Vec<Option<usize>> {
        vec![Some(0), Some(track.len() - 1)]
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
        let legs = legs_between(&track, &full(&track));
        assert_eq!(legs.len(), 1);
        let leg = legs[0].as_ref().unwrap();
        assert!(leg.detour_pct.abs() < 0.5, "detour {}", leg.detour_pct);
        assert_eq!(leg.duration_secs, Some(60.0));
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
        let legs = legs_between(&track, &[Some(0), Some(1), Some(2)]);
        assert_eq!(legs.len(), 2);
        assert!(legs.iter().all(Option::is_some));
    }

    #[test]
    fn unmatched_boundaries_blank_adjacent_legs_only() {
        let track = Track {
            points: (0..5i64)
                .map(|i| wp(59.0 + i as f64 * 0.001, 18.0, i * 10))
                .collect(),
        };
        // Middle control unmatched: legs 1 and 2 are None, leg 3 survives.
        let legs = legs_between(&track, &[Some(0), Some(1), None, Some(3), Some(4)]);
        assert_eq!(legs.len(), 4);
        assert!(legs[0].is_some());
        assert!(legs[1].is_none());
        assert!(legs[2].is_none());
        assert!(legs[3].is_some());
    }

    #[test]
    fn out_of_order_or_out_of_range_boundaries_yield_no_leg() {
        let track = Track {
            points: vec![wp(59.0, 18.0, 0), wp(59.001, 18.0, 30)],
        };
        let reversed = legs_between(&track, &[Some(1), Some(0)]);
        assert_eq!(reversed.len(), 1);
        assert!(reversed[0].is_none());
        assert!(legs_between(&track, &[Some(0), Some(99)])[0].is_none());
        assert!(legs_between(&track, &[]).is_empty());
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
        let legs = legs_between(&track, &full(&track));
        let leg = legs[0].as_ref().unwrap();
        assert!(leg.detour_pct > 10.0, "detour {}", leg.detour_pct);
        assert!(leg.route_length > leg.straight_distance);
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
        let legs = legs_between(&track, &full(&track));
        let leg = legs[0].as_ref().unwrap();
        assert_eq!(leg.duration_secs, None);
        assert_eq!(leg.pace_s_per_km, None);
    }

    #[test]
    fn zero_length_leg_has_no_pace() {
        // Two samples at the same spot: duration exists but pace is meaningless.
        let track = Track {
            points: vec![wp(59.0, 18.0, 0), wp(59.0, 18.0, 30)],
        };
        let legs = legs_between(&track, &full(&track));
        let leg = legs[0].as_ref().unwrap();
        assert_eq!(leg.duration_secs, Some(30.0));
        assert_eq!(leg.pace_s_per_km, None);
        assert_eq!(leg.detour_pct, 0.0); // degenerate straight line guarded
    }

    #[test]
    fn pace_is_duration_over_distance() {
        let track = Track {
            points: vec![wp(59.0, 18.0, 0), wp(59.009, 18.0, 300)], // ~1 km in 5 min
        };
        let legs = legs_between(&track, &full(&track));
        let pace = legs[0].as_ref().unwrap().pace_s_per_km.unwrap();
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
