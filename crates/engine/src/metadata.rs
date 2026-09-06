//! 轻量文件元数据提取。
//!
//! 不引入重型媒体解析依赖，仅做魔数检测 + 图片尺寸读取 + 文件类型识别。
//! 提取的元数据会拼入 LLM 分类/重命名 prompt，辅助决策。

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::llm::ImageData;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub file_type: String,
    /// 图片宽×高（像素），仅图片有效。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_dimensions: Option<(u32, u32)>,
    /// 音视频时长（秒），暂不支持提取（留接口位）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
}

/// 生成图片缩略图（≤max_px 边，保证 base64 规模受控），供多模态 prompt 使用。
/// 解码失败/非图片返回 None。输出 JPEG data URI。
pub fn image_thumbnail(path: &Path, max_px: u32) -> Option<ImageData> {
    image_thumbnail_file(&std::fs::File::open(path).ok()?, max_px)
}
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

/// 从文件头部提取元数据。读取前 64KB 做判断。
pub fn extract(path: &Path) -> Result<FileMetadata> {
    let mut buf = vec![0u8; 65536];
    let n = {
        let mut f = std::fs::File::open(path)?;
        f.read(&mut buf)?
    };
    let head = &buf[..n];
    let file_type = detect_type(head);
    let image_dimensions = image_dims(head, &file_type);

    Ok(FileMetadata {
        file_type,
        image_dimensions,
        duration_secs: None,
    })
}

/// 根据魔数检测文件类型。
fn detect_type(head: &[u8]) -> String {
    if head.len() < 4 {
        return "unknown".into();
    }
    match head {
        // 图片
        [0xFF, 0xD8, 0xFF, ..] => "jpeg",
        [0x89, 0x50, 0x4E, 0x47, ..] => "png",
        [0x47, 0x49, 0x46, 0x38, ..] => "gif",
        [b'B', b'M', ..] => "bmp",
        [b'W', b'E', b'B', b'P', ..] => "webp",
        // 视频
        [0x1A, 0x45, 0xDF, 0xA3, ..] => "mkv", // EBML/Matroska
        _ if head.len() > 11 && &head[4..8] == b"ftyp" => "mp4",
        _ if head.len() > 3 && &head[3..4] == [0x66] && &head[0..3] == [0, 0, 0] => "mp4", // 0x00 00 00 XX 66 74 79 70
        // 音频
        [0x49, 0x44, 0x33, ..] => "mp3", // ID3
        [0x66, 0x4C, 0x61, 0x43, ..] => "flac",
        [0x52, 0x49, 0x46, 0x46, ..] if head.len() > 12 && &head[8..12] == b"WAVE" => "wav",
        // 压缩包
        [0x50, 0x4B, 0x03, 0x04, ..] => "zip",
        [0x52, 0x61, 0x72, 0x21, ..] => "rar",
        [0x37, 0x7A, 0xBC, 0xAF, ..] => "7z",
        // 文档
        [0x25, 0x50, 0x44, 0x46, ..] => "pdf",
        _ if head.len() >= 2 && head[0] == 0xFE && head[1] == 0xFF => "text-utf16be",
        _ if head.len() >= 2 && head[0] == 0xFF && head[1] == 0xFE => "text-utf16le",
        // Office (ZIP 容器，已在上面 zip 命中)
        _ => {
            // 简单文本检测
            if head
                .iter()
                .take(64)
                .all(|b| *b >= 0x20 || *b == b'\n' || *b == b'\r' || *b == b'\t')
            {
                "text"
            } else {
                "binary"
            }
        }
    }
    .into()
}

/// 从图片头部提取宽×高。
fn image_dims(head: &[u8], file_type: &str) -> Option<(u32, u32)> {
    match file_type {
        "png" => png_dims(head),
        "gif" => gif_dims(head),
        "jpeg" => jpeg_dims(head),
        _ => None,
    }
}

fn png_dims(head: &[u8]) -> Option<(u32, u32)> {
    // PNG: 8-byte sig + IHDR(4 len + "IHDR" + 4 width + 4 height)
    if head.len() < 24 {
        return None;
    }
    let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
    let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
    Some((w, h))
}

fn gif_dims(head: &[u8]) -> Option<(u32, u32)> {
    // GIF: 6-byte header + 2 width + 2 height (little-endian)
    if head.len() < 10 {
        return None;
    }
    let w = u16::from_le_bytes([head[6], head[7]]) as u32;
    let h = u16::from_le_bytes([head[8], head[9]]) as u32;
    Some((w, h))
}

fn jpeg_dims(head: &[u8]) -> Option<(u32, u32)> {
    // JPEG: 扫描 SOF0 (0xFFC0) 标记，其后 2 字节长度 + 1 精度 + 2 高 + 2 宽
    let mut i = 2; // skip SOI (FFD8)
    while i + 8 < head.len() {
        if head[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = head[i + 1];
        // SOF0~SOF15 (C0~CF, 除 C4/D8/D12)
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            let h = u16::from_be_bytes([head[i + 5], head[i + 6]]) as u32;
            let w = u16::from_be_bytes([head[i + 7], head[i + 8]]) as u32;
            return Some((w, h));
        }
        // 跳过此标记段
        if i + 3 < head.len() {
            let seg_len = u16::from_be_bytes([head[i + 2], head[i + 3]]) as usize;
            i += 2 + seg_len;
        } else {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_basic_types() {
        let dir = std::env::temp_dir().join(format!("ds_meta_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // PNG (minimal 1x1)
        let png = dir.join("a.png");
        std::fs::write(
            &png,
            [
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
                0x00, 0x00, 0x00, 0x0D, // IHDR length
                b'I', b'H', b'D', b'R', 0x00, 0x00, 0x00, 0x01, // width=1
                0x00, 0x00, 0x00, 0x02, // height=2
            ],
        )
        .unwrap();
        let m = extract(&png).unwrap();
        assert_eq!(m.file_type, "png");
        assert_eq!(m.image_dimensions, Some((1, 2)));

        // Text
        let txt = dir.join("b.txt");
        std::fs::write(&txt, b"hello world").unwrap();
        let m = extract(&txt).unwrap();
        assert_eq!(m.file_type, "text");

        // PDF
        let pdf = dir.join("c.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 ...").unwrap();
        let m = extract(&pdf).unwrap();
        assert_eq!(m.file_type, "pdf");
    }
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

        let t = image_thumbnail(&path, 128);
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
        assert!(image_thumbnail(&f, 128).is_none());
    }
}
