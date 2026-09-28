//! PDF bytes, per-page text, and the content hash that keys the store.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use lopdf::Document;
use sha2::{Digest, Sha256};

use crate::error::{Result, io, parse};

pub(crate) struct LoadedPdf {
    pub doc: Document,
    pub pages: Vec<String>,
    pub sha256: String,
}

pub(crate) fn load(path: &Path) -> Result<LoadedPdf> {
    let bytes = std::fs::read(path).map_err(|e| io(format!("{}: {e}", path.display())))?;
    if bytes.is_empty() {
        return Err(parse(format!("{}: empty file", path.display())));
    }
    let sha256 = sha256_bytes(&bytes);
    let doc = Document::load_mem(&bytes).map_err(|e| parse(format!("{}: {e}", path.display())))?;
    let physical = doc.get_pages().len();
    if physical == 0 {
        return Err(parse(format!("{}: pdf has no pages", path.display())));
    }
    let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem_by_pages(&bytes)
    }));
    let mut pages = match extracted {
        Ok(Ok(pages)) => pages,
        Ok(Err(e)) => return Err(parse(format!("text: {e}"))),
        Err(_) => return Err(parse(format!("{}: text extract panicked", path.display()))),
    };
    if pages.len() < physical {
        pages.resize(physical, String::new());
    } else if pages.len() > physical {
        pages.truncate(physical);
    }
    Ok(LoadedPdf { doc, pages, sha256 })
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| io(format!("{}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| io(format!("{}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

pub(crate) fn lead_text(pages: &[String], start: u32, end: u32) -> String {
    if start == 0 || end < start {
        return String::new();
    }
    let mut acc = String::new();
    for page in start..=end {
        let Some(text) = pages.get((page as usize).wrapping_sub(1)) else {
            break;
        };
        if !acc.is_empty() {
            acc.push('\n');
        }
        acc.push_str(text.trim());
        if acc.chars().count() >= crate::LEAD_CHARS {
            break;
        }
    }
    acc.trim().chars().take(crate::LEAD_CHARS).collect()
}

pub(crate) fn char_count(pages: &[String], start: u32, end: u32) -> usize {
    if start == 0 || end < start {
        return 0;
    }
    let mut n = 0usize;
    for page in start..=end {
        if let Some(text) = pages.get((page as usize).wrapping_sub(1)) {
            n = n.saturating_add(text.chars().count());
        }
    }
    n
}

/// UTC timestamp with second precision, no extra crates.
pub(crate) fn now_rfc3339() -> String {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Howard Hinnant's `civil_from_days`, days since 1970-01-01.
pub(crate) fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    }
}
