//! Inverting a meters→pixels [`MapTransform`] to a pixels→meters one, so a route
//! drawn in image pixels can be measured in ground meters.
//!
//! Every `MapTransform::Matrix` the app builds is affine (bottom row `[0,0,1]`),
//! so the matrix cases invert exactly. `Tps` has no closed-form inverse; we
//! approximate it with an affine fit sampled over the region of interest, which
//! is sub-percent accurate because calibration warps are near-affine at map scale.

use crate::geo::MapTransform;
use nalgebra::Matrix3;

/// Invert `t` (meters→pixels) into a pixels→meters transform.
///
/// `bounds_m` is `((min_x, min_y), (max_x, max_y))` in meters — the region the
/// inverse must serve (used only for the sampled `Tps` case). Returns `None` for
/// a singular transform or a degenerate (zero-area) region it can't sample.
pub fn invert_transform(
    t: &MapTransform,
    bounds_m: ((f64, f64), (f64, f64)),
) -> Option<MapTransform> {
    match t {
        MapTransform::Matrix(m) => m.try_inverse().map(MapTransform::Matrix),
        MapTransform::Translated(base, d) => match invert_transform(base, bounds_m)? {
            // base.apply then +d; the inverse pre-shifts the pixel by -d.
            MapTransform::Matrix(minv) => {
                let shift = Matrix3::new(1.0, 0.0, -d[0], 0.0, 1.0, -d[1], 0.0, 0.0, 1.0);
                Some(MapTransform::Matrix(minv * shift))
            }
            // A non-matrix base (a TPS) already went through the sampled path.
            _ => sample_affine_inverse(t, bounds_m),
        },
        MapTransform::Tps(_) => sample_affine_inverse(t, bounds_m),
    }
}

/// Fit an affine pixels→meters map by sampling the forward transform on a 4×4
/// grid over `bounds_m` and swapping each (meters, pixel) pair.
fn sample_affine_inverse(
    t: &MapTransform,
    ((minx, miny), (maxx, maxy)): ((f64, f64), (f64, f64)),
) -> Option<MapTransform> {
    if !(maxx > minx && maxy > miny) {
        return None;
    }
    let mut pts = Vec::with_capacity(16);
    for gy in 0..4 {
        for gx in 0..4 {
            let mx = minx + (maxx - minx) * gx as f64 / 3.0;
            let my = miny + (maxy - miny) * gy as f64 / 3.0;
            let px = t.apply((mx, my));
            // Correspondence is (source, dest): here source = pixels, dest = meters.
            pts.push((px, (mx, my)));
        }
    }
    MapTransform::fit_affine(&pts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: ((f64, f64), (f64, f64)) = ((0.0, 0.0), (1000.0, 1000.0));

    fn round_trip_err(fwd: &MapTransform, inv: &MapTransform, at: (f64, f64)) -> f64 {
        let px = fwd.apply(at);
        let back = inv.apply(px);
        ((back.0 - at.0).powi(2) + (back.1 - at.1).powi(2)).sqrt()
    }

    #[test]
    fn matrix_inverts_exactly() {
        // Scale 2, rotate 90°, translate — a similarity.
        let fwd = MapTransform::Matrix(Matrix3::new(
            0.0, -2.0, 10.0, 2.0, 0.0, 5.0, 0.0, 0.0, 1.0,
        ));
        let inv = invert_transform(&fwd, BOUNDS).unwrap();
        for at in [(0.0, 0.0), (300.0, 700.0), (999.0, 1.0)] {
            assert!(round_trip_err(&fwd, &inv, at) < 1e-9);
        }
    }

    #[test]
    fn translated_matrix_inverts_exactly() {
        let base = MapTransform::Matrix(Matrix3::new(
            2.0, 0.0, 1.0, 0.0, 2.0, -1.0, 0.0, 0.0, 1.0,
        ));
        let fwd = MapTransform::Translated(Box::new(base), [10.0, -5.0]);
        let inv = invert_transform(&fwd, BOUNDS).unwrap();
        for at in [(0.0, 0.0), (250.0, 400.0), (1000.0, 1000.0)] {
            assert!(round_trip_err(&fwd, &inv, at) < 1e-9, "err at {at:?}");
        }
    }

    #[test]
    fn tps_inverts_approximately() {
        // A near-affine TPS: identity-ish pixels with a small warp on one control.
        let src = [(0.0, 0.0), (1000.0, 0.0), (0.0, 1000.0), (1000.0, 1000.0)];
        let dst = [(0.0, 0.0), (1000.0, 0.0), (0.0, 1000.0), (990.0, 1010.0)];
        let pts: Vec<_> = src.iter().copied().zip(dst).collect();
        let fwd = MapTransform::fit(&pts).unwrap();
        assert!(matches!(fwd, MapTransform::Tps(_)));
        let inv = invert_transform(&fwd, BOUNDS).unwrap();
        // Sub-percent (a few meters over a 1 km region) is plenty for route length.
        for at in [(250.0, 250.0), (500.0, 500.0), (750.0, 250.0)] {
            assert!(round_trip_err(&fwd, &inv, at) < 10.0, "err at {at:?}");
        }
    }

    #[test]
    fn degenerate_inputs_return_none() {
        // Zero-area bounds can't seed the sampled path.
        let src = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        let dst = [(0.0, 0.0), (10.0, 1.0), (1.0, 10.0)];
        let pts: Vec<_> = src.iter().copied().zip(dst).collect();
        let tps = MapTransform::fit(&pts).unwrap();
        assert!(invert_transform(&tps, ((0.0, 0.0), (0.0, 0.0))).is_none());
        // Singular matrix has no inverse.
        let singular = MapTransform::Matrix(Matrix3::zeros());
        assert!(invert_transform(&singular, BOUNDS).is_none());
    }
}
