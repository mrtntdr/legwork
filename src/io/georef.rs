//! Map georeferencing: world-file sidecars (`.pgw`/`.jgw`/`.tfw`/`.wld` + `.prj`)
//! and embedded GeoTIFF tags. When present, they define an exact mapping from
//! WGS84 lat/lon to image pixels, so GPS tracks align with no manual calibration.

use crate::geo::{Correspondence, MapTransform, TmParams, tm_forward};
use crate::model::{CrsFile, GeorefFile, RefPoint};
use std::path::{Path, PathBuf};

/// The coordinate reference system of a map's world coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Crs {
    /// World coordinates are geographic degrees (x = lon, y = lat).
    Geographic,
    /// A transverse-Mercator grid (UTM and the national grids orienteering maps
    /// actually use: SWEREF 99 TM, ETRS89/UTM zones, TM35FIN, …).
    TransverseMercator(TmParams),
    /// A projected CRS we couldn't identify. Resolved later by testing whether
    /// the UTM zone under a loaded track lands inside the map's world extent.
    UnknownProjected,
}

/// A georeferenced raster: the pixel→world affine plus the world CRS.
///
/// Affine layout `[a, b, c, d, e, f]`: `x_world = a·col + b·row + c`,
/// `y_world = d·col + e·row + f` (pixel centers, world-file convention).
#[derive(Clone, Debug, PartialEq)]
pub struct MapGeoref {
    pub px_to_world: [f64; 6],
    pub crs: Crs,
}

impl MapGeoref {
    /// World coordinates of a pixel.
    pub fn px_to_world(&self, col: f64, row: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.px_to_world;
        (a * col + b * row + c, d * col + e * row + f)
    }

    /// Pixel position of a world coordinate (affine inverse).
    pub fn world_to_px(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let [a, b, c, d, e, f] = self.px_to_world;
        let det = a * e - b * d;
        if det.abs() < 1e-12 {
            return None;
        }
        let (dx, dy) = (x - c, y - f);
        Some(((e * dx - b * dy) / det, (a * dy - d * dx) / det))
    }

    /// Pixel position of a WGS84 lat/lon, if the CRS is known.
    pub fn latlon_to_px(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        let world = match self.crs {
            Crs::Geographic => (lon, lat),
            Crs::TransverseMercator(p) => tm_forward(p, lat, lon),
            Crs::UnknownProjected => return None,
        };
        self.world_to_px(world.0, world.1)
    }

    /// World-space bounding box of an image of the given pixel size.
    pub fn world_bounds(&self, width: f64, height: f64) -> (f64, f64, f64, f64) {
        let corners = [
            self.px_to_world(0.0, 0.0),
            self.px_to_world(width, 0.0),
            self.px_to_world(0.0, height),
            self.px_to_world(width, height),
        ];
        corners.iter().fold(
            (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
            |(minx, maxx, miny, maxy), &(x, y)| {
                (minx.min(x), maxx.max(x), miny.min(y), maxy.max(y))
            },
        )
    }

    /// Try to resolve an `UnknownProjected` CRS using a WGS84 point known to be on
    /// the map (a track centroid): if that point, projected through a UTM zone
    /// near the point, lands inside the map's world extent, the grid is (close
    /// enough to) that zone. Neighboring zones are tried too, because national
    /// grids reuse one zone's parameters across the whole country — e.g. all of
    /// Sweden is SWEREF 99 TM (zone 33 parameters) even where the nominal UTM
    /// zone is 32–35. Returns whether the CRS is now usable.
    pub fn resolve_crs(&mut self, lat: f64, lon: f64, width: f64, height: f64) -> bool {
        if self.crs != Crs::UnknownProjected {
            return true;
        }
        let (minx, maxx, miny, maxy) = self.world_bounds(width, height);
        let (mx, my) = ((maxx - minx) * 0.2, (maxy - miny) * 0.2);
        let inside = |x: f64, y: f64| {
            x >= minx - mx && x <= maxx + mx && y >= miny - my && y <= maxy + my
        };
        let nominal = TmParams::utm_for(lat, lon);
        for zone_shift in [0.0, -1.0, 1.0, -2.0, 2.0] {
            let candidate = TmParams {
                lon0: nominal.lon0 + zone_shift * 6.0,
                ..nominal
            };
            let (e, n) = tm_forward(candidate, lat, lon);
            if inside(e, n) {
                self.crs = Crs::TransverseMercator(candidate);
                return true;
            }
        }
        if inside(lon, lat) {
            self.crs = Crs::Geographic;
            return true;
        }
        false
    }

    /// Short human description of the CRS for the status bar.
    pub fn describe(&self) -> String {
        match self.crs {
            Crs::Geographic => "lat/lon".into(),
            Crs::TransverseMercator(p) => {
                format!("TM/UTM grid, central meridian {}°", p.lon0)
            }
            Crs::UnknownProjected => "projected grid, inferred from tracks".into(),
        }
    }

    /// RMS distance, in ground meters, between where each reference point claims
    /// to be and where this georeferencing actually puts it — how well the fit
    /// honors the coordinates the user typed. Only meaningful on a grid CRS
    /// (which is what `georef_from_points` produces); `None` otherwise.
    pub fn residual_m(&self, pts: &[RefPoint]) -> Option<f64> {
        let Crs::TransverseMercator(tm) = self.crs else {
            return None;
        };
        if pts.is_empty() {
            return None;
        }
        let sum: f64 = pts
            .iter()
            .map(|p| {
                let (e, n) = tm_forward(tm, p.lat, p.lon);
                let (ae, an) = self.px_to_world(p.image_px[0], p.image_px[1]);
                (ae - e).powi(2) + (an - n).powi(2)
            })
            .sum();
        Some((sum / pts.len() as f64).sqrt())
    }

    pub fn to_file(&self) -> GeorefFile {
        GeorefFile {
            px_to_world: self.px_to_world,
            crs: match self.crs {
                Crs::Geographic => CrsFile::Geographic,
                Crs::TransverseMercator(p) => CrsFile::TransverseMercator {
                    lon0: p.lon0,
                    lat0: p.lat0,
                    k0: p.k0,
                    false_e: p.false_e,
                    false_n: p.false_n,
                },
                Crs::UnknownProjected => CrsFile::Unknown,
            },
        }
    }

    pub fn from_file(f: &GeorefFile) -> MapGeoref {
        MapGeoref {
            px_to_world: f.px_to_world,
            crs: match f.crs {
                CrsFile::Geographic => Crs::Geographic,
                CrsFile::TransverseMercator {
                    lon0,
                    lat0,
                    k0,
                    false_e,
                    false_n,
                } => Crs::TransverseMercator(TmParams {
                    lon0,
                    lat0,
                    k0,
                    false_e,
                    false_n,
                }),
                CrsFile::Unknown => Crs::UnknownProjected,
            },
        }
    }
}

// --- Manual reference points ---------------------------------------------------

/// Georeference a map from hand-entered reference points: spots on the image
/// whose real-world coordinates the user knows — normally the map's corners, read
/// off the printed sheet's margin. This is the path for a plain photo or scan
/// with no world file and no GPS track to calibrate against.
///
/// The coordinates are projected into the UTM grid under their centroid, so the
/// world side comes out in meters and every downstream consumer (route measuring,
/// control placement, track overlay) follows the same well-supported
/// transverse-Mercator path a world file would. Two points fit a similarity —
/// rotation, uniform scale, translation, the honest model for a map sheet — and
/// three or more spread across the sheet a full affine, which also absorbs the
/// slight non-uniform stretch of a scan.
///
/// `None` below two points, or when they sit on top of each other and so say
/// nothing about scale or rotation.
pub fn georef_from_points(pts: &[RefPoint]) -> Option<MapGeoref> {
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len() as f64;
    let lat = pts.iter().map(|p| p.lat).sum::<f64>() / n;
    let lon = pts.iter().map(|p| p.lon).sum::<f64>() / n;
    let tm = TmParams::utm_for(lat, lon);

    // Fit pixels → (easting, *southing*). Negating the northing puts source and
    // destination in the same handedness (x right, y down), which is what the
    // orientation-preserving similarity fit expects — a map scan is never mirrored.
    let corr: Vec<Correspondence> = pts
        .iter()
        .map(|p| {
            let (e, north) = tm_forward(tm, p.lat, p.lon);
            ((p.image_px[0], p.image_px[1]), (e, -north))
        })
        .collect();
    let (span, off_line) = pixel_spread(pts);
    if span < 1e-9 {
        return None; // coincident points: no scale, no rotation, nothing to fit
    }
    // A full affine needs real two-dimensional spread. Points strung out along one
    // line (say, three ticks down the sheet's west edge) leave the across-line
    // scale free, so fit the similarity they *do* determine instead of a warp.
    let fit = if pts.len() >= 3 && off_line > span * 0.02 {
        MapTransform::fit_affine(&corr)
    } else {
        MapTransform::fit_similarity(&corr)
    };
    let MapTransform::Matrix(m) = fit? else {
        return None;
    };
    // Undo the northing flip on the second row.
    let px_to_world = [
        m[(0, 0)],
        m[(0, 1)],
        m[(0, 2)],
        -m[(1, 0)],
        -m[(1, 1)],
        -m[(1, 2)],
    ];
    let det = px_to_world[0] * px_to_world[4] - px_to_world[1] * px_to_world[3];
    if !px_to_world.iter().all(|v| v.is_finite()) || det.abs() < 1e-12 {
        return None; // no usable inverse
    }
    Some(MapGeoref {
        px_to_world,
        crs: Crs::TransverseMercator(tm),
    })
}

/// Pixel-space geometry of a reference set: the distance between its two farthest
/// points, and how far the rest stray from the line joining them. The first says
/// whether there's any scale to read at all, the second whether the set pins down
/// a full affine or only a similarity.
fn pixel_spread(pts: &[RefPoint]) -> (f64, f64) {
    let mut span = 0.0;
    let (mut a, mut b) = ([0.0; 2], [0.0; 2]);
    for (i, p) in pts.iter().enumerate() {
        for q in &pts[i + 1..] {
            let d = ((q.image_px[0] - p.image_px[0]).powi(2)
                + (q.image_px[1] - p.image_px[1]).powi(2))
            .sqrt();
            if d > span {
                span = d;
                (a, b) = (p.image_px, q.image_px);
            }
        }
    }
    if span <= 0.0 {
        return (0.0, 0.0);
    }
    let off = pts
        .iter()
        .map(|p| {
            let (px, py) = (p.image_px[0], p.image_px[1]);
            ((b[0] - a[0]) * (a[1] - py) - (a[0] - px) * (b[1] - a[1])).abs() / span
        })
        .fold(0.0, f64::max);
    (span, off)
}

// --- World files ---------------------------------------------------------------

/// Parse a 6-line ESRI world file (A, D, B, E, C, F order) into the affine.
pub fn parse_world_file(text: &str) -> Option<[f64; 6]> {
    let v: Vec<f64> = text
        .split_whitespace()
        .take(6)
        .map(|s| s.parse().ok())
        .collect::<Option<Vec<f64>>>()?;
    if v.len() < 6 {
        return None;
    }
    // World-file line order is A D B E C F; ours is [a, b, c, d, e, f].
    Some([v[0], v[2], v[4], v[1], v[3], v[5]])
}

/// Candidate world-file sidecar paths for an image: the derived three-letter
/// extension (jpg→jgw, png→pgw, tif→tfw…), `<ext>w`, and generic `.wld`.
pub fn world_file_candidates(image: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(ext) = image.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_ascii_lowercase();
        let chars: Vec<char> = ext.chars().collect();
        if chars.len() >= 2 {
            let derived: String = [chars[0], chars[chars.len() - 1], 'w'].iter().collect();
            out.push(image.with_extension(derived));
        }
        out.push(image.with_extension(format!("{ext}w")));
    }
    out.push(image.with_extension("wld"));
    out.dedup();
    out
}

/// Interpret a `.prj` WKT string: transverse-Mercator projections are extracted
/// with their parameters; a bare geographic CRS maps world coords to lon/lat.
pub fn parse_prj(wkt: &str) -> Crs {
    let lower = wkt.to_ascii_lowercase();
    if lower.contains("projcs") {
        if lower.contains("transverse_mercator") || lower.contains("transverse mercator") {
            let get = |name: &str, default: f64| wkt_parameter(&lower, name).unwrap_or(default);
            return Crs::TransverseMercator(TmParams {
                lon0: get("central_meridian", 0.0),
                lat0: get("latitude_of_origin", 0.0),
                k0: get("scale_factor", 1.0),
                false_e: get("false_easting", 0.0),
                false_n: get("false_northing", 0.0),
            });
        }
        return Crs::UnknownProjected;
    }
    if lower.contains("geogcs") || lower.contains("geodcrs") || lower.contains("geogcrs") {
        return Crs::Geographic;
    }
    Crs::UnknownProjected
}

/// Extract `PARAMETER["<name>", <value>]` from lowercased WKT.
fn wkt_parameter(lower: &str, name: &str) -> Option<f64> {
    let needle = format!("\"{name}\"");
    let at = lower.find(&needle)? + needle.len();
    let rest = &lower[at..];
    let comma = rest.find(',')? + 1;
    let end = rest[comma..].find([']', ','])? + comma;
    rest[comma..end].trim().parse().ok()
}

/// A world-file affine whose numbers look like degrees rather than meters
/// (tiny per-pixel scale, origin within lon/lat bounds).
fn looks_geographic(affine: &[f64; 6]) -> bool {
    affine[0].abs() < 0.01 && affine[2].abs() <= 360.0 && affine[5].abs() <= 90.0
}

/// Look for georeferencing next to (or inside) an image file: world-file sidecar
/// (+`.prj` for the CRS) first, then embedded GeoTIFF tags for TIFFs.
pub fn detect_georef(image_path: &Path, image_bytes: &[u8]) -> Option<MapGeoref> {
    for cand in world_file_candidates(image_path) {
        let Ok(text) = std::fs::read_to_string(&cand) else {
            continue;
        };
        let Some(affine) = parse_world_file(&text) else {
            continue;
        };
        let crs = match std::fs::read_to_string(image_path.with_extension("prj")) {
            Ok(wkt) => parse_prj(&wkt),
            Err(_) if looks_geographic(&affine) => Crs::Geographic,
            Err(_) => Crs::UnknownProjected,
        };
        return Some(MapGeoref {
            px_to_world: affine,
            crs,
        });
    }
    let ext = image_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if matches!(ext.as_deref(), Some("tif" | "tiff")) {
        return parse_geotiff(image_bytes);
    }
    None
}

// --- GeoTIFF -------------------------------------------------------------------

/// Minimal GeoTIFF reader: walks IFD0 of a classic TIFF for the georeferencing
/// tags (ModelPixelScale 33550, ModelTiepoint 33922, ModelTransformation 34264,
/// GeoKeyDirectory 34735) — just enough to place the raster, ignoring the rest.
pub fn parse_geotiff(bytes: &[u8]) -> Option<MapGeoref> {
    let tiff = Tiff::new(bytes)?;
    let affine = if let Some(m) = tiff.doubles(34264) {
        // ModelTransformation: row-major 4x4.
        if m.len() < 8 {
            return None;
        }
        [m[0], m[1], m[3], m[4], m[5], m[7]]
    } else {
        let scale = tiff.doubles(33550)?;
        let tie = tiff.doubles(33922)?;
        if scale.len() < 2 || tie.len() < 6 {
            return None;
        }
        let (sx, sy) = (scale[0], scale[1]);
        let (i, j, x, y) = (tie[0], tie[1], tie[3], tie[4]);
        // Raster rows grow south: y decreases by sy per row.
        [sx, 0.0, x - i * sx, 0.0, -sy, y + j * sy]
    };
    let crs = tiff
        .shorts(34735)
        .map(|keys| crs_from_geokeys(&keys))
        .unwrap_or(Crs::UnknownProjected);
    Some(MapGeoref {
        px_to_world: affine,
        crs,
    })
}

/// CRS from a GeoKeyDirectory: model type (1024) and the projected (3072) or
/// geographic (2048) EPSG code.
fn crs_from_geokeys(keys: &[u16]) -> Crs {
    let mut model_type = None;
    let mut projected_epsg = None;
    let mut geographic = false;
    // Header is 4 shorts, then entries of (key, tag_location, count, value).
    for entry in keys.iter().skip(4).collect::<Vec<_>>().chunks(4) {
        let [key, loc, _count, value] = entry else {
            break;
        };
        if **loc != 0 {
            continue; // value stored in another tag; not needed for these keys
        }
        match **key {
            1024 => model_type = Some(**value),
            2048 => geographic = true,
            3072 => projected_epsg = Some(**value),
            _ => {}
        }
    }
    if let Some(code) = projected_epsg
        && let Some(crs) = crs_from_epsg(code)
    {
        return crs;
    }
    if model_type == Some(2) || (geographic && projected_epsg.is_none()) {
        return Crs::Geographic;
    }
    Crs::UnknownProjected
}

/// Transverse-Mercator parameters for common EPSG codes: UTM north/south,
/// ETRS89/UTM, and the Nordic national grids. Anything else is inferred later.
fn crs_from_epsg(code: u16) -> Option<Crs> {
    let utm = |zone: u16, south: bool| {
        Crs::TransverseMercator(TmParams {
            lon0: zone as f64 * 6.0 - 183.0,
            lat0: 0.0,
            k0: 0.9996,
            false_e: 500_000.0,
            false_n: if south { 10_000_000.0 } else { 0.0 },
        })
    };
    Some(match code {
        32601..=32660 => utm(code - 32600, false),
        32701..=32760 => utm(code - 32700, true),
        25828..=25838 => utm(code - 25800, false), // ETRS89 / UTM
        3006 => utm(33, false),                    // SWEREF 99 TM
        3067 => utm(35, false),                    // ETRS89 / TM35FIN
        4326 => Crs::Geographic,
        _ => return None,
    })
}

/// Just enough classic-TIFF structure to read IFD0 tag values.
struct Tiff<'a> {
    bytes: &'a [u8],
    le: bool,
    /// (tag, type, count, value_or_offset_position) for each IFD0 entry.
    entries: Vec<(u16, u16, u32, usize)>,
}

impl<'a> Tiff<'a> {
    fn new(bytes: &'a [u8]) -> Option<Self> {
        let le = match bytes.get(0..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let u16_at = |at: usize| -> Option<u16> {
            let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
            Some(if le {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            })
        };
        let u32_at = |at: usize| -> Option<u32> {
            let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
            Some(if le {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            })
        };
        if u16_at(2)? != 42 {
            return None; // BigTIFF (43) not supported
        }
        let ifd = u32_at(4)? as usize;
        let n = u16_at(ifd)? as usize;
        let mut entries = Vec::with_capacity(n);
        for k in 0..n {
            let at = ifd + 2 + k * 12;
            entries.push((u16_at(at)?, u16_at(at + 2)?, u32_at(at + 4)?, at + 8));
        }
        Some(Tiff { bytes, le, entries })
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn f64_at(&self, at: usize) -> Option<f64> {
        let b: [u8; 8] = self.bytes.get(at..at + 8)?.try_into().ok()?;
        Some(if self.le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        })
    }

    fn entry(&self, tag: u16) -> Option<(u16, u32, usize)> {
        self.entries
            .iter()
            .find(|e| e.0 == tag)
            .map(|&(_, ty, count, pos)| (ty, count, pos))
    }

    /// Values of a DOUBLE (type 12) tag.
    fn doubles(&self, tag: u16) -> Option<Vec<f64>> {
        let (ty, count, pos) = self.entry(tag)?;
        if ty != 12 {
            return None;
        }
        // 8-byte doubles never fit inline; the value field is an offset.
        let start = self.u32_at(pos)? as usize;
        (0..count as usize)
            .map(|k| self.f64_at(start + k * 8))
            .collect()
    }

    /// Values of a SHORT (type 3) tag.
    fn shorts(&self, tag: u16) -> Option<Vec<u16>> {
        let (ty, count, pos) = self.entry(tag)?;
        if ty != 3 {
            return None;
        }
        let count = count as usize;
        let start = if count <= 2 {
            pos // up to two shorts fit inline
        } else {
            self.u32_at(pos)? as usize
        };
        (0..count).map(|k| self.u16_at(start + k * 2)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_file_parses_in_adbecf_order() {
        let text = "2.0\n0.0\n0.0\n-2.0\n650000.5\n6580000.5\n";
        let a = parse_world_file(text).unwrap();
        assert_eq!(a, [2.0, 0.0, 650000.5, 0.0, -2.0, 6580000.5]);
        let g = MapGeoref {
            px_to_world: a,
            crs: Crs::UnknownProjected,
        };
        assert_eq!(g.px_to_world(0.0, 0.0), (650000.5, 6580000.5));
        assert_eq!(g.px_to_world(10.0, 10.0), (650020.5, 6579980.5));
        let (col, row) = g.world_to_px(650020.5, 6579980.5).unwrap();
        assert!((col - 10.0).abs() < 1e-9 && (row - 10.0).abs() < 1e-9);
    }

    #[test]
    fn world_file_rejects_garbage() {
        assert!(parse_world_file("not numbers").is_none());
        assert!(parse_world_file("1.0 2.0 3.0").is_none());
    }

    #[test]
    fn sidecar_candidates_follow_convention() {
        let c = world_file_candidates(Path::new("/maps/forest.png"));
        assert!(c.contains(&PathBuf::from("/maps/forest.pgw")));
        assert!(c.contains(&PathBuf::from("/maps/forest.pngw")));
        assert!(c.contains(&PathBuf::from("/maps/forest.wld")));
        let c = world_file_candidates(Path::new("x.jpeg"));
        assert!(c.contains(&PathBuf::from("x.jgw")));
        let c = world_file_candidates(Path::new("x.tif"));
        assert!(c.contains(&PathBuf::from("x.tfw")));
    }

    #[test]
    fn prj_extracts_transverse_mercator_parameters() {
        // SWEREF99 TM-style WKT.
        let wkt = r#"PROJCS["SWEREF99 TM",GEOGCS["SWEREF99",DATUM["SWEREF99"]],
            PROJECTION["Transverse_Mercator"],
            PARAMETER["latitude_of_origin",0],
            PARAMETER["central_meridian",15],
            PARAMETER["scale_factor",0.9996],
            PARAMETER["false_easting",500000],
            PARAMETER["false_northing",0],UNIT["metre",1]]"#;
        let Crs::TransverseMercator(p) = parse_prj(wkt) else {
            panic!("expected TM");
        };
        assert_eq!(p.lon0, 15.0);
        assert_eq!(p.k0, 0.9996);
        assert_eq!(p.false_e, 500_000.0);
    }

    #[test]
    fn prj_geographic_and_unknown() {
        assert_eq!(
            parse_prj(r#"GEOGCS["WGS 84",DATUM["WGS_1984"]]"#),
            Crs::Geographic
        );
        assert_eq!(
            parse_prj(r#"PROJCS["Weird",PROJECTION["Lambert_Conformal_Conic"]]"#),
            Crs::UnknownProjected
        );
    }

    #[test]
    fn latlon_to_px_through_a_tm_grid() {
        // 1 m/px north-up grid anchored at the projection of a known point, so
        // that point must land exactly at pixel (0, 0).
        let p = TmParams {
            lon0: 15.0,
            lat0: 0.0,
            k0: 0.9996,
            false_e: 500_000.0,
            false_n: 0.0,
        };
        let (e0, n0) = tm_forward(p, 59.33, 18.06);
        let g = MapGeoref {
            px_to_world: [1.0, 0.0, e0, 0.0, -1.0, n0],
            crs: Crs::TransverseMercator(p),
        };
        let (col, row) = g.latlon_to_px(59.33, 18.06).unwrap();
        assert!(col.abs() < 1e-6 && row.abs() < 1e-6, "({col}, {row})");
        // A point ~100 m north lands ~100 px up (negative row).
        let (_, row_n) = g.latlon_to_px(59.3309, 18.06).unwrap();
        assert!((-row_n - 100.0).abs() < 2.0, "row {row_n}");
    }

    #[test]
    fn unknown_crs_resolves_to_the_track_utm_zone() {
        // Grid in UTM 33N covering a 2 km square around a Stockholm-ish point.
        let p = TmParams {
            lon0: 15.0,
            lat0: 0.0,
            k0: 0.9996,
            false_e: 500_000.0,
            false_n: 0.0,
        };
        let (e0, n0) = tm_forward(p, 59.34, 18.05);
        let mut g = MapGeoref {
            px_to_world: [2.0, 0.0, e0, 0.0, -2.0, n0],
            crs: Crs::UnknownProjected,
        };
        assert!(g.latlon_to_px(59.33, 18.06).is_none());
        assert!(g.resolve_crs(59.33, 18.06, 1000.0, 1000.0));
        assert!(matches!(g.crs, Crs::TransverseMercator(t) if t.lon0 == 15.0));
        assert!(g.latlon_to_px(59.33, 18.06).is_some());
        // A far-away point does not resolve.
        let mut far = MapGeoref {
            px_to_world: [2.0, 0.0, e0, 0.0, -2.0, n0],
            crs: Crs::UnknownProjected,
        };
        assert!(!far.resolve_crs(-33.9, 151.2, 1000.0, 1000.0));
    }

    /// Build a tiny little-endian classic TIFF containing only the georef tags.
    fn synthetic_geotiff(epsg: u16) -> Vec<u8> {
        let mut b: Vec<u8> = Vec::new();
        b.extend(b"II");
        b.extend(42u16.to_le_bytes());
        b.extend(8u32.to_le_bytes()); // IFD at byte 8
        // IFD: 3 entries.
        let n_entries = 3u16;
        b.extend(n_entries.to_le_bytes());
        let ifd_end = 8 + 2 + 12 * n_entries as usize + 4;
        let scale_off = ifd_end;
        let tie_off = scale_off + 3 * 8;
        let keys_off = tie_off + 6 * 8;
        // ModelPixelScale (33550, DOUBLE, 3)
        b.extend(33550u16.to_le_bytes());
        b.extend(12u16.to_le_bytes());
        b.extend(3u32.to_le_bytes());
        b.extend((scale_off as u32).to_le_bytes());
        // ModelTiepoint (33922, DOUBLE, 6)
        b.extend(33922u16.to_le_bytes());
        b.extend(12u16.to_le_bytes());
        b.extend(6u32.to_le_bytes());
        b.extend((tie_off as u32).to_le_bytes());
        // GeoKeyDirectory (34735, SHORT, 12)
        b.extend(34735u16.to_le_bytes());
        b.extend(3u16.to_le_bytes());
        b.extend(12u32.to_le_bytes());
        b.extend((keys_off as u32).to_le_bytes());
        b.extend(0u32.to_le_bytes()); // next IFD
        // Tag payloads.
        for v in [2.0f64, 2.0, 0.0] {
            b.extend(v.to_le_bytes()); // pixel scale
        }
        for v in [0.0f64, 0.0, 0.0, 650000.0, 6580000.0, 0.0] {
            b.extend(v.to_le_bytes()); // tiepoint: px(0,0) → world
        }
        for v in [
            1u16, 1, 0, 2, // header: version, rev, minor, 2 keys
            1024, 0, 1, 1, // GTModelType = projected
            3072, 0, 1, epsg, // ProjectedCSType
        ] {
            b.extend(v.to_le_bytes());
        }
        b
    }

    #[test]
    fn geotiff_tags_yield_affine_and_epsg_crs() {
        let g = parse_geotiff(&synthetic_geotiff(32633)).unwrap();
        assert_eq!(g.px_to_world, [2.0, 0.0, 650000.0, 0.0, -2.0, 6580000.0]);
        let Crs::TransverseMercator(p) = g.crs else {
            panic!("expected TM, got {:?}", g.crs);
        };
        assert_eq!(p.lon0, 15.0); // UTM zone 33
        assert_eq!(p.false_n, 0.0);
    }

    #[test]
    fn geotiff_unknown_epsg_falls_back_to_inference() {
        let g = parse_geotiff(&synthetic_geotiff(27700)).unwrap(); // OSGB — not in table
        assert_eq!(g.crs, Crs::UnknownProjected);
    }

    #[test]
    fn non_tiff_bytes_are_rejected() {
        assert!(parse_geotiff(b"PNG not tiff").is_none());
        assert!(parse_geotiff(b"").is_none());
    }

    #[test]
    fn detect_finds_pgw_sidecar_and_applies_degrees_heuristic() {
        let dir = std::env::temp_dir().join(format!("legwork-georef-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("map.png");
        std::fs::write(&img, b"fake image").unwrap();
        // Degree-scale numbers, no .prj → heuristic says geographic.
        std::fs::write(
            dir.join("map.pgw"),
            "0.000006\n0\n0\n-0.000005\n18.0592\n59.3333\n",
        )
        .unwrap();

        let g = detect_georef(&img, b"fake image").expect("sidecar found");
        assert_eq!(g.crs, Crs::Geographic);
        // The track start (59.33, 18.06) must land inside an 800x800 map.
        let (col, row) = g.latlon_to_px(59.33, 18.06).unwrap();
        assert!(
            col > 0.0 && col < 800.0 && row > 0.0 && row < 800.0,
            "({col}, {row})"
        );

        // A .prj beats the heuristic.
        std::fs::write(dir.join("map.prj"), r#"PROJCS["X",PROJECTION["Lambert_Conformal_Conic"]]"#)
            .unwrap();
        let g = detect_georef(&img, b"fake image").unwrap();
        assert_eq!(g.crs, Crs::UnknownProjected);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A reference point at an image pixel with a WGS84 position.
    fn rp(col: f64, row: f64, lat: f64, lon: f64) -> RefPoint {
        RefPoint {
            image_px: [col, row],
            lat,
            lon,
        }
    }

    #[test]
    fn two_opposite_corners_georeference_the_map() {
        // A 2000x1500 sheet whose NW and SE corners are known. The fit must put
        // each corner back on its own pixel and the CRS must be a usable grid.
        let (nw_lat, nw_lon) = (59.3400, 18.0500);
        let (se_lat, se_lon) = (59.3300, 18.0700);
        let pts = [
            rp(0.0, 0.0, nw_lat, nw_lon),
            rp(2000.0, 1500.0, se_lat, se_lon),
        ];
        let g = georef_from_points(&pts).expect("two points are enough");
        assert!(matches!(g.crs, Crs::TransverseMercator(_)));
        for p in &pts {
            let (col, row) = g.latlon_to_px(p.lat, p.lon).unwrap();
            assert!((col - p.image_px[0]).abs() < 1e-3, "col {col}");
            assert!((row - p.image_px[1]).abs() < 1e-3, "row {row}");
        }
        assert!(g.residual_m(&pts).unwrap() < 1e-3);

        // North is up and east is right: the map's own corners bracket the middle.
        let (col, row) = g
            .latlon_to_px((nw_lat + se_lat) / 2.0, (nw_lon + se_lon) / 2.0)
            .unwrap();
        assert!((col - 1000.0).abs() < 30.0, "center col {col}");
        assert!((row - 750.0).abs() < 30.0, "center row {row}");
    }

    #[test]
    fn four_corners_fit_and_measure_ground_distance() {
        // A north-up sheet: 0.01° of latitude tall (~1111 m) over 1000 rows.
        let pts = [
            rp(0.0, 0.0, 59.34, 18.05),
            rp(1000.0, 0.0, 59.34, 18.07),
            rp(1000.0, 1000.0, 59.33, 18.07),
            rp(0.0, 1000.0, 59.33, 18.05),
        ];
        let g = georef_from_points(&pts).expect("four points fit");
        // Each corner is honored to well under a meter (the affine is exact here
        // apart from the map projection's own curvature over the sheet).
        assert!(g.residual_m(&pts).unwrap() < 1.0);
        // Top edge to bottom edge is 0.01° of latitude ≈ 1111 m.
        let a = g.px_to_world(500.0, 0.0);
        let b = g.px_to_world(500.0, 1000.0);
        let dist = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        assert!((dist - 1111.0).abs() < 15.0, "{dist} m");
    }

    #[test]
    fn a_rotated_sheet_is_fitted_without_mirroring() {
        // Build a truth mapping that is a 20° rotation + scale of a north-up sheet,
        // sample three of its corners, and check the fit reproduces the fourth.
        let truth = MapGeoref {
            px_to_world: [
                2.0 * 0.9397,
                2.0 * 0.3420,
                650_000.0,
                2.0 * 0.3420,
                -2.0 * 0.9397,
                6_580_000.0,
            ],
            crs: Crs::TransverseMercator(TmParams::utm_for(59.3, 18.0)),
        };
        let corners = [(0.0, 0.0), (1200.0, 0.0), (1200.0, 900.0)];
        let pts: Vec<RefPoint> = corners
            .iter()
            .map(|&(col, row)| {
                let (e, n) = truth.px_to_world(col, row);
                let (lat, lon) = utm_inverse(&truth, e, n);
                rp(col, row, lat, lon)
            })
            .collect();
        let g = georef_from_points(&pts).expect("three points fit an affine");
        let (e, n) = truth.px_to_world(0.0, 900.0);
        let (lat, lon) = utm_inverse(&truth, e, n);
        let (col, row) = g.latlon_to_px(lat, lon).unwrap();
        assert!((col - 0.0).abs() < 1.0, "col {col}");
        assert!((row - 900.0).abs() < 1.0, "row {row}");
    }

    /// Invert a TM grid position back to lat/lon by Newton iteration on
    /// `tm_forward`, so the tests can build reference points from grid truth.
    fn utm_inverse(g: &MapGeoref, e: f64, n: f64) -> (f64, f64) {
        let Crs::TransverseMercator(tm) = g.crs else {
            panic!("grid CRS expected")
        };
        let (mut lat, mut lon) = (59.3, 18.0);
        for _ in 0..40 {
            let (ce, cn) = tm_forward(tm, lat, lon);
            // ~1 m per 9e-6° of latitude; longitude scales by cos(lat).
            lat += (n - cn) * 9e-6;
            lon += (e - ce) * 9e-6 / lat.to_radians().cos();
        }
        (lat, lon)
    }

    #[test]
    fn too_few_or_degenerate_points_give_no_georeferencing() {
        assert!(georef_from_points(&[]).is_none());
        assert!(georef_from_points(&[rp(0.0, 0.0, 59.34, 18.05)]).is_none());
        // Two points on the same pixel say nothing about scale or rotation.
        assert!(
            georef_from_points(&[rp(10.0, 10.0, 59.34, 18.05), rp(10.0, 10.0, 59.33, 18.07)])
                .is_none()
        );
    }

    #[test]
    fn collinear_points_fall_back_to_a_similarity() {
        // Three ticks along the sheet's north edge leave the north–south scale
        // free, so an affine would be under-determined — the similarity they do
        // determine is used instead, and it still honors every point.
        let pts = [
            rp(0.0, 0.0, 59.34, 18.05),
            rp(500.0, 0.0, 59.34, 18.06),
            rp(1000.0, 0.0, 59.34, 18.07),
        ];
        let g = georef_from_points(&pts).expect("a similarity still fits");
        assert!(g.residual_m(&pts).unwrap() < 1.0);
        // Uniform scale: half the width across is half the ground distance.
        let a = g.px_to_world(0.0, 0.0);
        let b = g.px_to_world(1000.0, 0.0);
        let c = g.px_to_world(0.0, 1000.0);
        let across = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let down = ((c.0 - a.0).powi(2) + (c.1 - a.1).powi(2)).sqrt();
        assert!((across - down).abs() < 1e-6, "{across} vs {down}");
    }

    #[test]
    fn detect_without_sidecar_is_none_for_non_tiff() {
        let dir = std::env::temp_dir().join(format!("legwork-georef2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("plain.png");
        std::fs::write(&img, b"fake image").unwrap();
        assert!(detect_georef(&img, b"fake image").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
