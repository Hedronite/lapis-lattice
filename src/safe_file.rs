//! Same-directory staged writes. Existing files stay intact until a complete,
//! synced replacement is ready; temporary names never collide across writers.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct Staged(PathBuf);

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn stage(path: &Path, bytes: &[u8]) -> io::Result<Staged> {
    let parent = path.parent().ok_or_else(|| io::Error::other("file has no parent directory"))?;
    let (staged, mut file) = loop {
        let name = parent.join(format!(
            ".lapis-save-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&name) {
            Ok(file) => break (Staged(name), file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    if let Ok(metadata) = fs::metadata(path) {
        file.set_permissions(metadata.permissions())?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(staged)
}

/// Recheck the expected disk bytes after staging, immediately before rename.
/// This is optimistic concurrency with external editors, not a filesystem CAS.
pub(crate) fn replace(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> io::Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("cannot atomically replace a symlink; edit its target"));
        }
        if metadata.permissions().readonly() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file is read-only"));
        }
    }
    let staged = stage(path, bytes)?;
    if let Some(expected) = expected
        && fs::read(path)? != expected
    {
        return Err(io::Error::other(
            "file changed on disk; buffer retained; save a copy to keep both versions",
        ));
    }
    fs::rename(&staged.0, path)?;
    // Directory fsync is not supported on every mounted filesystem. The file
    // itself was synced before commit; never misreport a committed rename as
    // an uncommitted save when only the directory sync is unavailable.
    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Publish a complete new file without a check-then-overwrite race.
pub(crate) fn create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staged = stage(path, bytes)?;
    fs::hard_link(&staged.0, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_save_and_failed_publish_preserve_both_original_and_permissions() {
        let root = std::env::temp_dir().join(format!(
            "lapis-safe-file-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("note.md");
        fs::write(&file, "external edit").unwrap();
        assert!(replace(&file, b"my edit", Some(b"old body")).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"external edit");
        assert!(create(&file, b"must not overwrite").is_err());
        assert_eq!(fs::read(&file).unwrap(), b"external edit");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        }
        replace(&file, b"complete body\n", Some(b"external edit")).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"complete body\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        }
        create(&root.join("copy.md"), b"my edit").unwrap();
        assert_eq!(fs::read(root.join("copy.md")).unwrap(), b"my edit");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2, "no abandoned temporary files");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            fs::set_permissions(&file, fs::Permissions::from_mode(0o440)).unwrap();
            assert_eq!(replace(&file, b"no", None).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
            symlink(&file, root.join("link.md")).unwrap();
            assert!(replace(&root.join("link.md"), b"no", None).is_err());
            assert!(fs::symlink_metadata(root.join("link.md")).unwrap().file_type().is_symlink());
            assert_eq!(fs::read(&file).unwrap(), b"complete body\n");
        }
        fs::remove_dir_all(root).unwrap();
    }
}
