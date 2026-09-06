//! Bounded native page sampling. The helper never opens a PDF viewer or executes actions.
use super::*;

pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub fn backend() -> Option<&'static str> {
    if cfg!(windows) {
        Some("windows_pdf")
    } else if cfg!(target_os = "macos") {
        Some("coregraphics_pdf")
    } else {
        None
    }
}
pub fn sample_pages(count: u32) -> Vec<u32> {
    let mut pages = [0, 1, count / 2]
        .into_iter()
        .filter(|p| *p < count)
        .collect::<Vec<_>>();
    pages.sort_unstable();
    pages.dedup();
    pages
}
pub async fn preview(
    path: &Path,
    file: File,
    size: u64,
    modified: u64,
    cancel: &CancellationToken,
) -> Result<Visual> {
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    crate::safe_fs::verify_evidence(&file, size, modified)?;
    if size > MAX_FILE_BYTES {
        return Ok(Visual::unavailable("pdf_file_size_limit_64mib"));
    }
    if backend().is_none() {
        return Ok(Visual::unavailable("pdf_native_decoder_unavailable"));
    }
    let started = Instant::now();
    let result = native::preview_kind(path, &file, size, modified, cancel, true).await;
    ensure!(!cancel.is_cancelled(), "内容读取已取消");
    crate::safe_fs::verify_evidence(&file, size, modified)?;
    let mut visual =
        result.unwrap_or_else(|_| Visual::unavailable("pdf_encrypted_damaged_or_decode_limit"));
    visual.info["elapsed_ms"] = json!(started.elapsed().as_millis());
    Ok(visual)
}

#[cfg(windows)]
fn sheet(frames: &[Vec<u8>], sampled: &[u32], count: u32) -> Result<Visual> {
    let mut sheet =
        image::RgbImage::from_pixel(512 * frames.len() as u32, 768, image::Rgb([255, 255, 255]));
    for (i, bytes) in frames.iter().enumerate() {
        let mut reader =
            image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        // Windows can apply display scaling to the requested dimensions. Bound that
        // intermediate raster, then enforce the promised physical output dimensions.
        limits.max_image_width = Some(2048);
        limits.max_image_height = Some(3072);
        limits.max_alloc = Some(48 * 1024 * 1024);
        reader.limits(limits);
        let page = reader
            .decode()
            .context("pdf_page_image_decode")?
            .thumbnail(512, 768)
            .to_rgb8();
        image::imageops::replace(
            &mut sheet,
            &page,
            (i as u32 * 512 + (512 - page.width()) / 2) as i64,
            ((768 - page.height()) / 2) as i64,
        );
    }
    let sheet = image::DynamicImage::ImageRgb8(sheet);
    let image = [75, 55, 35]
        .into_iter()
        .filter_map(|q| crate::metadata::jpeg_data(&sheet, q))
        .find(|i| i.data_base64.len() <= 512 * 1024)
        .context("pdf_image_limit")?;
    Ok(Visual {
        image: Some(ImageData {
            high_detail: true,
            ..image
        }),
        info: json!({
            "status":"sampled","kind":"pdf_page_samples","backend":"windows_pdf","page_count":count,
            "sampled_pages":sampled,"layout":"left_to_right","page_max_px":[512,768],
            "partial":sampled.len() < sample_pages(count).len(),"full_document":false,
            "message":"从左到右对应 sampled_pages 中的页码；只预览首页、第二页和中间页，不代表全文。不执行脚本、附件或 OCR。"
        }),
    })
}

#[cfg(windows)]
pub(super) fn decode(path: &Path, _: &File) -> Result<Visual> {
    use windows::{
        core::HSTRING,
        Data::Pdf::{PdfDocument, PdfPageRenderOptions},
        Storage::{
            StorageFile,
            Streams::{DataReader, InMemoryRandomAccessStream},
        },
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    };
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let source =
        StorageFile::GetFileFromPathAsync(&HSTRING::from(native::storage_path(path)?))?.get()?;
    let document = PdfDocument::LoadFromFileAsync(&source)?.get()?;
    ensure!(!document.IsPasswordProtected()?, "pdf_encrypted");
    let count = document.PageCount()?;
    ensure!(count > 0 && count <= 100_000, "pdf_page_count_limit");
    let mut frames = vec![];
    let mut sampled = vec![];
    let mut last_error = None;
    for index in sample_pages(count) {
        let result = (|| -> Result<Vec<u8>> {
            let page = document.GetPage(index)?;
            let size = page.Size()?;
            ensure!(
                size.Width.is_finite()
                    && size.Height.is_finite()
                    && size.Width > 0.0
                    && size.Height > 0.0,
                "pdf_page_size"
            );
            let scale = (512.0 / size.Width).min(768.0 / size.Height);
            let options = PdfPageRenderOptions::new()?;
            options
                .SetBitmapEncoderId(windows::Graphics::Imaging::BitmapEncoder::PngEncoderId()?)?;
            options.SetDestinationWidth((size.Width * scale).round().clamp(1.0, 512.0) as u32)?;
            options.SetDestinationHeight((size.Height * scale).round().clamp(1.0, 768.0) as u32)?;
            let stream = InMemoryRandomAccessStream::new()?;
            page.RenderWithOptionsToStreamAsync(&stream, &options)?
                .get()
                .context("pdf_page_render")?;
            let len = stream.Size()?;
            ensure!(len > 0 && len <= 4 * 1024 * 1024, "pdf_page_output_limit");
            let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
            ensure!(
                reader.LoadAsync(len as u32)?.get()? == len as u32,
                "pdf_page_short"
            );
            let mut bytes = vec![0; len as usize];
            reader.ReadBytes(&mut bytes)?;
            reader.Close()?;
            stream.Close()?;
            page.Close()?;
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => {
                frames.push(bytes);
                sampled.push(index + 1);
                native::publish(&sheet(&frames, &sampled, count)?);
            }
            Err(error) => last_error = Some(error),
        }
    }
    if frames.is_empty() {
        return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("pdf_pages_unavailable")));
    }
    sheet(&frames, &sampled, count)
}

#[cfg(target_os = "macos")]
pub(super) fn decode(_: &Path, file: &File) -> Result<Visual> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn ds_macos_pdf(
            fd: std::ffi::c_int,
            output: *mut u8,
            capacity: usize,
            length: *mut usize,
        ) -> std::ffi::c_int;
    }
    let mut output = vec![0u8; 768 * 1024];
    let mut length = 0usize;
    let status = unsafe {
        ds_macos_pdf(
            file.as_raw_fd(),
            output.as_mut_ptr(),
            output.len(),
            &mut length,
        )
    };
    ensure!(
        status == 0 && length > 0 && length <= output.len(),
        "pdf_decode_unavailable"
    );
    Ok(serde_json::from_slice(&output[..length])?)
}
#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn decode(_: &Path, _: &File) -> Result<Visual> {
    anyhow::bail!("pdf_native_decoder_unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_are_bounded_unique_and_in_reading_order() {
        assert!(sample_pages(0).is_empty());
        assert_eq!(sample_pages(1), vec![0]);
        assert_eq!(sample_pages(2), vec![0, 1]);
        assert_eq!(sample_pages(6), vec![0, 1, 3]);
        assert_eq!(sample_pages(100_000), vec![0, 1, 50_000]);
    }
}
