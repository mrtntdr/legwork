use image::{ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

/// A colored route segment in image-pixel space: (start, end, RGBA).
pub type Segment = ((f64, f64), (f64, f64), [u8; 4]);

/// Render the map image with the colored route (and control markers) burned in,
/// returning PNG bytes at the map's native resolution.
pub fn render_png(
    image_bytes: &[u8],
    segments: &[Segment],
    markers: &[(f64, f64)],
) -> Result<Vec<u8>, String> {
    let mut img = image::load_from_memory(image_bytes)
        .map_err(|e| format!("decode image: {e}"))?
        .to_rgba8();

    for &(a, b, rgba) in segments {
        draw_thick_line(&mut img, a, b, rgba, 3);
    }
    for &m in markers {
        draw_disc(&mut img, m, 6, [20, 20, 20, 255]);
        draw_disc(&mut img, m, 4, [255, 255, 255, 255]);
    }

    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok(out.into_inner())
}

fn blend(dst: &mut Rgba<u8>, src: [u8; 4]) {
    let a = src[3] as f32 / 255.0;
    for i in 0..3 {
        dst[i] = (src[i] as f32 * a + dst[i] as f32 * (1.0 - a)) as u8;
    }
    dst[3] = 255;
}

fn put(img: &mut RgbaImage, x: i64, y: i64, color: [u8; 4]) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        let mut px = *img.get_pixel(x as u32, y as u32);
        blend(&mut px, color);
        img.put_pixel(x as u32, y as u32, px);
    }
}

fn draw_disc(img: &mut RgbaImage, center: (f64, f64), r: i64, color: [u8; 4]) {
    let (cx, cy) = (center.0.round() as i64, center.1.round() as i64);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                put(img, cx + dx, cy + dy, color);
            }
        }
    }
}

/// Bresenham line, stamping a small disc at each step for thickness.
fn draw_thick_line(
    img: &mut RgbaImage,
    a: (f64, f64),
    b: (f64, f64),
    color: [u8; 4],
    thickness: i64,
) {
    let (mut x0, mut y0) = (a.0.round() as i64, a.1.round() as i64);
    let (x1, y1) = (b.0.round() as i64, b.1.round() as i64);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let r = thickness / 2;
    loop {
        draw_disc(img, (x0 as f64, y0 as f64), r, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32x32 white PNG to draw on.
    fn white_png() -> Vec<u8> {
        let img = RgbaImage::from_pixel(32, 32, Rgba([255, 255, 255, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn decode(png: &[u8]) -> RgbaImage {
        image::load_from_memory(png).unwrap().to_rgba8()
    }

    #[test]
    fn rejects_undecodable_image() {
        assert!(render_png(b"not an image", &[], &[]).is_err());
    }

    #[test]
    fn output_keeps_native_resolution() {
        let png = render_png(&white_png(), &[], &[]).unwrap();
        let img = decode(&png);
        assert_eq!((img.width(), img.height()), (32, 32));
    }

    #[test]
    fn segments_are_burned_in() {
        let segments = [((4.0, 16.0), (28.0, 16.0), [255, 0, 0, 255])];
        let png = render_png(&white_png(), &segments, &[]).unwrap();
        let img = decode(&png);
        // A pixel on the line is red; a far corner is untouched white.
        let on_line = img.get_pixel(16, 16);
        assert_eq!(on_line.0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn markers_draw_dark_ring_with_white_core() {
        let png = render_png(&white_png(), &[], &[(16.0, 16.0)]).unwrap();
        let img = decode(&png);
        assert_eq!(img.get_pixel(16, 16).0, [255, 255, 255, 255]); // white core
        assert_eq!(img.get_pixel(16, 22).0, [20, 20, 20, 255]); // dark ring at r=6
    }

    #[test]
    fn out_of_bounds_drawing_does_not_panic() {
        let segments = [((-100.0, -100.0), (200.0, 200.0), [0, 255, 0, 255])];
        let markers = [(-50.0, 10.0), (1000.0, 1000.0)];
        let png = render_png(&white_png(), &segments, &markers).unwrap();
        assert!(!png.is_empty());
    }

    #[test]
    fn semi_transparent_color_blends() {
        let mut px = Rgba([100, 100, 100, 255]);
        blend(&mut px, [200, 0, 0, 128]);
        // ~50/50 mix of 100 and 200 in red, alpha forced opaque.
        assert!((px[0] as i32 - 150).abs() <= 2, "r {}", px[0]);
        assert!(px[1] < 100 && px[2] < 100);
        assert_eq!(px[3], 255);
    }
}
