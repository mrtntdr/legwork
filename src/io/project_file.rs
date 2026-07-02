use crate::model::ProjectFile;
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;

/// The three pieces that make up a loaded project: metadata plus the raw
/// image and track file bytes.
#[derive(Debug)]
pub struct ProjectBundle {
    pub project: ProjectFile,
    pub image_bytes: Vec<u8>,
    pub track_bytes: Vec<u8>,
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
    put(&bundle.project.track_name, &bundle.track_bytes)?;

    let cursor = zip.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

/// Read a `.legit` zip container back into its parts.
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
    let project: ProjectFile = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
    let image_bytes = read_entry(&mut archive, &project.image_name)?;
    let track_bytes = read_entry(&mut archive, &project.track_name)?;

    Ok(ProjectBundle {
        project,
        image_bytes,
        track_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CalibrationPoint, Control, ViewState};

    fn sample_bundle() -> ProjectBundle {
        ProjectBundle {
            project: ProjectFile {
                image_name: "map.png".into(),
                track_name: "run.gpx".into(),
                calibration: vec![CalibrationPoint {
                    track_index: 7,
                    image_px: [120.5, 300.25],
                }],
                controls: vec![Control { track_index: 42 }],
                view: ViewState {
                    offset: [10.0, -20.0],
                    zoom: 1.5,
                },
            },
            image_bytes: vec![1, 2, 3, 4],
            track_bytes: b"<gpx/>".to_vec(),
        }
    }

    #[test]
    fn bundle_round_trips_through_zip() {
        let bytes = write_bundle(&sample_bundle()).unwrap();
        let back = read_bundle(&bytes).unwrap();
        assert_eq!(back.project.image_name, "map.png");
        assert_eq!(back.project.track_name, "run.gpx");
        assert_eq!(back.project.calibration.len(), 1);
        assert_eq!(back.project.calibration[0].track_index, 7);
        assert_eq!(back.project.calibration[0].image_px, [120.5, 300.25]);
        assert_eq!(back.project.controls[0].track_index, 42);
        assert_eq!(back.project.view.zoom, 1.5);
        assert_eq!(back.image_bytes, vec![1, 2, 3, 4]);
        assert_eq!(back.track_bytes, b"<gpx/>".to_vec());
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
}
