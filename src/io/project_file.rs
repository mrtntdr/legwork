use crate::athlete::ATHLETE_COLORS;
use crate::model::{AnyProjectFile, AthleteFile, ProjectFileV2};
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;

/// The pieces that make up a loaded project: metadata plus the raw image and
/// per-athlete track file bytes.
#[derive(Debug)]
pub struct ProjectBundle {
    pub project: ProjectFileV2,
    pub image_bytes: Vec<u8>,
    /// Raw track file bytes, parallel to `project.athletes`.
    pub tracks: Vec<Vec<u8>>,
    /// Set only when a V1 (single-track) project was read: the old waypoint-index
    /// controls, to be converted to map positions by the caller once the athlete's
    /// transform exists.
    pub legacy_control_indices: Option<Vec<usize>>,
}

const PROJECT_JSON: &str = "project.json";

/// Serialize a project into a `.legit` zip container.
pub fn write_bundle(bundle: &ProjectBundle) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();

    let json = serde_json::to_vec_pretty(&bundle.project).map_err(|e| e.to_string())?;
    let mut put = |name: &str, data: &[u8]| -> Result<(), String> {
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(data).map_err(|e| e.to_string())
    };
    put(PROJECT_JSON, &json)?;
    put(&bundle.project.image_name, &bundle.image_bytes)?;
    for (athlete, track) in bundle.project.athletes.iter().zip(&bundle.tracks) {
        put(&athlete.track_entry, track)?;
    }

    let cursor = zip.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

/// Read a `.legit` zip container back into its parts. Old single-track projects
/// are lifted into a one-athlete V2 with `legacy_control_indices` set.
pub fn read_bundle(bytes: &[u8]) -> Result<ProjectBundle, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;

    let read_entry =
        |archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str| -> Result<Vec<u8>, String> {
            let mut file = archive
                .by_name(name)
                .map_err(|e| format!("missing '{name}': {e}"))?;
            let mut out = Vec::new();
            file.read_to_end(&mut out).map_err(|e| e.to_string())?;
            Ok(out)
        };

    let json = read_entry(&mut archive, PROJECT_JSON)?;
    let any: AnyProjectFile = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
    let (project, legacy_control_indices) = match any {
        AnyProjectFile::V2(p) => (p, None),
        AnyProjectFile::V1(p) => {
            let c = ATHLETE_COLORS[0];
            let athlete = AthleteFile {
                name: file_stem(&p.track_name),
                color: [c.r(), c.g(), c.b()],
                visible: true,
                track_entry: p.track_name,
                calibration: p.calibration,
            };
            let controls = p.controls.iter().map(|c| c.track_index).collect();
            (
                ProjectFileV2 {
                    version: 2,
                    image_name: p.image_name,
                    athletes: vec![athlete],
                    controls: Vec::new(),
                    active: 0,
                    view: p.view,
                    georef: None,
                },
                Some(controls),
            )
        }
    };

    let image_bytes = read_entry(&mut archive, &project.image_name)?;
    let tracks = project
        .athletes
        .iter()
        .map(|a| read_entry(&mut archive, &a.track_entry))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProjectBundle {
        project,
        image_bytes,
        tracks,
        legacy_control_indices,
    })
}

/// "run.gpx" -> "run"; leaves extension-less names untouched.
fn file_stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CalibrationPoint, CoursePoint, ViewState};

    fn sample_bundle() -> ProjectBundle {
        ProjectBundle {
            project: ProjectFileV2 {
                version: 2,
                image_name: "map.png".into(),
                athletes: vec![
                    AthleteFile {
                        name: "Anna".into(),
                        color: [230, 60, 60],
                        visible: true,
                        track_entry: "tracks/0/run.gpx".into(),
                        calibration: vec![CalibrationPoint {
                            track_index: 7,
                            image_px: [120.5, 300.25],
                        }],
                    },
                    AthleteFile {
                        name: "Bo".into(),
                        color: [70, 120, 250],
                        visible: false,
                        track_entry: "tracks/1/run.gpx".into(),
                        calibration: vec![],
                    },
                ],
                controls: vec![CoursePoint {
                    image_px: [42.0, 43.5],
                }],
                active: 1,
                view: ViewState {
                    offset: [10.0, -20.0],
                    zoom: 1.5,
                    rotation: 0.0,
                },
                georef: Some(crate::model::GeorefFile {
                    px_to_world: [2.0, 0.0, 650000.0, 0.0, -2.0, 6580000.0],
                    crs: crate::model::CrsFile::TransverseMercator {
                        lon0: 15.0,
                        lat0: 0.0,
                        k0: 0.9996,
                        false_e: 500_000.0,
                        false_n: 0.0,
                    },
                }),
            },
            image_bytes: vec![1, 2, 3, 4],
            tracks: vec![b"<gpx a/>".to_vec(), b"<gpx b/>".to_vec()],
            legacy_control_indices: None,
        }
    }

    #[test]
    fn bundle_round_trips_through_zip() {
        let bytes = write_bundle(&sample_bundle()).unwrap();
        let back = read_bundle(&bytes).unwrap();
        assert_eq!(back.project.image_name, "map.png");
        assert_eq!(back.project.athletes.len(), 2);
        assert_eq!(back.project.athletes[0].name, "Anna");
        assert_eq!(back.project.athletes[0].calibration[0].track_index, 7);
        assert_eq!(back.project.athletes[1].visible, false);
        assert_eq!(back.project.controls[0].image_px, [42.0, 43.5]);
        assert_eq!(back.project.active, 1);
        assert_eq!(back.project.view.zoom, 1.5);
        assert_eq!(back.image_bytes, vec![1, 2, 3, 4]);
        assert_eq!(back.tracks, vec![b"<gpx a/>".to_vec(), b"<gpx b/>".to_vec()]);
        assert!(back.legacy_control_indices.is_none());
        let georef = back.project.georef.expect("georef survives the round trip");
        assert_eq!(georef.px_to_world[2], 650000.0);
        assert!(matches!(
            georef.crs,
            crate::model::CrsFile::TransverseMercator { lon0, .. } if lon0 == 15.0
        ));
    }

    #[test]
    fn v1_container_loads_as_single_athlete_with_legacy_controls() {
        // Build a V1 zip by hand: project.json (old schema) + image + track.
        let v1_json = r#"{
            "image_name": "map.png",
            "track_name": "run.gpx",
            "calibration": [{ "track_index": 3, "image_px": [1.0, 2.0] }],
            "splits": [{ "track_index": 12 }, { "track_index": 30 }],
            "view": { "offset": [0.0, 0.0], "zoom": 2.0 }
        }"#;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        for (name, data) in [
            (PROJECT_JSON, v1_json.as_bytes()),
            ("map.png", b"img".as_slice()),
            ("run.gpx", b"<gpx/>".as_slice()),
        ] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();

        let back = read_bundle(&bytes).unwrap();
        assert_eq!(back.project.athletes.len(), 1);
        assert_eq!(back.project.athletes[0].name, "run");
        assert_eq!(back.project.athletes[0].track_entry, "run.gpx");
        assert_eq!(back.project.athletes[0].calibration.len(), 1);
        assert!(back.project.controls.is_empty());
        assert_eq!(back.legacy_control_indices, Some(vec![12, 30]));
        assert_eq!(back.tracks, vec![b"<gpx/>".to_vec()]);
        assert_eq!(back.project.view.zoom, 2.0);
    }

    #[test]
    fn rejects_non_zip_bytes() {
        assert!(read_bundle(b"definitely not a zip").is_err());
    }

    #[test]
    fn reports_missing_entries() {
        // A valid zip that lacks project.json.
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("something-else.txt", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hi").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let err = read_bundle(&bytes).unwrap_err();
        assert!(err.contains(PROJECT_JSON), "error was: {err}");
    }

    #[test]
    fn file_stem_strips_only_the_extension() {
        assert_eq!(file_stem("run.gpx"), "run");
        assert_eq!(file_stem("morning.run.tcx"), "morning.run");
        assert_eq!(file_stem("noext"), "noext");
        assert_eq!(file_stem(".hidden"), ".hidden");
    }
}
