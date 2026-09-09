//! 已验证文件句柄的有界缩略图解码，以及图像证据编码。

use crate::llm::ImageData;

pub fn image_thumbnail_file(file: &std::fs::File, max_px: u32) -> Option<ImageData> {
    let mut reader = image::ImageReader::new(std::io::BufReader::new(file))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    // 计算缩放到 max_px 的尺寸（保持比例）
    let scale = (max_px as f64) / (w.max(h) as f64);
    let img = if scale < 1.0 {
        let nw = ((w as f64 * scale).max(1.0)) as u32;
        let nh = ((h as f64 * scale).max(1.0)) as u32;
        img.resize(nw, nh, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    jpeg_data(&img, 75)
}

pub(crate) fn jpeg_data(img: &image::DynamicImage, quality: u8) -> Option<ImageData> {
    let mut buf = Vec::new();
    let mut encode = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    let rgba = img.to_rgb8();
    encode
        .encode_image(&image::DynamicImage::ImageRgb8(rgba))
        .ok()?;
    let data_base64 = base64_encode(&buf);
    Some(ImageData {
        high_detail: false,
        data_base64,
        mime: "image/jpeg".into(),
    })
}

// 简陋 base64 编码（避免引入 base64 crate 依赖）
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod thumb_tests {
    use super::*;

    #[test]
    fn thumbnail_small_png() {
        // 用 image crate 生成一张 200x100 PNG 再缩略
        let dir = std::env::temp_dir().join(format!("ds_thumb_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.png");
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            200,
            100,
            image::Rgba([10, 200, 30, 255]),
        ));
        img.save(&path).unwrap();

        let t = image_thumbnail_file(&std::fs::File::open(&path).unwrap(), 128);
        assert!(t.is_some(), "应生成缩略图");
        let t = t.unwrap();
        assert_eq!(t.mime, "image/jpeg");
        assert!(
            t.data_base64.starts_with("/9j/"),
            "JPEG base64 头应为 /9j/: {}",
            &t.data_base64[..8]
        );
        assert!(t.data_base64.len() < 50_000, "缩略图 base64 应受限");

        // 非图片文件 → None
        let f = dir.join("a.txt");
        std::fs::write(&f, b"hello").unwrap();
        assert!(image_thumbnail_file(&std::fs::File::open(&f).unwrap(), 128).is_none());
    }
}
