use serde::{Deserialize, Serialize};

/// A calibration pin: a track waypoint dragged onto its matching map feature.
/// `image_px` is in original-image pixel coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibrationPoint {
    pub track_index: usize,
    pub image_px: [f64; 2],
}

/// A manually placed control, attached to a track waypoint.
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

/// The serializable project metadata stored as `project.json` inside a `.legit` container.
/// The map image and the original track file are stored as separate zip entries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    pub image_name: String,
    pub track_name: String,
    pub calibration: Vec<CalibrationPoint>,
    /// `alias = "splits"` keeps projects saved before this field was renamed loadable.
    #[serde(alias = "splits")]
    pub controls: Vec<Control>,
    pub view: ViewState,
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
        let project: ProjectFile = serde_json::from_str(legacy).unwrap();
        assert_eq!(project.controls.len(), 1);
        assert_eq!(project.controls[0].track_index, 12);
    }
}
