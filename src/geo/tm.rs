//! Forward transverse Mercator projection on the WGS84 ellipsoid.
//!
//! This is the projection family behind UTM and most national grids used for
//! orienteering maps (SWEREF 99 TM, ETRS89/UTM zones, TM35FIN, …), so supporting
//! it — parametrized by central meridian, scale, and false offsets — covers the
//! coordinate systems world files and GeoTIFFs actually carry, without a PROJ
//! dependency. Datum differences between WGS84 and ETRS89-family datums are
//! sub-meter, far below map-drawing accuracy.

/// Parameters of a transverse Mercator CRS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TmParams {
    /// Central meridian, degrees.
    pub lon0: f64,
    /// Latitude of origin, degrees (0 for UTM).
    pub lat0: f64,
    /// Scale factor on the central meridian (0.9996 for UTM).
    pub k0: f64,
    pub false_e: f64,
    pub false_n: f64,
}

impl TmParams {
    /// The UTM zone containing `lon`, in the hemisphere of `lat`.
    pub fn utm_for(lat: f64, lon: f64) -> TmParams {
        let zone = (((lon + 180.0) / 6.0).floor() as i32).clamp(0, 59) + 1;
        TmParams {
            lon0: zone as f64 * 6.0 - 183.0,
            lat0: 0.0,
            k0: 0.9996,
            false_e: 500_000.0,
            false_n: if lat < 0.0 { 10_000_000.0 } else { 0.0 },
        }
    }
}

const A: f64 = 6_378_137.0; // WGS84 semi-major axis
const F: f64 = 1.0 / 298.257_223_563;

/// Project WGS84 lat/lon (degrees) to transverse Mercator easting/northing
/// (meters), using the standard USGS/Snyder series (sub-meter within a zone).
pub fn tm_forward(p: TmParams, lat: f64, lon: f64) -> (f64, f64) {
    let e2 = F * (2.0 - F);
    let ep2 = e2 / (1.0 - e2);

    let phi = lat.to_radians();
    let dlam = (lon - p.lon0).to_radians();

    let sin_phi = phi.sin();
    let cos_phi = phi.cos();
    let tan_phi = phi.tan();

    let n = A / (1.0 - e2 * sin_phi * sin_phi).sqrt();
    let t = tan_phi * tan_phi;
    let c = ep2 * cos_phi * cos_phi;
    let a_ = cos_phi * dlam;

    let m = meridian_arc(e2, phi);
    let m0 = meridian_arc(e2, p.lat0.to_radians());

    let easting = p.false_e
        + p.k0
            * n
            * (a_
                + (1.0 - t + c) * a_.powi(3) / 6.0
                + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * a_.powi(5) / 120.0);
    let northing = p.false_n
        + p.k0
            * (m - m0
                + n * tan_phi
                    * (a_ * a_ / 2.0
                        + (5.0 - t + 9.0 * c + 4.0 * c * c) * a_.powi(4) / 24.0
                        + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * a_.powi(6)
                            / 720.0));
    (easting, northing)
}

/// Meridian arc length from the equator to latitude `phi` (radians).
fn meridian_arc(e2: f64, phi: f64) -> f64 {
    let e4 = e2 * e2;
    let e6 = e4 * e2;
    A * ((1.0 - e2 / 4.0 - 3.0 * e4 / 64.0 - 5.0 * e6 / 256.0) * phi
        - (3.0 * e2 / 8.0 + 3.0 * e4 / 32.0 + 45.0 * e6 / 1024.0) * (2.0 * phi).sin()
        + (15.0 * e4 / 256.0 + 45.0 * e6 / 1024.0) * (4.0 * phi).sin()
        - (35.0 * e6 / 3072.0) * (6.0 * phi).sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Waypoint;
    use crate::model::track::haversine;

    const UTM33N: TmParams = TmParams {
        lon0: 15.0,
        lat0: 0.0,
        k0: 0.9996,
        false_e: 500_000.0,
        false_n: 0.0,
    };

    #[test]
    fn central_meridian_maps_to_false_easting() {
        for lat in [0.0, 30.0, 59.33, 70.0] {
            let (e, n) = tm_forward(UTM33N, lat, 15.0);
            assert!((e - 500_000.0).abs() < 1e-6, "easting {e} at lat {lat}");
            if lat > 0.0 {
                assert!(n > 0.0);
            }
        }
    }

    #[test]
    fn eastings_mirror_around_the_central_meridian() {
        let (e_w, n_w) = tm_forward(UTM33N, 59.0, 15.0 - 1.5);
        let (e_e, n_e) = tm_forward(UTM33N, 59.0, 15.0 + 1.5);
        assert!((e_w + e_e - 1_000_000.0).abs() < 0.01, "{e_w} vs {e_e}");
        assert!((n_w - n_e).abs() < 0.01);
    }

    #[test]
    fn zone_edge_easting_at_equator_matches_known_value() {
        // 3° from the central meridian at the equator ≈ 833 978 m easting —
        // the well-known UTM zone-boundary value.
        let (e, _) = tm_forward(UTM33N, 0.0, 18.0);
        assert!((e - 833_978.0).abs() < 100.0, "easting {e}");
    }

    #[test]
    fn projected_distances_match_haversine_locally() {
        // Two points ~1 km apart near Stockholm: TM distance (scale ≈ k0 near the
        // central meridian) must agree with the great-circle distance to ~0.1%.
        let (a_lat, a_lon) = (59.33, 18.06);
        let (b_lat, b_lon) = (59.335, 18.07);
        let (e1, n1) = tm_forward(UTM33N, a_lat, a_lon);
        let (e2, n2) = tm_forward(UTM33N, b_lat, b_lon);
        let planar = ((e2 - e1).powi(2) + (n2 - n1).powi(2)).sqrt();
        let wp = |lat, lon| Waypoint {
            lat,
            lon,
            ..Waypoint::default()
        };
        let gc = haversine(&wp(a_lat, a_lon), &wp(b_lat, b_lon));
        // haversine is spherical (R = 6371 km) while TM is ellipsoidal; at 59°N
        // the local ellipsoid radii differ from the sphere by ~0.3%, which
        // dominates the comparison. 0.5% still catches real formula errors.
        assert!(
            (planar - gc).abs() / gc < 5e-3,
            "planar {planar} vs great-circle {gc}"
        );
    }

    #[test]
    fn utm_zone_picker() {
        let z33 = TmParams::utm_for(59.0, 15.1);
        assert_eq!(z33.lon0, 15.0);
        assert_eq!(z33.false_n, 0.0);
        let z35s = TmParams::utm_for(-33.0, 25.0);
        assert_eq!(z35s.lon0, 27.0);
        assert_eq!(z35s.false_n, 10_000_000.0);
        // Zone 1 starts at -180.
        assert_eq!(TmParams::utm_for(0.0, -179.9).lon0, -177.0);
    }
}
