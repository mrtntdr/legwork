use crate::analysis::leg::{Leg, legs_between};
use crate::model::Track;

/// One athlete's numbers for one leg of the comparison.
pub struct LegCell {
    pub leg: Option<Leg>,
    /// Elapsed seconds from the run start to this leg's end boundary. Computed
    /// directly against the start (not by summing legs), so one missed control
    /// blanks only its adjacent legs while the cumulative time recovers at the
    /// next matched control.
    pub cum_secs: Option<f64>,
}

/// One row of the leg comparison table.
pub struct LegRow {
    /// "S–1", "1–2", …, "3–F" (or "S–F" with no controls).
    pub label: String,
    /// One cell per compared athlete, in the order passed to `compare`.
    pub cells: Vec<LegCell>,
    /// Column index of the fastest leg among the comparable cells.
    pub best: Option<usize>,
    /// Column index of the cumulative-time leader at this leg's end.
    pub best_cum: Option<usize>,
}

/// Build the leg-by-leg comparison across athletes. Each entry is one athlete's
/// track plus their leg boundaries (start, one per shared control, finish). An
/// entry whose boundary count doesn't fit the shared course (e.g. an empty
/// track) gets blank cells throughout.
pub fn compare(entries: &[(&Track, &[Option<usize>])], n_controls: usize) -> Vec<LegRow> {
    let n_legs = n_controls + 1;
    let per_athlete: Vec<Vec<Option<Leg>>> = entries
        .iter()
        .map(|(track, b)| {
            if b.len() == n_controls + 2 {
                legs_between(track, b)
            } else {
                (0..n_legs).map(|_| None).collect()
            }
        })
        .collect();

    (0..n_legs)
        .map(|li| {
            let cells: Vec<LegCell> = entries
                .iter()
                .zip(&per_athlete)
                .map(|((track, b), legs)| LegCell {
                    leg: legs[li].clone(),
                    cum_secs: b
                        .get(li + 1)
                        .copied()
                        .flatten()
                        .and_then(|end| track.duration_between(0, end)),
                })
                .collect();
            let best = best_index(cells.iter().map(|c| c.leg.as_ref().and_then(|l| l.duration_secs)));
            let best_cum = best_index(cells.iter().map(|c| c.cum_secs));
            LegRow {
                label: leg_label(li, n_controls),
                cells,
                best,
                best_cum,
            }
        })
        .collect()
}

/// Index of the smallest present value, if any.
fn best_index(values: impl Iterator<Item = Option<f64>>) -> Option<usize> {
    values
        .enumerate()
        .filter_map(|(i, v)| v.map(|v| (i, v)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn leg_label(i: usize, n_controls: usize) -> String {
    let from = if i == 0 { "S".into() } else { i.to_string() };
    let to = if i == n_controls {
        "F".into()
    } else {
        (i + 1).to_string()
    };
    format!("{from}–{to}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Waypoint;
    use chrono::{TimeZone, Utc};

    /// A straight track north with one point every ~111 m, `secs` apart each.
    fn track(n: usize, secs: i64) -> Track {
        Track {
            points: (0..n)
                .map(|i| Waypoint {
                    time: Some(
                        Utc.timestamp_opt(1_600_000_000 + i as i64 * secs, 0)
                            .unwrap(),
                    ),
                    lat: 59.0 + i as f64 * 0.001,
                    lon: 18.0,
                    ele: None,
                    hr: None,
                })
                .collect(),
        }
    }

    #[test]
    fn best_leg_and_deltas_pick_the_faster_athlete() {
        let fast = track(5, 10); // 10 s per point
        let slow = track(5, 15); // 15 s per point
        let fb = [Some(0), Some(2), Some(4)];
        let sb = [Some(0), Some(2), Some(4)];
        let rows = compare(&[(&fast, &fb), (&slow, &sb)], 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "S–1");
        assert_eq!(rows[1].label, "1–F");
        for row in &rows {
            assert_eq!(row.best, Some(0));
            assert_eq!(row.best_cum, Some(0));
        }
        let d0 = rows[0].cells[0].leg.as_ref().unwrap().duration_secs;
        let d1 = rows[0].cells[1].leg.as_ref().unwrap().duration_secs;
        assert_eq!(d0, Some(20.0));
        assert_eq!(d1, Some(30.0));
    }

    #[test]
    fn missed_control_blanks_legs_but_cumulative_recovers() {
        let a = track(7, 10);
        let b = track(7, 10);
        let ab = [Some(0), Some(2), Some(4), Some(6)];
        let bb = [Some(0), None, Some(4), Some(6)]; // athlete b missed control 1
        let rows = compare(&[(&a, &ab), (&b, &bb)], 2);
        assert_eq!(rows.len(), 3);
        // Legs adjacent to the miss are blank for athlete b…
        assert!(rows[0].cells[1].leg.is_none());
        assert!(rows[1].cells[1].leg.is_none());
        assert!(rows[2].cells[1].leg.is_some());
        // …and its cumulative time is back at the next matched control.
        assert_eq!(rows[0].cells[1].cum_secs, None);
        assert_eq!(rows[1].cells[1].cum_secs, Some(40.0));
        // Best-of-leg ignores the blank cell.
        assert_eq!(rows[0].best, Some(0));
    }

    #[test]
    fn single_athlete_compares_against_itself() {
        let t = track(3, 10);
        let b = [Some(0), Some(2)];
        let rows = compare(&[(&t, &b)], 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "S–F");
        assert_eq!(rows[0].best, Some(0));
    }

    #[test]
    fn mismatched_boundary_count_blanks_the_athlete() {
        let a = track(5, 10);
        let empty = Track::default();
        let ab = [Some(0), Some(2), Some(4)];
        let eb: [Option<usize>; 0] = []; // empty track has no boundaries
        let rows = compare(&[(&a, &ab), (&empty, &eb)], 1);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].cells[1].leg.is_none());
        assert!(rows[0].cells[1].cum_secs.is_none());
        assert_eq!(rows[0].best, Some(0));
    }
}
