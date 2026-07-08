//! Drawn route options: measured stats for user-sketched polylines. Pure geometry
//! in image-pixel space; the meters conversion and control scores are supplied by
//! the caller (`App`), so this layer stays free of the map transform and egui.

use crate::geo::point_segment_dist;

/// Derived stats for one drawn route. Not persisted — recomputed from the route
/// geometry, the map's pixel→meters mapping, and the course.
#[derive(Clone, Debug, Default)]
pub struct RouteStats {
    /// Ground length in meters; `None` when the map can't be measured yet
    /// (no georeferencing and no calibrated athlete).
    pub length_m: Option<f64>,
    /// Indices of controls the route passes within the collection radius,
    /// in control order.
    pub collected: Vec<usize>,
    /// Sum of the collected controls' point values (rogaine scoring).
    pub points: u32,
}

/// Total length of a polyline in pixels.
pub fn polyline_len_px(pts: &[[f64; 2]]) -> f64 {
    pts.windows(2)
        .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
        .sum()
}

/// The point halfway along a polyline by cumulative length — for placing a label
/// or probing the local scale. `None` only for an empty polyline.
pub fn route_midpoint_px(pts: &[[f64; 2]]) -> Option<[f64; 2]> {
    match pts.len() {
        0 => return None,
        1 => return Some(pts[0]),
        _ => {}
    }
    let half = polyline_len_px(pts) / 2.0;
    if half <= 0.0 {
        return Some(pts[0]);
    }
    let mut acc = 0.0;
    for w in pts.windows(2) {
        let seg = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        if acc + seg >= half {
            let t = if seg > 1e-9 { (half - acc) / seg } else { 0.0 };
            return Some([
                w[0][0] + t * (w[1][0] - w[0][0]),
                w[0][1] + t * (w[1][1] - w[0][1]),
            ]);
        }
        acc += seg;
    }
    pts.last().copied()
}

/// Controls whose distance to the nearest point of the route polyline is within
/// `radius_px`, in control order. Both `route_px` and `controls` are image pixels.
pub fn collected_controls(
    route_px: &[[f64; 2]],
    controls: &[[f64; 2]],
    radius_px: f64,
) -> Vec<usize> {
    controls
        .iter()
        .enumerate()
        .filter(|&(_, &c)| within_route(route_px, c, radius_px))
        .map(|(i, _)| i)
        .collect()
}

fn within_route(route_px: &[[f64; 2]], c: [f64; 2], radius_px: f64) -> bool {
    match route_px {
        [] => false,
        [only] => ((only[0] - c[0]).powi(2) + (only[1] - c[1]).powi(2)).sqrt() <= radius_px,
        _ => route_px
            .windows(2)
            .any(|w| point_segment_dist(c, w[0], w[1]) <= radius_px),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_midpoint() {
        // Two segments: 50 px then 10 px, total 60 px.
        let pts = [[0.0, 0.0], [30.0, 40.0], [30.0, 50.0]];
        assert!((polyline_len_px(&pts) - 60.0).abs() < 1e-9);
        // Halfway (30 px) lands 0.6 along the first 50 px segment.
        assert_eq!(route_midpoint_px(&pts), Some([18.0, 24.0]));
        assert_eq!(route_midpoint_px(&[]), None);
        assert_eq!(route_midpoint_px(&[[5.0, 5.0]]), Some([5.0, 5.0]));
    }

    #[test]
    fn collects_controls_within_radius() {
        let route = [[0.0, 0.0], [100.0, 0.0]];
        let controls = [
            [0.0, 0.0],   // on the start vertex
            [50.0, 3.0],  // near mid-segment
            [50.0, 40.0], // far off
        ];
        assert_eq!(collected_controls(&route, &controls, 10.0), vec![0, 1]);
        assert_eq!(collected_controls(&route, &controls, 100.0), vec![0, 1, 2]);
    }

    #[test]
    fn single_vertex_and_empty_route() {
        let controls = [[0.0, 0.0], [5.0, 0.0]];
        assert_eq!(collected_controls(&[[0.0, 0.0]], &controls, 3.0), vec![0]);
        assert!(collected_controls(&[], &controls, 3.0).is_empty());
    }
}
