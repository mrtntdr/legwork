use crate::geo::MapTransform;
use crate::model::{CalibrationPoint, Track};
use egui::Color32;

/// Distinct solid route colors, cycled as athletes are added.
pub const ATHLETE_COLORS: [Color32; 8] = [
    Color32::from_rgb(230, 60, 60),   // red
    Color32::from_rgb(70, 120, 250),  // blue
    Color32::from_rgb(60, 190, 90),   // green
    Color32::from_rgb(245, 160, 40),  // orange
    Color32::from_rgb(170, 90, 240),  // purple
    Color32::from_rgb(40, 200, 220),  // cyan
    Color32::from_rgb(235, 80, 200),  // magenta
    Color32::from_rgb(170, 210, 60),  // yellow-green
];

/// One loaded runner: their track, per-athlete georeferencing, and display state.
/// The map image and the course (controls) are shared across athletes; each
/// athlete carries their own calibration and meters→pixels transform so GPS
/// offsets between devices can be corrected independently.
pub struct Athlete {
    pub name: String,
    pub color: Color32,
    pub visible: bool,
    pub track: Track,
    pub track_name: String,
    pub track_bytes: Vec<u8>,
    /// Track points through the app's shared projection (meters, y-down).
    pub projected: Vec<(f64, f64)>,
    /// Per-segment pace (sec/km), length == points-1.
    pub seg_metric: Vec<f64>,
    pub calibration: Vec<CalibrationPoint>,
    pub transform: Option<MapTransform>,
    /// Per shared control (course order): this athlete's matched waypoint index,
    /// `None` where the route never passes near the control.
    pub matched: Vec<Option<usize>>,
}

impl Athlete {
    /// Leg boundaries for this athlete: implicit start, each control's matched
    /// waypoint, implicit finish. Empty for an empty track.
    pub fn boundaries(&self) -> Vec<Option<usize>> {
        if self.track.is_empty() {
            return Vec::new();
        }
        let mut b = Vec::with_capacity(self.matched.len() + 2);
        b.push(Some(0));
        b.extend(self.matched.iter().copied());
        b.push(Some(self.track.len() - 1));
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Waypoint;

    fn athlete_with(points: usize, matched: Vec<Option<usize>>) -> Athlete {
        Athlete {
            name: "a".into(),
            color: ATHLETE_COLORS[0],
            visible: true,
            track: Track {
                points: (0..points)
                    .map(|i| Waypoint {
                        lat: 59.0 + i as f64 * 0.001,
                        lon: 18.0,
                        ..Waypoint::default()
                    })
                    .collect(),
            },
            track_name: "a.gpx".into(),
            track_bytes: Vec::new(),
            projected: Vec::new(),
            seg_metric: Vec::new(),
            calibration: Vec::new(),
            transform: None,
            matched,
        }
    }

    #[test]
    fn boundaries_wrap_matches_with_start_and_finish() {
        let a = athlete_with(10, vec![Some(3), None, Some(7)]);
        assert_eq!(
            a.boundaries(),
            vec![Some(0), Some(3), None, Some(7), Some(9)]
        );
    }

    #[test]
    fn boundaries_of_empty_track_are_empty() {
        let a = athlete_with(0, vec![Some(1)]);
        assert!(a.boundaries().is_empty());
    }
}
