//! IOF XML 3.0 course import (OCAD, Purple Pen, Condes exports): reads the
//! control positions (WGS84) and the first course's control order, so a course
//! can be placed on the map without clicking controls in by hand.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;

/// A parsed IOF course: ordered numbered-control positions (start/finish are
/// implicit in legwork) plus context for the status line.
#[derive(Debug)]
pub struct CourseImport {
    /// Name of the imported course, when the file has courses.
    pub course_name: Option<String>,
    /// (lat, lon) of each control in course order.
    pub controls: Vec<(f64, f64)>,
    /// Number of courses in the file (the first is imported).
    pub n_courses: usize,
    /// Controls skipped because the file carries no geo position for them
    /// (e.g. Purple Pen exports from a non-georeferenced map).
    pub skipped: usize,
}

/// Parse IOF XML 3.0 `CourseData`. Uses the first `Course`'s control order; a
/// file with only a control list imports all its numbered controls in file order.
pub fn parse_iof_course(bytes: &[u8]) -> Result<CourseImport, String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    // Top-level control list: code → position; codes without a Position are
    // remembered so they can be counted when a course references them.
    let mut positions: HashMap<String, (f64, f64)> = HashMap::new();
    let mut listed: Vec<String> = Vec::new(); // numbered controls, file order
    let mut courses: Vec<(Option<String>, Vec<String>)> = Vec::new();

    // Parser state.
    let mut path: Vec<String> = Vec::new();
    let mut control_id = String::new();
    let mut control_pos: Option<(f64, f64)> = None;
    let mut control_kind_numbered = true;
    let mut course_name: Option<String> = None;
    let mut course_codes: Vec<String> = Vec::new();
    let mut cc_numbered = true;
    let mut cc_code = String::new();
    let mut text = String::new();

    let local = |name: &[u8]| -> String {
        let s = String::from_utf8_lossy(name);
        s.rsplit(':').next().unwrap_or(&s).to_string()
    };
    let type_attr = |e: &BytesStart| -> Option<String> {
        e.attributes().flatten().find_map(|a| {
            (local(a.key.as_ref()) == "type")
                .then(|| String::from_utf8_lossy(&a.value).to_string())
        })
    };

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local(e.name().as_ref());
                match name.as_str() {
                    // A control definition directly under the course data (not a
                    // course's reference, which is also named `Control`).
                    "Control" if !path.iter().any(|p| p == "Course") => {
                        control_id.clear();
                        control_pos = None;
                        control_kind_numbered = !matches!(
                            type_attr(&e).as_deref(),
                            Some("Start") | Some("Finish") | Some("CrossingPoint")
                                | Some("EndOfMarkedRoute")
                        );
                    }
                    "Course" => {
                        course_name = None;
                        course_codes = Vec::new();
                    }
                    "CourseControl" => {
                        cc_code.clear();
                        cc_numbered = !matches!(
                            type_attr(&e).as_deref(),
                            Some("Start") | Some("Finish")
                        );
                    }
                    _ => {}
                }
                path.push(name);
                text.clear();
            }
            Ok(Event::Empty(e)) => {
                if local(e.name().as_ref()) == "Position"
                    && path.last().is_some_and(|p| p == "Control")
                    && !path.iter().any(|p| p == "Course")
                {
                    let mut lat = None;
                    let mut lon = None;
                    for a in e.attributes().flatten() {
                        let v = String::from_utf8_lossy(&a.value).parse::<f64>().ok();
                        match local(a.key.as_ref()).as_str() {
                            "lat" => lat = v,
                            "lng" => lon = v,
                            _ => {}
                        }
                    }
                    if let (Some(lat), Some(lon)) = (lat, lon) {
                        control_pos = Some((lat, lon));
                    }
                }
            }
            Ok(Event::Text(t)) => {
                text = t.decode().map_err(|e| e.to_string())?.trim().to_string();
            }
            Ok(Event::End(e)) => {
                let name = local(e.name().as_ref());
                let in_course = path.iter().any(|p| p == "Course");
                match name.as_str() {
                    "Id" if path.get(path.len().saturating_sub(2))
                        .is_some_and(|p| p == "Control")
                        && !in_course =>
                    {
                        control_id = text.clone();
                    }
                    "Control" if !in_course => {
                        if !control_id.is_empty() {
                            if let Some(pos) = control_pos {
                                positions.insert(control_id.clone(), pos);
                            }
                            if control_kind_numbered {
                                listed.push(control_id.clone());
                            }
                        }
                    }
                    // Inside a CourseControl, `<Control>code</Control>` is the reference.
                    "Control" if in_course => {
                        cc_code = text.clone();
                    }
                    "CourseControl" => {
                        if cc_numbered && !cc_code.is_empty() {
                            course_codes.push(cc_code.clone());
                        }
                    }
                    "Name" if path.get(path.len().saturating_sub(2))
                        .is_some_and(|p| p == "Course") =>
                    {
                        course_name = Some(text.clone());
                    }
                    "Course" => {
                        courses.push((course_name.take(), std::mem::take(&mut course_codes)));
                    }
                    _ => {}
                }
                path.pop();
                text.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {e}")),
            _ => {}
        }
    }

    // Course order when available, otherwise the listed numbered controls.
    let (course_name, codes) = match courses.first() {
        Some((name, codes)) if !codes.is_empty() => (name.clone(), codes.clone()),
        _ => (None, listed),
    };
    if codes.is_empty() {
        return Err("No controls found in the IOF XML file.".into());
    }
    let mut controls = Vec::new();
    let mut skipped = 0;
    for code in &codes {
        match positions.get(code) {
            Some(&pos) => controls.push(pos),
            None => skipped += 1,
        }
    }
    if controls.is_empty() {
        return Err(
            "The IOF XML file has no geo positions for its controls (the map used for \
             course setting was probably not georeferenced)."
                .into(),
        );
    }
    Ok(CourseImport {
        course_name,
        controls,
        n_courses: courses.len(),
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<CourseData xmlns="http://www.orienteering.org/datastandard/3.0" iofVersion="3.0">
  <RaceCourseData>
    <Control type="Start"><Id>S1</Id><Position lng="18.0600" lat="59.3300"/></Control>
    <Control><Id>31</Id><Position lng="18.0610" lat="59.3310"/></Control>
    <Control><Id>32</Id><Position lng="18.0620" lat="59.3320"/></Control>
    <Control><Id>33</Id></Control>
    <Control type="Finish"><Id>F1</Id><Position lng="18.0630" lat="59.3330"/></Control>
    <Course>
      <Name>Long</Name>
      <CourseControl type="Start"><Control>S1</Control></CourseControl>
      <CourseControl><Control>32</Control></CourseControl>
      <CourseControl><Control>31</Control></CourseControl>
      <CourseControl><Control>33</Control></CourseControl>
      <CourseControl type="Finish"><Control>F1</Control></CourseControl>
    </Course>
    <Course>
      <Name>Short</Name>
      <CourseControl type="Start"><Control>S1</Control></CourseControl>
      <CourseControl><Control>31</Control></CourseControl>
      <CourseControl type="Finish"><Control>F1</Control></CourseControl>
    </Course>
  </RaceCourseData>
</CourseData>"#;

    #[test]
    fn first_course_order_wins_and_start_finish_are_dropped() {
        let c = parse_iof_course(SAMPLE.as_bytes()).unwrap();
        assert_eq!(c.course_name.as_deref(), Some("Long"));
        assert_eq!(c.n_courses, 2);
        // Course order 32 → 31 (33 has no position and is skipped).
        assert_eq!(
            c.controls,
            vec![(59.3320, 18.0620), (59.3310, 18.0610)]
        );
        assert_eq!(c.skipped, 1);
    }

    #[test]
    fn control_list_without_courses_imports_in_file_order() {
        let xml = r#"<CourseData><RaceCourseData>
            <Control type="Start"><Id>S1</Id><Position lng="18.0" lat="59.0"/></Control>
            <Control><Id>31</Id><Position lng="18.1" lat="59.1"/></Control>
            <Control><Id>32</Id><Position lng="18.2" lat="59.2"/></Control>
        </RaceCourseData></CourseData>"#;
        let c = parse_iof_course(xml.as_bytes()).unwrap();
        assert_eq!(c.course_name, None);
        assert_eq!(c.n_courses, 0);
        // Start excluded; numbered controls in listed order.
        assert_eq!(c.controls, vec![(59.1, 18.1), (59.2, 18.2)]);
    }

    #[test]
    fn missing_positions_everywhere_is_an_error() {
        let xml = r#"<CourseData><RaceCourseData>
            <Control><Id>31</Id></Control>
        </RaceCourseData></CourseData>"#;
        let err = parse_iof_course(xml.as_bytes()).unwrap_err();
        assert!(err.contains("geo positions"), "{err}");
    }

    #[test]
    fn empty_file_is_an_error() {
        assert!(parse_iof_course(b"<CourseData/>").is_err());
    }

    #[test]
    fn namespaced_tags_are_handled() {
        let xml = r#"<iof:CourseData xmlns:iof="urn:x"><iof:RaceCourseData>
            <iof:Control><iof:Id>31</iof:Id><iof:Position lng="18.1" lat="59.1"/></iof:Control>
        </iof:RaceCourseData></iof:CourseData>"#;
        let c = parse_iof_course(xml.as_bytes()).unwrap();
        assert_eq!(c.controls, vec![(59.1, 18.1)]);
    }
}
