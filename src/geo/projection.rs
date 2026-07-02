/// Local equirectangular (tangent-plane) projection about a reference lat/lon.
///
/// For a single orienteering map the area is small enough that this simple
/// projection is accurate to well under a pixel, and it avoids a PROJ dependency.
/// Output units are meters: `x` east, `y` **south (down)**. Using a y-down
/// convention matches image-pixel space, so an orientation-preserving (no-mirror)
/// similarity is the correct one — otherwise a start/finish-only calibration flips
/// the route upside down.
#[derive(Clone, Copy, Debug)]
pub struct LocalProjection {
    lat0: f64,
    lon0: f64,
    cos_lat0: f64,
}

const R: f64 = 6_371_000.0;

impl LocalProjection {
    pub fn new(lat0: f64, lon0: f64) -> Self {
        Self {
            lat0,
            lon0,
            cos_lat0: lat0.to_radians().cos(),
        }
    }

    pub fn project(&self, lat: f64, lon: f64) -> (f64, f64) {
        let x = R * (lon - self.lon0).to_radians() * self.cos_lat0;
        let y = -R * (lat - self.lat0).to_radians(); // y grows southward (down)
        (x, y)
    }

    #[allow(dead_code)] // inverse projection, used by tests and future lat/lon readouts
    pub fn unproject(&self, x: f64, y: f64) -> (f64, f64) {
        let lat = self.lat0 - (y / R).to_degrees();
        let lon = self.lon0 + (x / (R * self.cos_lat0)).to_degrees();
        (lat, lon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = LocalProjection::new(59.33, 18.06); // Stockholm-ish
        let (lat, lon) = (59.331, 18.062);
        let (x, y) = p.project(lat, lon);
        let (rlat, rlon) = p.unproject(x, y);
        assert!((lat - rlat).abs() < 1e-9, "lat {lat} vs {rlat}");
        assert!((lon - rlon).abs() < 1e-9, "lon {lon} vs {rlon}");
    }

    #[test]
    fn axes_are_east_and_south() {
        let p = LocalProjection::new(59.33, 18.06);
        let (x0, y0) = p.project(59.33, 18.06);
        assert_eq!((x0, y0), (0.0, 0.0));
        // North must be negative y (image-space y grows downward).
        let (_, y_north) = p.project(59.34, 18.06);
        assert!(
            y_north < 0.0,
            "north should be y-down negative, got {y_north}"
        );
        // East must be positive x.
        let (x_east, _) = p.project(59.33, 18.07);
        assert!(x_east > 0.0, "east should be positive x, got {x_east}");
    }

    #[test]
    fn projected_distance_matches_haversine_locally() {
        use crate::model::Waypoint;
        use crate::model::track::haversine;
        let p = LocalProjection::new(59.33, 18.06);
        let (lat, lon) = (59.335, 18.07);
        let (x, y) = p.project(lat, lon);
        let planar = (x * x + y * y).sqrt();
        let a = Waypoint {
            lat: 59.33,
            lon: 18.06,
            ..Waypoint::default()
        };
        let b = Waypoint {
            lat,
            lon,
            ..Waypoint::default()
        };
        let great_circle = haversine(&a, &b);
        // Sub-permille agreement over map-scale distances.
        assert!(
            (planar - great_circle).abs() / great_circle < 1e-3,
            "planar {planar} vs haversine {great_circle}"
        );
    }
}
