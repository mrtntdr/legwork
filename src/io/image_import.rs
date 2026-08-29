use egui::ColorImage;

/// A decoded map image ready to upload as an egui texture, plus the original
/// encoded bytes (kept so the project container can store the file verbatim).
pub struct LoadedImage {
    pub color_image: ColorImage,
    pub size: [usize; 2],
    pub bytes: Vec<u8>,
}

/// Decode encoded image bytes (jpeg/png/gif/tiff/bmp/webp) into an egui image.
///
/// `size` is always the original pixel dimensions — the coordinate space controls
/// and calibration pins live in — even when the texture is downscaled for the GPU.
pub fn load_image(bytes: Vec<u8>) -> Result<LoadedImage, String> {
    let dynamic =
        image::load_from_memory(&bytes).map_err(|e| format!("Failed to decode image: {e}"))?;
    let size = [dynamic.width() as usize, dynamic.height() as usize];

    // Mobile/WebGL commonly caps textures at 4096² and rejects larger uploads with a
    // black map. Downscale the texture pixels to fit (aspect preserved); the quad is
    // still drawn at the original `size`, so the (blurrier) texture just stretches
    // over it and image-pixel coordinates stay exact. The original `bytes` are kept
    // verbatim for lossless save/export.
    #[cfg(target_arch = "wasm32")]
    let dynamic = {
        const MAX_TEX: u32 = 4096;
        if dynamic.width() > MAX_TEX || dynamic.height() > MAX_TEX {
            dynamic.resize(MAX_TEX, MAX_TEX, image::imageops::FilterType::Triangle)
        } else {
            dynamic
        }
    };

    let rgba = dynamic.to_rgba8();
    let tex_size = [rgba.width() as usize, rgba.height() as usize];
    let color_image = ColorImage::from_rgba_unmultiplied(tex_size, rgba.as_raw());
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
