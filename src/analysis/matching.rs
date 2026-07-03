use crate::geo::MapTransform;

/// Match ordered course controls to one athlete's route, both in image pixels.
///
/// Sequential greedy: control `k` is matched to the route point nearest its map
/// position, searching from just past the previous match so matched indices are
/// strictly increasing along the track. A control whose nearest remaining point
/// is farther than `max_dist_px` stays unmatched (`None`) and does not advance
/// the search cursor, so later controls can still match. The first and last
/// route points are reserved for the implicit start/finish boundaries.
pub fn match_controls(
    route_px: &[(f64, f64)],
    controls: &[[f64; 2]],
    max_dist_px: f64,
) -> Vec<Option<usize>> {
    let mut out = Vec::with_capacity(controls.len());
    let last = route_px.len().saturating_sub(1);
    let mut start = 1usize;
    for c in controls {
        let mut best: Option<(usize, f64)> = None;
        for (i, p) in route_px.iter().enumerate().take(last).skip(start) {
            let d2 = (p.0 - c[0]).powi(2) + (p.1 - c[1]).powi(2);
            if best.is_none_or(|(_, bd)| d2 < bd) {
                best = Some((i, d2));
            }
        }
        let matched = best.filter(|&(_, d2)| d2 <= max_dist_px * max_dist_px);
        if let Some((i, _)) = matched {
            start = i + 1;
        }
        out.push(matched.map(|(i, _)| i));
    }
    out
}

/// Approximate local scale (pixels per meter) of a transform near a source point,
/// by finite differences. Used to make the control match radius scale-aware, which
/// also guards against wild TPS extrapolation far outside the calibrated region.
pub fn local_scale_px_per_m(t: &MapTransform, at: (f64, f64)) -> f64 {
    let o = t.apply(at);
    let ex = t.apply((at.0 + 1.0, at.1));
    let ey = t.apply((at.0, at.1 + 1.0));
    let sx = ((ex.0 - o.0).powi(2) + (ex.1 - o.1).powi(2)).sqrt();
    let sy = ((ey.0 - o.0).powi(2) + (ey.1 - o.1).powi(2)).sqrt();
    (sx + sy) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight horizontal route: x = 0, 10, 20, … at y = 0.
    fn route(n: usize) -> Vec<(f64, f64)> {
        (0..n).map(|i| (i as f64 * 10.0, 0.0)).collect()
    }

    #[test]
    fn controls_match_their_nearest_pass() {
        let m = match_controls(&route(11), &[[31.0, 2.0], [72.0, -3.0]], 15.0);
        assert_eq!(m, vec![Some(3), Some(7)]);
    }

    #[test]
    fn far_controls_stay_unmatched_without_blocking_later_ones() {
        // Control 1 is nowhere near the route; control 2 must still match.
        let m = match_controls(&route(11), &[[50.0, 500.0], [72.0, 0.0]], 15.0);
        assert_eq!(m, vec![None, Some(7)]);
    }

    #[test]
    fn matches_are_strictly_increasing_even_for_out_of_order_controls() {
        // Second control lies geographically *before* the first along the route;
        // the search cursor forces a later (monotone) match or none at all.
        let m = match_controls(&route(11), &[[70.0, 0.0], [30.0, 0.0]], 15.0);
        assert_eq!(m[0], Some(7));
        match m[1] {
            Some(i) => assert!(i > 7),
            None => {} // nothing near enough after index 7 — also fine
        }
    }

    #[test]
    fn start_and_finish_points_are_reserved() {
        // Controls sitting exactly on the first/last points match neighbors instead.
        let m = match_controls(&route(5), &[[0.0, 0.0], [40.0, 0.0]], 15.0);
        assert_eq!(m, vec![Some(1), Some(3)]);
    }

    #[test]
    fn empty_or_tiny_route_matches_nothing() {
        assert_eq!(match_controls(&[], &[[0.0, 0.0]], 15.0), vec![None]);
        assert_eq!(
            match_controls(&[(0.0, 0.0)], &[[0.0, 0.0]], 15.0),
            vec![None]
        );
    }

    #[test]
    fn local_scale_of_uniform_matrix() {
        use nalgebra::Matrix3;
        let t = MapTransform::Matrix(Matrix3::new(
            3.0, 0.0, 5.0, 0.0, 3.0, -2.0, 0.0, 0.0, 1.0,
        ));
        let s = local_scale_px_per_m(&t, (100.0, 100.0));
        assert!((s - 3.0).abs() < 1e-9, "scale {s}");
    }
}
