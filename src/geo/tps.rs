use crate::geo::transform::Correspondence;
use nalgebra::DMatrix;

/// A thin-plate-spline warp mapping source (meters) to destination (pixels).
///
/// TPS is an *interpolating* transform: it passes through every control point
/// exactly, and bends smoothly in between. That is what lets calibration pins stay
/// locked on their map features no matter how many points are added.
#[derive(Clone, Debug)]
pub struct Tps {
    controls: Vec<(f64, f64)>,
    /// RBF weights for the u (x-pixel) and v (y-pixel) outputs.
    wu: Vec<f64>,
    wv: Vec<f64>,
    /// Affine part `[a0, a1*x, a2*y]` for each output.
    au: [f64; 3],
    av: [f64; 3],
}

/// TPS radial basis `U(r) = r^2 * ln(r)`, evaluated from `r^2` (0 at r = 0).
fn rbf(r2: f64) -> f64 {
    if r2 <= 0.0 { 0.0 } else { 0.5 * r2 * r2.ln() }
}

impl Tps {
    /// Fit an interpolating TPS through the control points. Needs at least 3
    /// non-collinear points; returns `None` if the system is singular (the caller
    /// then falls back to an affine fit).
    pub fn fit(pts: &[Correspondence]) -> Option<Tps> {
        let n = pts.len();
        if n < 3 {
            return None;
        }

        // L = [ K  P ; P^T 0 ], size (n+3) x (n+3).
        let mut l = DMatrix::zeros(n + 3, n + 3);
        for i in 0..n {
            for j in 0..n {
                let (xi, yi) = pts[i].0;
                let (xj, yj) = pts[j].0;
                let (dx, dy) = (xi - xj, yi - yj);
                l[(i, j)] = rbf(dx * dx + dy * dy);
            }
        }
        for i in 0..n {
            let (x, y) = pts[i].0;
            l[(i, n)] = 1.0;
            l[(i, n + 1)] = x;
            l[(i, n + 2)] = y;
            l[(n, i)] = 1.0;
            l[(n + 1, i)] = x;
            l[(n + 2, i)] = y;
        }

        // RHS: columns are the u and v targets, with 3 trailing zeros each.
        let mut b = DMatrix::zeros(n + 3, 2);
        for i in 0..n {
            b[(i, 0)] = pts[i].1.0;
            b[(i, 1)] = pts[i].1.1;
        }

        let sol = l.lu().solve(&b)?;

        let wu = (0..n).map(|i| sol[(i, 0)]).collect();
        let wv = (0..n).map(|i| sol[(i, 1)]).collect();
        let au = [sol[(n, 0)], sol[(n + 1, 0)], sol[(n + 2, 0)]];
        let av = [sol[(n, 1)], sol[(n + 1, 1)], sol[(n + 2, 1)]];
        let controls = pts.iter().map(|&(src, _)| src).collect();
        Some(Tps {
            controls,
            wu,
            wv,
            au,
            av,
        })
    }

    pub fn apply(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let mut u = self.au[0] + self.au[1] * x + self.au[2] * y;
        let mut v = self.av[0] + self.av[1] * x + self.av[2] * y;
        for (i, &(cx, cy)) in self.controls.iter().enumerate() {
            let (dx, dy) = (x - cx, y - cy);
            let phi = rbf(dx * dx + dy * dy);
            u += self.wu[i] * phi;
            v += self.wv[i] * phi;
        }
        (u, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_exact(src: &[(f64, f64)], dst: &[(f64, f64)], tol: f64) {
        let pts: Vec<Correspondence> = src.iter().copied().zip(dst.iter().copied()).collect();
        let tps = Tps::fit(&pts).expect("tps fit");
        for (&s, &d) in src.iter().zip(dst.iter()) {
            let (u, v) = tps.apply(s);
            assert!(
                (u - d.0).abs() < tol && (v - d.1).abs() < tol,
                "control {s:?} -> {:?} expected {d:?}",
                (u, v)
            );
        }
    }

    #[test]
    fn interpolates_all_controls_exactly() {
        // Locked points must be reproduced exactly for 3, 5 and 8 points.
        let three = [(0.0, 0.0), (100.0, 0.0), (40.0, 90.0)];
        let three_dst = [(10.0, 20.0), (210.0, 25.0), (95.0, 190.0)];
        check_exact(&three, &three_dst, 1e-6);

        let five = [
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, 80.0),
            (0.0, 80.0),
            (50.0, 40.0),
        ];
        let five_dst = [
            (5.0, 3.0),
            (198.0, 12.0),
            (205.0, 168.0),
            (2.0, 160.0),
            (110.0, 88.0),
        ];
        check_exact(&five, &five_dst, 1e-6);

        let eight: Vec<(f64, f64)> = (0..8).map(|i| (i as f64 * 13.0, (i * i) as f64)).collect();
        let eight_dst: Vec<(f64, f64)> = (0..8)
            .map(|i| (i as f64 * 27.0 + 4.0, (i as f64) * 9.0 - 3.0))
            .collect();
        check_exact(&eight, &eight_dst, 1e-5);
    }

    #[test]
    fn collinear_points_are_rejected() {
        let pts: Vec<Correspondence> = vec![
            ((0.0, 0.0), (0.0, 0.0)),
            ((1.0, 1.0), (2.0, 2.0)),
            ((2.0, 2.0), (4.0, 4.0)),
        ];
        assert!(Tps::fit(&pts).is_none());
    }
}
