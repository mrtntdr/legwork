use crate::model::{Track, Waypoint};
use chrono::{DateTime, Utc};
use quick_xml::Reader;
use quick_xml::events::Event;

/// Parse a GPX or TCX track from raw bytes into the unified [`Track`] model.
///
/// The two formats are handled by one event-driven pass that keys off local
/// element names (namespace prefixes are stripped), so HR from GPX `gpxtpx`
/// extensions and TCX `HeartRateBpm` are both captured.
pub fn parse_track(bytes: &[u8]) -> Result<Track, String> {
    let mut reader = Reader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut track = Track::default();

    let mut cur: Option<Waypoint> = None;
    let mut stack: Vec<String> = Vec::new();
    let mut in_heart_rate = false; // inside a TCX <HeartRateBpm> wrapper

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                match name.as_str() {
                    "trkpt" | "Trackpoint" => {
                        let mut wp = Waypoint::default();
                        if name == "trkpt" {
                            read_latlon_attrs(&e, &mut wp);
                        }
                        cur = Some(wp);
                    }
                    "HeartRateBpm" => in_heart_rate = true,
                    _ => {}
                }
                stack.push(name);
            }
            Ok(Event::Empty(e)) => {
                // A self-closing <trkpt lat=".." lon=".."/> carries only attributes.
                let name = local_name(e.name().as_ref());
                if name == "trkpt" {
                    let mut wp = Waypoint::default();
                    read_latlon_attrs(&e, &mut wp);
                    track.points.push(wp);
                }
            }
            Ok(Event::Text(t)) => {
                let text = t.decode().map(|s| s.trim().to_string()).unwrap_or_default();
                if text.is_empty() {
                    buf.clear();
                    continue;
                }
                if let (Some(field), Some(wp)) = (stack.last(), cur.as_mut()) {
                    match field.as_str() {
                        "time" | "Time" => wp.time = parse_time(&text),
                        "ele" | "AltitudeMeters" => wp.ele = text.parse().ok(),
                        "LatitudeDegrees" => wp.lat = text.parse().unwrap_or(wp.lat),
                        "LongitudeDegrees" => wp.lon = text.parse().unwrap_or(wp.lon),
                        "hr" => wp.hr = text.parse().ok(),
                        "Value" if in_heart_rate => wp.hr = text.parse().ok(),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                match name.as_str() {
                    "HeartRateBpm" => in_heart_rate = false,
                    "trkpt" | "Trackpoint" => {
                        if let Some(wp) = cur.take() {
                            track.points.push(wp);
                        }
                    }
                    _ => {}
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(format!(
                    "XML parse error at byte {}: {e}",
                    reader.buffer_position()
                ));
            }
            _ => {}
        }
        buf.clear();
    }

    if track.points.is_empty() {
        return Err("No track points found (expected GPX <trkpt> or TCX <Trackpoint>).".into());
    }
    Ok(track)
}

fn read_latlon_attrs(e: &quick_xml::events::BytesStart, wp: &mut Waypoint) {
    for attr in e.attributes().flatten() {
        let key = local_name(attr.key.as_ref());
        // lat/lon attributes are plain ASCII numbers with no XML entities.
        let val = String::from_utf8_lossy(&attr.value);
        match key.as_str() {
            "lat" => wp.lat = val.parse().unwrap_or(0.0),
            "lon" => wp.lon = val.parse().unwrap_or(0.0),
            _ => {}
        }
    }
}

/// Strip any namespace prefix: `ns3:hr` -> `hr`.
fn local_name(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => s.to_string(),
    }
}

/// Parse an ISO-8601 / RFC-3339 timestamp into UTC.
fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GPX: &str = r#"<?xml version="1.0"?>
    <gpx xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">
      <trk><trkseg>
        <trkpt lat="59.3300" lon="18.0600">
          <ele>12.0</ele><time>2024-05-01T10:00:00Z</time>
          <extensions><gpxtpx:TrackPointExtension><gpxtpx:hr>140</gpxtpx:hr></gpxtpx:TrackPointExtension></extensions>
        </trkpt>
        <trkpt lat="59.3310" lon="18.0605">
          <ele>13.5</ele><time>2024-05-01T10:00:10Z</time>
        </trkpt>
      </trkseg></trk>
    </gpx>"#;

    const TCX: &str = r#"<?xml version="1.0"?>
    <TrainingCenterDatabase><Activities><Activity><Lap><Track>
      <Trackpoint>
        <Time>2024-05-01T10:00:00Z</Time>
        <Position><LatitudeDegrees>59.3300</LatitudeDegrees><LongitudeDegrees>18.0600</LongitudeDegrees></Position>
        <AltitudeMeters>12.0</AltitudeMeters>
        <HeartRateBpm><Value>140</Value></HeartRateBpm>
      </Trackpoint>
      <Trackpoint>
        <Time>2024-05-01T10:00:10Z</Time>
        <Position><LatitudeDegrees>59.3310</LatitudeDegrees><LongitudeDegrees>18.0605</LongitudeDegrees></Position>
      </Trackpoint>
    </Track></Lap></Activity></Activities></TrainingCenterDatabase>"#;

    #[test]
    fn parses_gpx_with_hr() {
        let t = parse_track(GPX.as_bytes()).unwrap();
        assert_eq!(t.len(), 2);
        assert!((t.points[0].lat - 59.33).abs() < 1e-6);
        assert_eq!(t.points[0].hr, Some(140));
        assert!(t.points[0].time.is_some());
    }

    #[test]
    fn parses_tcx_with_hr() {
        let t = parse_track(TCX.as_bytes()).unwrap();
        assert_eq!(t.len(), 2);
        assert!((t.points[1].lon - 18.0605).abs() < 1e-6);
        assert_eq!(t.points[0].hr, Some(140));
    }

    #[test]
    fn rejects_input_without_track_points() {
        assert!(parse_track(b"").is_err());
        assert!(parse_track(b"<gpx></gpx>").is_err());
        assert!(parse_track(b"not xml at all").is_err());
    }

    #[test]
    fn parses_self_closing_trkpt() {
        let gpx = r#"<gpx><trk><trkseg>
            <trkpt lat="59.33" lon="18.06"/>
            <trkpt lat="59.34" lon="18.07"/>
        </trkseg></trk></gpx>"#;
        let t = parse_track(gpx.as_bytes()).unwrap();
        assert_eq!(t.len(), 2);
        assert!((t.points[1].lat - 59.34).abs() < 1e-9);
        assert!(t.points[0].time.is_none());
    }

    #[test]
    fn optional_fields_stay_empty_when_absent() {
        let gpx = r#"<gpx><trk><trkseg>
            <trkpt lat="59.33" lon="18.06"><time>2024-05-01T10:00:00Z</time></trkpt>
        </trkseg></trk></gpx>"#;
        let t = parse_track(gpx.as_bytes()).unwrap();
        let p = &t.points[0];
        assert!(p.time.is_some());
        assert_eq!(p.ele, None);
        assert_eq!(p.hr, None);
    }

    #[test]
    fn malformed_values_degrade_gracefully() {
        // Bad lat parses to 0.0; bad time/ele are just dropped.
        let gpx = r#"<gpx><trk><trkseg>
            <trkpt lat="oops" lon="18.06"><ele>tall</ele><time>yesterday</time></trkpt>
        </trkseg></trk></gpx>"#;
        let t = parse_track(gpx.as_bytes()).unwrap();
        let p = &t.points[0];
        assert_eq!(p.lat, 0.0);
        assert!((p.lon - 18.06).abs() < 1e-9);
        assert_eq!(p.ele, None);
        assert_eq!(p.time, None);
    }

    #[test]
    fn timestamps_with_offsets_convert_to_utc() {
        let gpx = r#"<gpx><trk><trkseg>
            <trkpt lat="59.33" lon="18.06"><time>2024-05-01T12:00:00+02:00</time></trkpt>
        </trkseg></trk></gpx>"#;
        let t = parse_track(gpx.as_bytes()).unwrap();
        let time = t.points[0].time.unwrap();
        assert_eq!(time.to_rfc3339(), "2024-05-01T10:00:00+00:00");
    }

    #[test]
    fn strips_namespace_prefixes() {
        assert_eq!(local_name(b"ns3:hr"), "hr");
        assert_eq!(local_name(b"hr"), "hr");
        assert_eq!(
            local_name(b"gpxtpx:TrackPointExtension"),
            "TrackPointExtension"
        );
    }
}
