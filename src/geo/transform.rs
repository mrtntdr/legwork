use crate::geo::tps::Tps;
use nalgebra::{DMatrix, DVector, Matrix3, Vector3};

/// A planar-meters -> image-pixels mapping.
///
/// - `Matrix`: a 3x3 homogeneous transform (translation / similarity / affine).
/// - `Tps`: an interpolating thin-plate spline that honors every control point exactly.
/// - `Translated`: a base transform followed by a fixed pixel offset, used to honor
///   a single calibration pin on top of a transform borrowed from another athlete.
#[derive(Clone, Debug)]
pub enum MapTransform {
    Matrix(Matrix3<f64>),
    Tps(Tps),
    Translated(Box<MapTransform>, [f64; 2]),
}

/// A (source-meters, dest-pixel) correspondence used to fit a transform.
pub type Correspondence = ((f64, f64), (f64, f64));

impl MapTransform {
    /// Map a point in source (meters) space to dest (pixel) space.
    pub fn apply(&self, (x, y): (f64, f64)) -> (f64, f64) {
        match self {
            MapTransform::Matrix(m) => {
                let v = m * Vector3::new(x, y, 1.0);
                (v.x / v.z, v.y / v.z)
            }
            MapTransform::Tps(t) => t.apply((x, y)),
            MapTransform::Translated(base, d) => {
                let (u, v) = base.apply((x, y));
                (u + d[0], v + d[1])
            }
        }
    }

    /// Fit an *exact* transform through the correspondences so locked points never move:
    /// 2 -> similarity, 3+ -> interpolating TPS (affine fallback if degenerate).
    pub fn fit(pts: &[Correspondence]) -> Option<MapTransform> {
        match pts.len() {
            0 | 1 => None,
            2 => fit_similarity(pts),
            _ => Tps::fit(pts)
                .map(MapTransform::Tps)
                .or_else(|| fit_affine(pts)),
        }
    }

    /// Least-squares affine fit (3+ points). Unlike `fit`, this never warps: it's
    /// used for smooth mappings that are already near-affine, like composing a
    /// georeferenced map's projection with the local meter frame.
    pub fn fit_affine(pts: &[Correspondence]) -> Option<MapTransform> {
        if pts.len() < 3 {
            return None;
        }
        fit_affine(pts)
    }

    /// Root-mean-square residual in pixels over the given correspondences.
    /// For exact fits this is ~0 at the control points.
    pub fn rms_residual(&self, pts: &[Correspondence]) -> f64 {
        if pts.is_empty() {
            return 0.0;
        }
        let sum: f64 = pts
            .iter()
            .map(|&(src, dst)| {
                let (u, v) = self.apply(src);
                (u - dst.0).powi(2) + (v - dst.1).powi(2)
            })
            .sum();
        (sum / pts.len() as f64).sqrt()
    }
}

/// Helmert similarity (rotation + uniform scale + translation).
/// Two points determine it exactly; more are least-squares.
/// Model: u = a*x - b*y + tx ; v = b*x + a*y + ty.
fn fit_similarity(pts: &[Correspondence]) -> Option<MapTransform> {
    let n = pts.len();
    let mut a = DMatrix::zeros(2 * n, 4);
    let mut rhs = DVector::zeros(2 * n);
    for (i, &((x, y), (u, v))) in pts.iter().enumerate() {
        a[(2 * i, 0)] = x;
        a[(2 * i, 1)] = -y;
        a[(2 * i, 2)] = 1.0;
        a[(2 * i + 1, 0)] = y;
        a[(2 * i + 1, 1)] = x;
        a[(2 * i + 1, 3)] = 1.0;
        rhs[2 * i] = u;
        rhs[2 * i + 1] = v;
    }
    let p = solve_lstsq(&a, &rhs)?;
    Some(MapTransform::Matrix(Matrix3::new(
        p[0], -p[1], p[2], p[1], p[0], p[3], 0.0, 0.0, 1.0,
    )))
}

/// Affine least-squares fit, used only as a fallback when TPS is degenerate.
fn fit_affine(pts: &[Correspondence]) -> Option<MapTransform> {
    let n = pts.len();
    let mut a = DMatrix::zeros(n, 3);
    let mut bu = DVector::zeros(n);
    let mut bv = DVector::zeros(n);
    for (i, &((x, y), (u, v))) in pts.iter().enumerate() {
        a[(i, 0)] = x;
        a[(i, 1)] = y;
        a[(i, 2)] = 1.0;
        bu[i] = u;
        bv[i] = v;
    }
    let row0 = solve_lstsq(&a, &bu)?;
    let row1 = solve_lstsq(&a, &bv)?;
    Some(MapTransform::Matrix(Matrix3::new(
        row0[0], row0[1], row0[2], row1[0], row1[1], row1[2], 0.0, 0.0, 1.0,
    )))
}

/// Least-squares solve of `A x = b` via the pseudo-inverse (SVD-backed).
fn solve_lstsq(a: &DMatrix<f64>, b: &DVector<f64>) -> Option<DVector<f64>> {
    let svd = a.clone().svd(true, true);
    svd.solve(b, 1e-12).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_all(m: &MapTransform, src: &[(f64, f64)]) -> Vec<(f64, f64)> {
        src.iter().map(|&p| m.apply(p)).collect()
    }

    #[test]
    fn similarity_recovers_rotation_scale() {
        // Known transform: scale 2, rotate 90deg, translate (10, 5).
        let truth =
            MapTransform::Matrix(Matrix3::new(0.0, -2.0, 10.0, 2.0, 0.0, 5.0, 0.0, 0.0, 1.0));
        let src = [(1.0, 0.0), (0.0, 1.0)];
        let dst: Vec<_> = apply_all(&truth, &src);
        let pts: Vec<Correspondence> = src.iter().cloned().zip(dst).collect();
        let fit = MapTransform::fit(&pts).unwrap();
        assert!(fit.rms_residual(&pts) < 1e-9);
    }

    #[test]
    fn tps_interpolates_three_points_exactly() {
        let src = [(0.0, 0.0), (3.0, 1.0), (1.0, 4.0)];
        let dst = [(4.0, -1.0), (9.5, 1.0), (5.3, 7.0)];
        let pts: Vec<Correspondence> = src.iter().cloned().zip(dst).collect();
        let fit = MapTransform::fit(&pts).unwrap();
        assert!(matches!(fit, MapTransform::Tps(_)));
        assert!(fit.rms_residual(&pts) < 1e-6);
    }

    #[test]
    fn too_few_points_yield_no_fit() {
        assert!(MapTransform::fit(&[]).is_none());
        assert!(MapTransform::fit(&[((0.0, 0.0), (1.0, 1.0))]).is_none());
    }

    #[test]
    fn translated_applies_base_then_offset() {
        let base = MapTransform::Matrix(Matrix3::new(
            2.0, 0.0, 1.0, 0.0, 2.0, -1.0, 0.0, 0.0, 1.0,
        ));
        let t = MapTransform::Translated(Box::new(base), [10.0, -5.0]);
        assert_eq!(t.apply((3.0, 4.0)), (2.0 * 3.0 + 1.0 + 10.0, 2.0 * 4.0 - 1.0 - 5.0));
    }

    #[test]
    fn rms_residual_of_empty_set_is_zero() {
        let ident = MapTransform::Matrix(Matrix3::identity());
        assert_eq!(ident.rms_residual(&[]), 0.0);
    }

    #[test]
    fn collinear_points_fall_back_to_affine() {
        // Three collinear sources are degenerate for TPS; the affine fallback
        // must still reproduce this (exactly affine) mapping.
        let pts: Vec<Correspondence> = vec![
            ((0.0, 0.0), (10.0, 5.0)),
            ((1.0, 1.0), (12.0, 8.0)),
            ((2.0, 2.0), (14.0, 11.0)),
        ];
        let fit = MapTransform::fit(&pts).unwrap();
        assert!(matches!(fit, MapTransform::Matrix(_)));
        assert!(
            fit.rms_residual(&pts) < 1e-9,
            "residual {}",
            fit.rms_residual(&pts)
        );
    }

    #[test]
    fn similarity_preserves_orientation() {
        // A similarity fit must not mirror: the cross product sign of two basis
        // vectors is preserved through the transform.
        let src = [(0.0, 0.0), (10.0, 0.0)];
        let dst = [(3.0, 4.0), (3.0, 24.0)]; // rotated 90°, scaled 2x
        let pts: Vec<Correspondence> = src.iter().cloned().zip(dst).collect();
        let fit = MapTransform::fit(&pts).unwrap();
        assert!(fit.rms_residual(&pts) < 1e-9);
        // (1,0) and (0,1) in source keep their handedness in dest space.
        let o = fit.apply((0.0, 0.0));
        let ex = fit.apply((1.0, 0.0));
        let ey = fit.apply((0.0, 1.0));
        let cross = (ex.0 - o.0) * (ey.1 - o.1) - (ex.1 - o.1) * (ey.0 - o.0);
        assert!(cross > 0.0, "similarity mirrored the plane (cross {cross})");
    }

    #[test]
    fn locked_points_do_not_move_as_more_are_added() {
        // Five arbitrary correspondences; every one must be reproduced exactly.
        let src = [
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 80.0),
            (0.0, 80.0),
            (50.0, 40.0),
        ];
        let dst = [
            (5.0, 3.0),
            (198.0, 12.0),
            (205.0, 168.0),
            (2.0, 160.0),
            (140.0, 70.0),
        ];
        let pts: Vec<Correspondence> = src.iter().cloned().zip(dst).collect();
        let fit = MapTransform::fit(&pts).unwrap();
        assert!(
            fit.rms_residual(&pts) < 1e-6,
            "residual {}",
            fit.rms_residual(&pts)
        );
    }
}
