//! Polyline geometry: perpendicular point→segment distance and an iterative
//! Douglas–Peucker simplification. Used to turn a noisy freehand drag into a
//! compact vertex list, and to hit-test drawn routes against the pointer.

/// Distance from point `p` to the segment `a`–`b` (clamped to the endpoints, so a
/// point beyond an end measures to that end). Degenerates to the point distance
/// when the segment has zero length.
pub fn point_segment_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    if len2 <= f64::EPSILON {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    let t = (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (a[0] + t * abx, a[1] + t * aby);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// Douglas–Peucker simplification: keep the fewest vertices such that no dropped
/// point lies farther than `tolerance` from the retained polyline. The two
/// endpoints are always kept. Iterative (explicit stack) so a long freehand
/// stroke can't overflow the call stack.
pub fn simplify_polyline(pts: &[[f64; 2]], tolerance: f64) -> Vec<[f64; 2]> {
    let n = pts.len();
    if n <= 2 || tolerance <= 0.0 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((lo, hi)) = stack.pop() {
        if hi <= lo + 1 {
            continue;
        }
        let mut best = tolerance;
        let mut best_i = None;
        for i in (lo + 1)..hi {
            let d = point_segment_dist(pts[i], pts[lo], pts[hi]);
            if d > best {
                best = d;
                best_i = Some(i);
            }
        }
        if let Some(i) = best_i {
            keep[i] = true;
            stack.push((lo, i));
            stack.push((i, hi));
        }
    }
    pts.iter()
        .zip(&keep)
        .filter_map(|(&p, &k)| k.then_some(p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_segment_dist_perpendicular_and_clamped() {
        // Perpendicular foot lands inside the segment.
        assert!((point_segment_dist([1.0, 1.0], [0.0, 0.0], [2.0, 0.0]) - 1.0).abs() < 1e-9);
        // Beyond the end: measures to the endpoint.
        assert!((point_segment_dist([5.0, 0.0], [0.0, 0.0], [2.0, 0.0]) - 3.0).abs() < 1e-9);
        // Zero-length segment: point distance.
        assert!((point_segment_dist([3.0, 4.0], [0.0, 0.0], [0.0, 0.0]) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn short_inputs_pass_through() {
        assert_eq!(simplify_polyline(&[], 1.0), Vec::<[f64; 2]>::new());
        assert_eq!(simplify_polyline(&[[0.0, 0.0]], 1.0), vec![[0.0, 0.0]]);
        let two = [[0.0, 0.0], [1.0, 1.0]];
        assert_eq!(simplify_polyline(&two, 1.0), two.to_vec());
    }

    #[test]
    fn collinear_chain_collapses_to_endpoints() {
        let pts = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0], [4.0, 0.0]];
        assert_eq!(simplify_polyline(&pts, 0.01), vec![[0.0, 0.0], [4.0, 0.0]]);
    }

    #[test]
    fn a_spike_above_tolerance_survives() {
        // A bump of height 5 in the middle must be kept at tolerance 1.
        let pts = [[0.0, 0.0], [5.0, 5.0], [10.0, 0.0]];
        let out = simplify_polyline(&pts, 1.0);
        assert_eq!(out, pts.to_vec());
        // …but collapses when the tolerance exceeds the bump.
        assert_eq!(
            simplify_polyline(&pts, 6.0),
            vec![[0.0, 0.0], [10.0, 0.0]]
        );
    }

    #[test]
    fn tolerance_zero_keeps_everything() {
        let pts = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]];
        assert_eq!(simplify_polyline(&pts, 0.0), pts.to_vec());
    }

    #[test]
    fn endpoints_always_preserved() {
        let pts = [[0.0, 0.0], [1.0, 0.1], [2.0, 0.0], [3.0, 0.1], [9.0, 0.0]];
        let out = simplify_polyline(&pts, 100.0);
        assert_eq!(out.first(), Some(&[0.0, 0.0]));
        assert_eq!(out.last(), Some(&[9.0, 0.0]));
    }
}
