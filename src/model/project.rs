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
    /// Point value for score-O / rogaine courses. `None` for a plain control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u32>,
}

impl CoursePoint {
    /// A control at an image-pixel position, with no score.
    pub fn at(x: f64, y: f64) -> Self {
        Self {
            image_px: [x, y],
            score: None,
        }
    }
}

/// A user-drawn route option, a polyline in original-image pixel coordinates.
/// Length and collected controls are derived at runtime, not stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DrawnRoute {
    pub points: Vec<[f64; 2]>,
    /// The course leg this variant belongs to (0-based, `selected_leg` indexing);
    /// `None` for a free-form measuring route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leg: Option<usize>,
    /// Optional user label; empty means auto-name ("A", "B", … / "Route n").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Optional override color; `None` cycles a palette by index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
}

/// V1 only: a control attached to a waypoint of the (single) track. Kept so old
/// projects still deserialize; converted to a `CoursePoint` on load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Control {
    pub track_index: usize,
}

/// Pan/zoom/rotation state for the map canvas, persisted so a reopened project
/// looks the same.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ViewState {
    /// Screen-space offset of the image origin (pixels).
    pub offset: [f32; 2],
    /// Image-pixel to screen-pixel scale.
    pub zoom: f32,
    /// Clockwise view rotation, in radians. Absent in projects saved before map
    /// rotation, where it defaults to 0 (north/image up).
    #[serde(default)]
    pub rotation: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            offset: [0.0, 0.0],
            zoom: 1.0,
            rotation: 0.0,
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

/// Saved map georeferencing (from a world file or GeoTIFF): the pixel→world
/// affine `[a, b, c, d, e, f]` plus the world CRS, kept in the project because
/// sidecar files don't travel inside a `.legit` container.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeorefFile {
    pub px_to_world: [f64; 6],
    pub crs: CrsFile,
}

/// Serialized CRS of a map's world coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CrsFile {
    Geographic,
    TransverseMercator {
        lon0: f64,
        lat0: f64,
        k0: f64,
        false_e: f64,
        false_n: f64,
    },
    Unknown,
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
    /// Absent in projects saved before georeferencing support.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub georef: Option<GeorefFile>,
    /// User-drawn route options (analysis board). Absent in projects saved before
    /// the feature; empty projects serialize the same as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<DrawnRoute>,
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
        // Fields added after this schema default cleanly for old files.
        assert!(p.routes.is_empty());
        assert!(p.controls[0].score.is_none());
    }

    #[test]
    fn empty_routes_and_scoreless_controls_omit_their_keys() {
        // Byte-compat: a project with no routes and no scores must serialize the
        // same as before those fields existed.
        let p = ProjectFileV2 {
            version: 2,
            image_name: "map.png".into(),
            athletes: vec![],
            controls: vec![CoursePoint::at(1.0, 2.0)],
            active: 0,
            view: ViewState::default(),
            georef: None,
            routes: Vec::new(),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("routes"), "{json}");
        assert!(!json.contains("score"), "{json}");
    }

    #[test]
    fn routes_and_scores_round_trip() {
        let p = ProjectFileV2 {
            version: 2,
            image_name: "map.png".into(),
            athletes: vec![],
            controls: vec![CoursePoint {
                image_px: [1.0, 2.0],
                score: Some(30),
            }],
            active: 0,
            view: ViewState::default(),
            georef: None,
            routes: vec![
                DrawnRoute {
                    points: vec![[0.0, 0.0], [5.0, 5.0]],
                    leg: Some(2),
                    name: String::new(),
                    color: Some([10, 20, 30]),
                },
                DrawnRoute {
                    points: vec![[1.0, 1.0], [2.0, 2.0], [3.0, 1.0]],
                    leg: None,
                    name: "Long way".into(),
                    color: None,
                },
            ],
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: ProjectFileV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(back.controls[0].score, Some(30));
        assert_eq!(back.routes.len(), 2);
        assert_eq!(back.routes[0].leg, Some(2));
        assert_eq!(back.routes[0].color, Some([10, 20, 30]));
        assert_eq!(back.routes[1].name, "Long way");
        assert!(back.routes[1].leg.is_none());
    }
}
