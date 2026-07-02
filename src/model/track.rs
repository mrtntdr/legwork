use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single GPS sample from a track.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Waypoint {
    pub time: Option<DateTime<Utc>>,
    pub lat: f64,
    pub lon: f64,
    pub ele: Option<f64>,
    pub hr: Option<u16>,
}

/// An ordered sequence of GPS samples.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Track {
    pub points: Vec<Waypoint>,
}

/// Great-circle distance between two waypoints, in meters.
pub fn haversine(a: &Waypoint, b: &Waypoint) -> f64 {
    const R: f64 = 6_371_000.0;
    let lat1 = a.lat.to_radians();
    let lat2 = b.lat.to_radians();
    let dlat = (b.lat - a.lat).to_radians();
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * h.sqrt().clamp(-1.0, 1.0).asin()
}

impl Track {
    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Average lat/lon, used as the projection reference.
    pub fn centroid(&self) -> Option<(f64, f64)> {
        if self.points.is_empty() {
            return None;
        }
        let n = self.points.len() as f64;
        let (lat, lon) = self
            .points
            .iter()
            .fold((0.0, 0.0), |(la, lo), p| (la + p.lat, lo + p.lon));
        Some((lat / n, lon / n))
    }

    /// Cumulative along-track distance in meters for each waypoint (first = 0).
    pub fn cumulative_distance(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.points.len());
        let mut acc = 0.0;
        for (i, p) in self.points.iter().enumerate() {
            if i > 0 {
                acc += haversine(&self.points[i - 1], p);
            }
            out.push(acc);
        }
        out
    }

    /// Distance in meters between two waypoint indices along the track.
    pub fn route_length(&self, i0: usize, i1: usize) -> f64 {
        let (a, b) = (i0.min(i1), i0.max(i1));
        (a..b)
            .map(|i| haversine(&self.points[i], &self.points[i + 1]))
            .sum()
    }

    /// Straight-line distance in meters between two waypoint indices.
    pub fn straight_distance(&self, i0: usize, i1: usize) -> f64 {
        haversine(&self.points[i0], &self.points[i1])
    }

    /// Per-segment speed in m/s (length == points-1). 0.0 where time is missing/zero.
    pub fn segment_speeds(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.points.len().saturating_sub(1));
        for w in self.points.windows(2) {
            let dist = haversine(&w[0], &w[1]);
            let speed = match (w[0].time, w[1].time) {
                (Some(t0), Some(t1)) => {
                    let dt = (t1 - t0).num_milliseconds() as f64 / 1000.0;
                    if dt > 0.0 { dist / dt } else { 0.0 }
                }
                _ => 0.0,
            };
            out.push(speed);
        }
        out
    }

    /// Elapsed seconds between two waypoint indices, if both have timestamps.
    pub fn duration_between(&self, i0: usize, i1: usize) -> Option<f64> {
        let t0 = self.points.get(i0)?.time?;
        let t1 = self.points.get(i1)?.time?;
        Some((t1 - t0).num_milliseconds() as f64 / 1000.0)
    }

    /// Total moving duration in seconds (last time - first time), if available.
    pub fn duration_secs(&self) -> Option<f64> {
        self.duration_between(0, self.points.len().checked_sub(1)?)
    }

    pub fn total_distance(&self) -> f64 {
        self.cumulative_distance().last().copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn wp(lat: f64, lon: f64) -> Waypoint {
        Waypoint {
            lat,
            lon,
            ..Waypoint::default()
        }
    }

    fn wp_t(lat: f64, lon: f64, secs: i64) -> Waypoint {
        Waypoint {
            time: Some(Utc.timestamp_opt(1_600_000_000 + secs, 0).unwrap()),
            ..wp(lat, lon)
        }
    }

    #[test]
    fn haversine_one_degree_of_latitude() {
        // 1 degree of latitude is ~111.2 km everywhere on the sphere.
        let d = haversine(&wp(59.0, 18.0), &wp(60.0, 18.0));
        assert!((d - 111_195.0).abs() < 100.0, "got {d}");
    }

    #[test]
    fn haversine_is_symmetric_and_zero_at_same_point() {
        let (a, b) = (wp(59.33, 18.06), wp(59.34, 18.08));
        assert_eq!(haversine(&a, &a), 0.0);
        assert!((haversine(&a, &b) - haversine(&b, &a)).abs() < 1e-9);
    }

    #[test]
    fn centroid_averages_coordinates() {
        assert!(Track::default().centroid().is_none());
        let track = Track {
            points: vec![wp(59.0, 18.0), wp(61.0, 20.0)],
        };
        let (lat, lon) = track.centroid().unwrap();
        assert!((lat - 60.0).abs() < 1e-12);
        assert!((lon - 19.0).abs() < 1e-12);
    }

    #[test]
    fn cumulative_distance_starts_at_zero_and_is_monotonic() {
        let track = Track {
            points: vec![wp(59.0, 18.0), wp(59.001, 18.0), wp(59.002, 18.0)],
        };
        let cum = track.cumulative_distance();
        assert_eq!(cum.len(), 3);
        assert_eq!(cum[0], 0.0);
        assert!(cum.windows(2).all(|w| w[1] >= w[0]));
        assert!((cum[2] - track.total_distance()).abs() < 1e-9);
        assert!((cum[2] - track.route_length(0, 2)).abs() < 1e-9);
    }

    #[test]
    fn route_length_is_order_independent() {
        let track = Track {
            points: vec![wp(59.0, 18.0), wp(59.001, 18.001), wp(59.002, 18.0)],
        };
        assert_eq!(track.route_length(0, 2), track.route_length(2, 0));
        assert_eq!(track.route_length(1, 1), 0.0);
    }

    #[test]
    fn segment_speeds_from_timestamps() {
        // ~111.2 m of latitude covered in 10 s => ~11 m/s.
        let track = Track {
            points: vec![wp_t(59.0, 18.0, 0), wp_t(59.001, 18.0, 10)],
        };
        let speeds = track.segment_speeds();
        assert_eq!(speeds.len(), 1);
        assert!((speeds[0] - 11.12).abs() < 0.1, "got {}", speeds[0]);
    }

    #[test]
    fn segment_speeds_zero_without_time_or_with_zero_dt() {
        let no_time = Track {
            points: vec![wp(59.0, 18.0), wp(59.001, 18.0)],
        };
        assert_eq!(no_time.segment_speeds(), vec![0.0]);

        let same_time = Track {
            points: vec![wp_t(59.0, 18.0, 0), wp_t(59.001, 18.0, 0)],
        };
        assert_eq!(same_time.segment_speeds(), vec![0.0]);
    }

    #[test]
    fn duration_between_and_total() {
        let track = Track {
            points: vec![
                wp_t(59.0, 18.0, 0),
                wp(59.001, 18.0),
                wp_t(59.002, 18.0, 90),
            ],
        };
        assert_eq!(track.duration_between(0, 2), Some(90.0));
        assert_eq!(track.duration_between(0, 1), None); // missing timestamp
        assert_eq!(track.duration_between(0, 99), None); // out of range
        assert_eq!(track.duration_secs(), Some(90.0));
        assert_eq!(Track::default().duration_secs(), None);
    }
}
