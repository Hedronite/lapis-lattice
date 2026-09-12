//! The private PDF worker is a separate process: no PDFium objects enter GPUI or Tokio tasks.
use crate::cli::PdfRenderArgs;
use lapis_desktop::services::{ArcCancel, PdfPage, PdfPageInfo};
use std::path::Path;
use std::sync::atomic::Ordering;
use tokio::io::AsyncReadExt;

const MAX_PIXELS: usize = 8_000_000;
const MAX_HEADER: usize = 1024 * 1024;
const MAX_FRAME: usize = 4 + MAX_HEADER + MAX_PIXELS * 4;

pub fn revision(path: &Path) -> Result<String, String> {
    let m = std::fs::metadata(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}:{}:{}:{}", m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
    }
    #[cfg(not(unix))]
    Ok(format!("{}:{:?}", m.len(), m.modified()))
}

pub async fn request(path: &Path, page: u32, width: u32, cancel: ArcCancel) -> Result<PdfPage, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    request_with_program(&exe, path, page, width, cancel).await
}

async fn request_with_program(
    exe: &Path,
    path: &Path,
    page: u32,
    width: u32,
    cancel: ArcCancel,
) -> Result<PdfPage, String> {
    let before = revision(path)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("PDF loading cancelled".into());
    }
    let mut child = tokio::process::Command::new(exe)
        .arg("__pdf-render")
        .arg(path)
        .arg("--page")
        .arg(page.to_string())
        .arg("--width")
        .arg(width.clamp(320, 2400).to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Could not start PDF reader: {e}"))?;
    let stdout = child.stdout.take().ok_or("PDF worker has no output pipe")?;
    let stderr = child.stderr.take().ok_or("PDF worker has no error pipe")?;
    let output = async move {
        let read = async {
            let mut bytes = Vec::new();
            stdout.take(MAX_FRAME as u64 + 1).read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        };
        let errors = async {
            let mut bytes = Vec::new();
            stderr.take(65536).read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        };
        tokio::try_join!(read, errors)
    };
    let cancelled = async {
        while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    };
    let (bytes, errors) = tokio::select! {
        result = output => result.map_err(|e| e.to_string())?,
        _ = cancelled => { let _ = child.kill().await; return Err("PDF loading cancelled".into()); },
        _ = tokio::time::sleep(std::time::Duration::from_secs(20)) => {
            let _ = child.kill().await; return Err("PDF page took too long. Reduce zoom, then Retry.".into());
        }
    };
    let status = match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
        Ok(status) => status.map_err(|e| e.to_string())?,
        Err(_) => {
            let _ = child.kill().await;
            return Err("PDF reader did not exit after rendering".into());
        }
    };
    if !status.success() {
        let message = String::from_utf8_lossy(&errors);
        return Err(if message.trim().is_empty() {
            "The PDF reader exited before rendering a page".into()
        } else {
            message.trim().into()
        });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("PDF loading cancelled".into());
    }
    if revision(path)? != before {
        return Err("The PDF changed while loading. Reload the document.".into());
    }
    decode(bytes, before)
}

fn decode(mut bytes: Vec<u8>, revision: String) -> Result<PdfPage, String> {
    if bytes.len() < 4 || bytes.len() > MAX_FRAME {
        return Err("Invalid PDF worker response size".into());
    }
    let count = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    if count > MAX_HEADER || count > bytes.len() - 4 {
        return Err("Invalid PDF page header".into());
    }
    let info: PdfPageInfo = serde_json::from_slice(&bytes[4..4 + count])
        .map_err(|e| format!("Invalid PDF page metadata: {e}"))?;
    let pixels = (info.width as usize).checked_mul(info.height as usize).ok_or("PDF page is too large")?;
    if pixels == 0
        || pixels > MAX_PIXELS
        || info.pages == 0
        || info.page >= info.pages
        || bytes.len() - 4 - count != pixels * 4
    {
        return Err("Invalid PDF page dimensions or pixel data".into());
    }
    bytes.drain(..4 + count);
    Ok(PdfPage { info, bgra: bytes, revision })
}

#[cfg(not(feature = "desktop"))]
pub fn worker(_: &PdfRenderArgs) -> Result<(), String> {
    Err("PDF page reading requires a desktop build".into())
}

#[cfg(feature = "desktop")]
pub fn worker(args: &PdfRenderArgs) -> Result<(), String> {
    use pdfium_render::prelude::*;
    use std::io::Write;
    let name = Pdfium::pdfium_platform_library_name();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("Cannot locate the PDF runtime")?;
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("LAPIS_PDFIUM_LIBRARY") {
        candidates.push(path.into());
    }
    candidates.push(parent.join(&name));
    candidates.push(parent.join("../Frameworks").join(&name));
    let library = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or("PDF reader is missing from this installation. Reinstall the desktop package, then Retry.")?;
    let bindings = Pdfium::bind_to_library(&library)
        .map_err(|e| format!("Cannot load PDF runtime {}: {e}", library.display()))?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium.load_pdf_from_file(&args.path, None).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            "This PDF requires a password. Open an unlocked copy to read it in Lapis.".into()
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            "This PDF is damaged or has an unsupported format. Replace it with a readable copy, then Reload."
                .into()
        }
        _ => format!("Could not read this PDF. Check that the file is accessible, then Retry. ({e})"),
    })?;
    let pages = u32::try_from(document.pages().len()).map_err(|_| "Invalid PDF page count")?;
    if args.page >= pages {
        return Err(format!("Page {} is outside this {pages}-page PDF", args.page + 1));
    }
    let page = document.pages().get(args.page as i32).map_err(|e| e.to_string())?;
    let width = args.width.clamp(320, 2400);
    let config = PdfRenderConfig::new()
        .set_target_width(width as i32)
        .set_maximum_height((MAX_PIXELS / width as usize) as i32)
        .set_format(PdfBitmapFormat::BGRA)
        .set_reverse_byte_order(false)
        .set_clear_color(PdfColor::WHITE)
        .render_form_data(false);
    let bitmap =
        page.render_with_config(&config).map_err(|e| format!("Cannot render page {}: {e}", args.page + 1))?;
    let width = bitmap.width() as u32;
    let height = bitmap.height() as u32;
    let pixels = width as usize * height as usize;
    if pixels == 0 || pixels > MAX_PIXELS {
        return Err("Rendered page exceeds the pixel budget".into());
    }
    let bgra = bitmap.as_raw_bytes();
    if bgra.len() != pixels * 4 {
        return Err("Unsupported PDF bitmap stride".into());
    }
    let text = page.text().map(|t| t.all()).unwrap_or_default();
    let header = page_header(PdfPageInfo { page: args.page, pages, width, height, text })?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&(header.len() as u32).to_le_bytes())
        .and_then(|_| stdout.write_all(&header))
        .and_then(|_| stdout.write_all(&bgra))
        .map_err(|e| e.to_string())
}

#[cfg(any(feature = "desktop", test))]
fn page_header(mut info: PdfPageInfo) -> Result<Vec<u8>, String> {
    // JSON can expand a control character to six bytes. Bound before encoding,
    // while retaining the image when extraction produces unusually large text.
    const TEXT_LIMIT: usize = (MAX_HEADER - 1024) / 6;
    if info.text.len() > TEXT_LIMIT {
        let mut end = TEXT_LIMIT;
        while !info.text.is_char_boundary(end) {
            end -= 1;
        }
        info.text.truncate(end);
        info.text.push_str("\n[Page text truncated; the complete page remains visible]");
    }
    serde_json::to_vec(&info).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let header =
            serde_json::to_vec(&PdfPageInfo { page: 0, pages: 1, width, height, text: "hello".into() })
                .unwrap();
        let mut b = (header.len() as u32).to_le_bytes().to_vec();
        b.extend(header);
        b.extend(pixels);
        b
    }
    #[test]
    fn framed_pages_validate_pixels_and_never_accept_truncated_responses() {
        let page = decode(frame(2, 1, &[1, 2, 3, 255, 4, 5, 6, 255]), "revision".into()).unwrap();
        assert_eq!(page.bgra, [1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(page.info.text, "hello");
        assert!(decode(frame(2, 1, &[1, 2, 3, 255]), String::new()).is_err());
        assert!(decode(frame(u32::MAX, u32::MAX, &[]), String::new()).is_err());
        assert!(decode(vec![255; 4], String::new()).is_err());
    }
    #[test]
    fn oversized_extracted_text_keeps_a_valid_page_header() {
        for text in ["\u{0000}".repeat(MAX_HEADER), "藍".repeat(MAX_HEADER / 2)] {
            let header =
                page_header(PdfPageInfo { page: 1, pages: 3, width: 1200, height: 1600, text }).unwrap();
            assert!(header.len() < MAX_HEADER);
            let info: PdfPageInfo = serde_json::from_slice(&header).unwrap();
            assert_eq!(info.page, 1);
            assert!(info.text.ends_with("the complete page remains visible]"));
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_and_reaps_the_running_pdf_worker() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("lapis-pdf-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("fixture.pdf");
        let script = root.join("worker.py");
        std::fs::write(&file, b"%PDF fixture").unwrap();
        std::fs::write(&script,"#!/usr/bin/env python3\nimport os,sys,time\nfrom pathlib import Path\nPath(sys.argv[2]+'.pid').write_text(str(os.getpid()))\ntime.sleep(30)\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancel.clone();
        let path = file.clone();
        let task = tokio::spawn(async move { request_with_program(&script, &path, 0, 1200, flag).await });
        let pid_file = file.with_extension("pdf.pid");
        for _ in 0..200 {
            if pid_file.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(pid_file.exists(), "worker must actually start before cancellation");
        let pid = std::fs::read_to_string(pid_file).unwrap();
        cancel.store(true, Ordering::Relaxed);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), task).await.unwrap().unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
        let alive = std::process::Command::new("kill").args(["-0", pid.trim()]).output().unwrap();
        assert!(!alive.status.success(), "cancelled worker must have exited and been reaped");
        std::fs::remove_dir_all(root).unwrap();
    }
}
