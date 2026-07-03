use serde::{Deserialize, Serialize};

/// A calibration pin: a track waypoint dragged onto its matching map feature.
/// `image_px` is in original-image pixel coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibrationPoint {
    pub track_index: usize,
    pub image_px: [f64; 2],
}

/// A shared course control placed directly on the map, in original-image pixel
/// coordinates. The vector order is the course order; each athlete's track is
/// matched to its nearest pass by each control.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoursePoint {
    pub image_px: [f64; 2],
}

/// V1 only: a control attached to a waypoint of the (single) track. Kept so old
/// projects still deserialize; converted to a `CoursePoint` on load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Control {
    pub track_index: usize,
}

/// Pan/zoom state for the map canvas, persisted so a reopened project looks the same.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ViewState {
    /// Screen-space offset of the image origin (pixels).
    pub offset: [f32; 2],
    /// Image-pixel to screen-pixel scale.
    pub zoom: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            offset: [0.0, 0.0],
            zoom: 1.0,
        }
    }
}

/// One athlete's entry in a saved project. The original track file is stored as
/// its own zip entry named `track_entry`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AthleteFile {
    pub name: String,
    pub color: [u8; 3],
    pub visible: bool,
    pub track_entry: String,
    pub calibration: Vec<CalibrationPoint>,
}

/// The current project metadata stored as `project.json` inside a `.legit`
/// container. The map image and each athlete's track file are separate zip entries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectFileV2 {
    pub version: u32,
    pub image_name: String,
    pub athletes: Vec<AthleteFile>,
    pub controls: Vec<CoursePoint>,
    pub active: usize,
    pub view: ViewState,
}

/// The original single-track schema, kept so pre-multi-athlete projects still open.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectFileV1 {
    pub image_name: String,
    pub track_name: String,
    pub calibration: Vec<CalibrationPoint>,
    /// `alias = "splits"` keeps projects saved before this field was renamed loadable.
    #[serde(alias = "splits")]
    pub controls: Vec<Control>,
    pub view: ViewState,
}

/// Either schema, distinguished structurally: V2 has `athletes`, V1 requires
/// `track_name` (which V2 lacks), so `untagged` is unambiguous.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AnyProjectFile {
    V2(ProjectFileV2),
    V1(ProjectFileV1),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_splits_field_still_deserializes_into_controls() {
        // A project.json written before controls were renamed from "splits".
        let legacy = r#"{
            "image_name": "map.png",
            "track_name": "run.gpx",
            "calibration": [],
            "splits": [{ "track_index": 12 }],
            "view": { "offset": [0.0, 0.0], "zoom": 1.0 }
        }"#;
        let project: ProjectFileV1 = serde_json::from_str(legacy).unwrap();
        assert_eq!(project.controls.len(), 1);
        assert_eq!(project.controls[0].track_index, 12);
    }

    #[test]
    fn any_project_file_discriminates_versions() {
        let v1 = r#"{
            "image_name": "map.png",
            "track_name": "run.gpx",
            "calibration": [],
            "controls": [{ "track_index": 3 }],
            "view": { "offset": [0.0, 0.0], "zoom": 1.0 }
        }"#;
        assert!(matches!(
            serde_json::from_str::<AnyProjectFile>(v1).unwrap(),
            AnyProjectFile::V1(_)
        ));

        let v2 = r#"{
            "version": 2,
            "image_name": "map.png",
            "athletes": [{
                "name": "Anna",
                "color": [230, 60, 60],
                "visible": true,
                "track_entry": "tracks/0/run.gpx",
                "calibration": []
            }],
            "controls": [{ "image_px": [10.0, 20.0] }],
            "active": 0,
            "view": { "offset": [0.0, 0.0], "zoom": 1.0 }
        }"#;
        let AnyProjectFile::V2(p) = serde_json::from_str::<AnyProjectFile>(v2).unwrap() else {
            panic!("expected V2");
        };
        assert_eq!(p.athletes[0].name, "Anna");
        assert_eq!(p.controls[0].image_px, [10.0, 20.0]);
    }
}
