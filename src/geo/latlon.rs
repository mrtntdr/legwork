//! Parsing and formatting of WGS84 coordinates typed by hand.
//!
//! Map corner coordinates get read off a printed map's margin, a course-setting
//! program, or a map service, so they arrive in whatever notation that source
//! uses: decimal degrees, degrees + decimal minutes, or full degrees/minutes/
//! seconds, with the hemisphere as a sign or as an N/S/E/W letter before or
//! after the number. `parse_latlon` accepts all of those in one field, so the
//! user can paste rather than convert.

/// One lexical token of a coordinate string: a number, or a hemisphere letter.
enum Tok {
    Num(f64),
    Hemi(char),
}

/// Split a coordinate string into numbers and hemisphere letters. Degree,
/// minute and second marks — and any other punctuation — are separators.
fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut num = String::new();
    // A sign only opens a number; inside one it's a separator, so "59-19-55 N"
    // reads as degrees-minutes-seconds rather than a subtraction.
    for ch in s.chars().chain(std::iter::once(' ')) {
        let part_of_number =
            ch.is_ascii_digit() || ch == '.' || (ch == '-' || ch == '+') && num.is_empty();
        if part_of_number {
            num.push(ch);
            continue;
        }
        if !num.is_empty() {
            out.push(Tok::Num(num.parse().ok()?));
            num.clear();
        }
        match ch.to_ascii_uppercase() {
            c @ ('N' | 'S' | 'E' | 'W') => out.push(Tok::Hemi(c)),
            // Any other letter means this isn't a coordinate at all.
            c if c.is_alphabetic() => return None,
            _ => {}
        }
    }
    Some(out)
}

/// Degrees / degrees+minutes / degrees+minutes+seconds collapsed to one value.
/// The sign rides on the degrees part, as in "-59 19 55".
fn to_degrees(nums: &[f64]) -> Option<f64> {
    let (d, sign) = match nums.first() {
        Some(&d) => (d.abs(), if d.is_sign_negative() { -1.0 } else { 1.0 }),
        None => return None,
    };
    let value = match nums.len() {
        1 => d,
        2 => d + nums[1] / 60.0,
        3 => d + nums[1] / 60.0 + nums[2] / 3600.0,
        _ => return None,
    };
    Some(sign * value)
}

/// Parse a "lat, lon" string into decimal degrees.
///
/// Understands `59.3321, 18.0654`, `N 59.3321 E 18.0654`, `59°19'55.6"N
/// 18°03'55.4"E`, `59 19.93 N, 18 3.92 E` and the plain space-separated forms.
/// Returns `None` unless both halves parse and land inside ±90 / ±180.
pub fn parse_latlon(s: &str) -> Option<(f64, f64)> {
    // Group the tokens: a hemisphere letter closes the group it trails and opens
    // the one it leads, which covers both "59.33 N" and "N 59.33".
    let mut groups: Vec<(Vec<f64>, Option<char>)> = Vec::new();
    let mut nums: Vec<f64> = Vec::new();
    let mut hemi: Option<char> = None;
    for tok in tokenize(s)? {
        match tok {
            Tok::Num(n) => {
                if hemi.is_some() && !nums.is_empty() {
                    groups.push((std::mem::take(&mut nums), hemi.take()));
                }
                nums.push(n);
            }
            Tok::Hemi(c) => {
                if hemi.is_some() {
                    groups.push((std::mem::take(&mut nums), hemi.take()));
                }
                hemi = Some(c);
                if !nums.is_empty() {
                    groups.push((std::mem::take(&mut nums), hemi.take()));
                }
            }
        }
    }
    if !nums.is_empty() || hemi.is_some() {
        groups.push((nums, hemi));
    }

    // With no hemisphere letters the two halves aren't marked, so an even run of
    // numbers splits down the middle: 2 = decimal degrees, 4 = d+m, 6 = d+m+s.
    if groups.len() == 1 && groups[0].1.is_none() {
        let (nums, _) = groups.remove(0);
        if nums.len() % 2 != 0 || nums.is_empty() {
            return None;
        }
        let half = nums.len() / 2;
        groups.push((nums[..half].to_vec(), None));
        groups.push((nums[half..].to_vec(), None));
    }
    if groups.len() != 2 {
        return None;
    }

    let mut values = [0.0f64; 2];
    for (i, (nums, hemi)) in groups.iter().enumerate() {
        let v = to_degrees(nums)?;
        values[i] = match hemi {
            Some('S' | 'W') => -v.abs(),
            Some(_) => v.abs(),
            None => v,
        };
    }
    // An explicit E/W on the first half means the pair is given lon-first.
    let lon_first = matches!(groups[0].1, Some('E' | 'W'));
    let (lat, lon) = if lon_first {
        (values[1], values[0])
    } else {
        (values[0], values[1])
    };
    (lat.abs() <= 90.0 && lon.abs() <= 180.0).then_some((lat, lon))
}

/// Decimal-degree rendering for the reference-point list. Five decimals is about
/// a meter — finer than anyone can point at a map corner.
pub fn format_latlon(lat: f64, lon: f64) -> String {
    format!("{lat:.5}, {lon:.5}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(got: Option<(f64, f64)>, lat: f64, lon: f64) {
        let (a, b) = got.expect("should parse");
        assert!((a - lat).abs() < 1e-4, "lat {a} vs {lat}");
        assert!((b - lon).abs() < 1e-4, "lon {b} vs {lon}");
    }

    #[test]
    fn decimal_degrees_in_the_usual_notations() {
        near(parse_latlon("59.3321, 18.0654"), 59.3321, 18.0654);
        near(parse_latlon("59.3321 18.0654"), 59.3321, 18.0654);
        near(parse_latlon("  59.3321;18.0654 "), 59.3321, 18.0654);
        near(parse_latlon("-33.8688, 151.2093"), -33.8688, 151.2093);
    }

    #[test]
    fn hemisphere_letters_before_or_after() {
        near(parse_latlon("59.3321N, 18.0654E"), 59.3321, 18.0654);
        near(parse_latlon("N 59.3321 E 18.0654"), 59.3321, 18.0654);
        near(parse_latlon("33.8688 S 151.2093 E"), -33.8688, 151.2093);
        near(parse_latlon("40.7128 N 74.0060 W"), 40.7128, -74.0060);
    }

    #[test]
    fn lon_first_when_the_letters_say_so() {
        near(parse_latlon("E 18.0654 N 59.3321"), 59.3321, 18.0654);
    }

    #[test]
    fn degrees_minutes_and_seconds() {
        near(
            parse_latlon("59°19'55.6\"N 18°03'55.4\"E"),
            59.33211,
            18.06539,
        );
        near(
            parse_latlon("59 19.9267 N, 18 3.9233 E"),
            59.33211,
            18.06539,
        );
        // No letters: an even run of numbers splits down the middle.
        near(parse_latlon("59 19 55.6 18 3 55.4"), 59.33211, 18.06539);
        near(parse_latlon("59 19.9267 18 3.9233"), 59.33211, 18.06539);
    }

    #[test]
    fn rejects_what_isnt_a_coordinate_pair() {
        assert!(parse_latlon("").is_none());
        assert!(
            parse_latlon("59.3321").is_none(),
            "one number is not a pair"
        );
        assert!(parse_latlon("59.3321, 18.0654, 12").is_none(), "odd count");
        assert!(parse_latlon("somewhere near the lake").is_none());
        assert!(
            parse_latlon("591.3, 18.06").is_none(),
            "latitude out of range"
        );
        assert!(
            parse_latlon("59.33, 1810.06").is_none(),
            "longitude out of range"
        );
    }

    #[test]
    fn formats_to_about_a_meter() {
        assert_eq!(format_latlon(59.332105, 18.065394), "59.33210, 18.06539");
    }
}
