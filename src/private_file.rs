//! Cache files only this user can read. A cache entry can hold note paths or a
//! service address, so its folder is created 0700 and the file 0600, through a
//! temporary file that is created exclusively (never followed through a planted
//! link) and then renamed into place, so a reader never sees half a file.
use std::io::Write;
use std::path::Path;

pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    let mut folder = std::fs::DirBuilder::new();
    folder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut folder, 0o700);
    folder.create(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&tmp).and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()));
    let placed = written.and_then(|()| std::fs::rename(&tmp, path));
    if placed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    placed
}

/// The file's bytes, or `None` when it is missing, unreadable or over `cap`
/// bytes: a cache is never worth reading an unbounded file for.
pub fn read_capped(path: &Path, cap: u64) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    std::fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_private_write_lands_whole_and_owner_only() {
        let base = std::env::temp_dir().join(format!("lapis-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let file = base.join("deep/cache.json");
        write(&file, b"{}").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"{}");
        write(&file, b"[1]").unwrap();
        assert_eq!(read_capped(&file, 16).unwrap(), b"[1]", "a second write replaces the first");
        assert_eq!(read_capped(&file, 2), None, "over the cap is not read");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(base.join("deep")).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let leftovers = std::fs::read_dir(base.join("deep")).unwrap().count();
        assert_eq!(leftovers, 1, "no temporary file is left behind");
        let _ = std::fs::remove_dir_all(base);
    }
}
