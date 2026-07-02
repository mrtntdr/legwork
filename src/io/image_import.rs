use egui::ColorImage;

/// A decoded map image ready to upload as an egui texture, plus the original
/// encoded bytes (kept so the project container can store the file verbatim).
pub struct LoadedImage {
    pub color_image: ColorImage,
    pub size: [usize; 2],
    pub bytes: Vec<u8>,
}

/// Decode encoded image bytes (jpeg/png/gif/tiff/bmp/webp) into an egui image.
pub fn load_image(bytes: Vec<u8>) -> Result<LoadedImage, String> {
    let dynamic =
        image::load_from_memory(&bytes).map_err(|e| format!("Failed to decode image: {e}"))?;
    let rgba = dynamic.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color_image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Ok(LoadedImage {
        color_image,
        size,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decodes_png_and_keeps_original_bytes() {
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        let bytes = out.into_inner();

        let loaded = load_image(bytes.clone()).unwrap();
        assert_eq!(loaded.size, [3, 2]);
        assert_eq!(loaded.color_image.size, [3, 2]);
        assert_eq!(loaded.bytes, bytes); // original bytes kept verbatim
    }

    #[test]
    fn rejects_garbage() {
        assert!(load_image(b"not an image".to_vec()).is_err());
    }
}
