use crate::model::Track;
use chrono::{DateTime, Utc};

/// Which clock a replay runs on.
#[derive(Clone, Copy, PartialEq)]
pub enum ClockMode {
    /// Every athlete's clock zeroed at their own track start.
    MassStart,
    /// Actual recorded wall-clock time; athletes offset by their real start.
    RealTime,
    /// Everyone restarted together at the given leg index (0-based leg, where
    /// leg `li` runs from boundary `li` to boundary `li + 1`).
    Leg(usize),
}

/// One athlete's playback window: a track-relative span `[t0, t1]` (seconds from
/// the athlete's first timestamp) plus the global-clock offset `g0` at which that
/// span begins. The athlete's track-time for a global clock `g` is
/// `clamp(t0 + (g - g0), t0, t1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub t0: f64,
    pub t1: f64,
    pub g0: f64,
}

impl Window {
    /// The athlete's track-time (seconds from their first timestamp) at global
    /// clock `g`, clamped to the window so dots wait at the start/finish.
    pub fn track_time(&self, g: f64) -> f64 {
        (self.t0 + (g - self.g0)).clamp(self.t0, self.t1)
    }

    /// Duration of this window in seconds.
    pub fn duration(&self) -> f64 {
        (self.t1 - self.t0).max(0.0)
    }
}

/// Build an athlete's playback timeline: `(seconds_from_first_timestamp,
/// waypoint_index)` for every timestamped point, dropping points without a time
/// and any whose time goes backwards, so both columns are strictly increasing.
pub fn build_timeline(track: &Track) -> Vec<(f64, usize)> {
    let mut out: Vec<(f64, usize)> = Vec::new();
    let mut t0: Option<DateTime<Utc>> = None;
    for (i, p) in track.points.iter().enumerate() {
        let Some(t) = p.time else { continue };
        let start = *t0.get_or_insert(t);
        let secs = (t - start).num_milliseconds() as f64 / 1000.0;
        // Keep strictly increasing seconds; skip stalls/backwards timestamps.
        if out.last().is_none_or(|&(s, _)| secs > s) {
            out.push((secs, i));
        }
    }
    out
}

/// Interpolated projected-meters position at track-time `t`, lerping between the
/// surrounding waypoints. Clamps to the first/last sample outside the range.
pub fn position_at(
    timeline: &[(f64, usize)],
    projected: &[(f64, f64)],
    t: f64,
) -> Option<(f64, f64)> {
    if timeline.is_empty() {
        return None;
    }
    let at = |k: usize| projected.get(timeline[k].1).copied();
    if t <= timeline[0].0 {
        return at(0);
    }
    if t >= timeline[timeline.len() - 1].0 {
        return at(timeline.len() - 1);
    }
    // First sample strictly after `t`; interpolate from the one before it.
    let hi = timeline.partition_point(|&(s, _)| s <= t).max(1);
    let (s0, _) = timeline[hi - 1];
    let (s1, _) = timeline[hi];
    let (p0, p1) = (at(hi - 1)?, at(hi)?);
    let f = if s1 > s0 { (t - s0) / (s1 - s0) } else { 0.0 };
    Some((p0.0 + (p1.0 - p0.0) * f, p0.1 + (p1.1 - p0.1) * f))
}

/// Waypoint index of the last sample at or before track-time `t` (the tail head).
pub fn index_at(timeline: &[(f64, usize)], t: f64) -> Option<usize> {
    if timeline.is_empty() {
        return None;
    }
    let k = timeline.partition_point(|&(s, _)| s <= t);
    Some(timeline[k.saturating_sub(1)].1)
}

/// Track-time (seconds from the athlete's first timestamp) at a waypoint index,
/// via the timeline. `None` if the index isn't timestamped.
pub fn time_at_index(timeline: &[(f64, usize)], index: usize) -> Option<f64> {
    timeline
        .iter()
        .find(|&&(_, i)| i == index)
        .map(|&(s, _)| s)
}

/// Compute an athlete's playback window for a clock mode. `anchor` is the global
/// real-time reference (earliest start among animated athletes); only used for
/// `RealTime`. Returns `None` when the athlete can't participate (no timeline, or
/// an unmatched/untimed leg boundary).
pub fn window(
    timeline: &[(f64, usize)],
    boundaries: &[Option<usize>],
    mode: ClockMode,
    start_utc: Option<DateTime<Utc>>,
    anchor: Option<DateTime<Utc>>,
) -> Option<Window> {
    if timeline.is_empty() {
        return None;
    }
    let full_end = timeline[timeline.len() - 1].0;
    match mode {
        ClockMode::MassStart => Some(Window {
            t0: 0.0,
            t1: full_end,
            g0: 0.0,
        }),
        ClockMode::RealTime => {
            let (start, anchor) = (start_utc?, anchor?);
            let g0 = (start - anchor).num_milliseconds() as f64 / 1000.0;
            Some(Window {
                t0: 0.0,
                t1: full_end,
                g0,
            })
        }
        ClockMode::Leg(li) => {
            let from = *boundaries.get(li)?.as_ref()? ;
            let to = *boundaries.get(li + 1)?.as_ref()?;
            let t0 = time_at_index(timeline, from)?;
            let t1 = time_at_index(timeline, to)?;
            if t1 < t0 {
                return None;
            }
            // Re-zero: everyone restarts together at this leg's start control.
            Some(Window { t0, t1, g0: 0.0 })
        }
    }
}

/// Total length of the global timeline over a set of windows: the latest global
/// time any athlete is still moving.
pub fn total_span(windows: impl Iterator<Item = Window>) -> f64 {
    windows
        .map(|w| w.g0 + w.duration())
        .fold(0.0_f64, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Waypoint;
    use chrono::TimeZone;

    fn wp(lat: f64, secs: Option<i64>) -> Waypoint {
        Waypoint {
            time: secs.map(|s| Utc.timestamp_opt(1_600_000_000 + s, 0).unwrap()),
            lat,
            lon: 18.0,
            ele: None,
            hr: None,
        }
    }

    fn track(times: &[Option<i64>]) -> Track {
        Track {
            points: times
                .iter()
                .enumerate()
                .map(|(i, &s)| wp(59.0 + i as f64 * 0.001, s))
                .collect(),
        }
    }

    #[test]
    fn timeline_skips_untimed_and_backwards_points() {
        let t = track(&[Some(0), None, Some(10), Some(5), Some(20)]);
        let tl = build_timeline(&t);
        // index 1 (no time) and index 3 (backwards) dropped.
        assert_eq!(tl, vec![(0.0, 0), (10.0, 2), (20.0, 4)]);
    }

    #[test]
    fn timeline_zeroes_on_first_timestamp() {
        let t = track(&[None, Some(100), Some(130)]);
        let tl = build_timeline(&t);
        assert_eq!(tl, vec![(0.0, 1), (30.0, 2)]);
    }

    #[test]
    fn position_lerps_midway_and_clamps_at_ends() {
        let projected = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let tl = vec![(0.0, 0), (10.0, 1), (20.0, 2)];
        // Midway through the first segment.
        assert_eq!(position_at(&tl, &projected, 5.0), Some((5.0, 0.0)));
        // Exactly on a sample.
        assert_eq!(position_at(&tl, &projected, 10.0), Some((10.0, 0.0)));
        // Midway through the second segment.
        assert_eq!(position_at(&tl, &projected, 15.0), Some((10.0, 5.0)));
        // Clamp before start and after end.
        assert_eq!(position_at(&tl, &projected, -5.0), Some((0.0, 0.0)));
        assert_eq!(position_at(&tl, &projected, 99.0), Some((10.0, 10.0)));
    }

    #[test]
    fn index_at_finds_tail_head() {
        let tl = vec![(0.0, 0), (10.0, 3), (20.0, 7)];
        assert_eq!(index_at(&tl, -1.0), Some(0));
        assert_eq!(index_at(&tl, 0.0), Some(0));
        assert_eq!(index_at(&tl, 9.9), Some(0));
        assert_eq!(index_at(&tl, 10.0), Some(3));
        assert_eq!(index_at(&tl, 25.0), Some(7));
        assert_eq!(index_at(&[], 5.0), None);
    }

    #[test]
    fn mass_start_window_is_full_track_from_zero() {
        let tl = build_timeline(&track(&[Some(0), Some(10), Some(30)]));
        let w = window(&tl, &[], ClockMode::MassStart, None, None).unwrap();
        assert_eq!(w, Window { t0: 0.0, t1: 30.0, g0: 0.0 });
        assert_eq!(w.track_time(5.0), 5.0);
        assert_eq!(w.track_time(-2.0), 0.0); // clamped
        assert_eq!(w.track_time(999.0), 30.0);
    }

    #[test]
    fn real_time_window_offsets_by_start_difference() {
        let a = build_timeline(&track(&[Some(0), Some(30)])); // starts at +0
        let b = build_timeline(&track(&[Some(0), Some(30)])); // starts at +50
        let anchor = Utc.timestamp_opt(1_600_000_000, 0).unwrap();
        let a_start = Utc.timestamp_opt(1_600_000_000, 0).unwrap();
        let b_start = Utc.timestamp_opt(1_600_000_050, 0).unwrap();
        let wa = window(&a, &[], ClockMode::RealTime, Some(a_start), Some(anchor)).unwrap();
        let wb = window(&b, &[], ClockMode::RealTime, Some(b_start), Some(anchor)).unwrap();
        assert_eq!(wa.g0, 0.0);
        assert_eq!(wb.g0, 50.0);
        // b hasn't started at global t=25 → clamped at its own start position.
        assert_eq!(wb.track_time(25.0), 0.0);
        assert_eq!(wb.track_time(60.0), 10.0);
        assert_eq!(total_span([wa, wb].into_iter()), 80.0); // 50 + 30
    }

    #[test]
    fn leg_window_rezeroes_and_rejects_unmatched() {
        let tl = build_timeline(&track(&[Some(0), Some(10), Some(20), Some(30)]));
        // Leg from boundary index 1 (t=10) to boundary index 2 (t=20).
        let b = [Some(0), Some(1), Some(2), Some(3)];
        let w = window(&tl, &b, ClockMode::Leg(1), None, None).unwrap();
        assert_eq!(w, Window { t0: 10.0, t1: 20.0, g0: 0.0 });
        assert_eq!(w.duration(), 10.0);
        assert_eq!(w.track_time(0.0), 10.0); // starts at the leg's start control
        assert_eq!(w.track_time(5.0), 15.0);
        // Unmatched boundary → no window.
        let b2 = [Some(0), None, Some(2), Some(3)];
        assert!(window(&tl, &b2, ClockMode::Leg(1), None, None).is_none());
    }

    #[test]
    fn empty_timeline_has_no_window() {
        assert!(window(&[], &[], ClockMode::MassStart, None, None).is_none());
    }
}
