//! Same-directory staged writes, the create path, moved from the CLI's
//! `safe_file` / `write_atomic`. Existing files stay intact until a complete,
//! synced replacement is ready; temporary names never collide across writers.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
/// Staged temp file whose backing slot is removed on any early drop.
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
/// Replace `path` with `bytes`. Never follows a symlink at `path` (an existing
/// link must be edited through its target); a read-only file is refused. With
/// `expected`, the previous bytes are rechecked just before the rename.
///
/// The write is the whole staged file plus one rename: a crash leaves either
/// the old file or the new one, never a partial body.
pub fn replace(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> io::Result<()> {
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
/// Publish the bytes of a new note: create the missing folder, then stage and
/// hard-link, so a target that appears between the containment check and this
/// publish is refused instead of overwritten.
///
/// The hard link fails when the target exists, never muddling a file that got
/// there first; the staged temporary is removed either way, so an abandoned
/// publication leaves no litter and the first writer's bytes stay on disk.
/// New files are `0600` on unix, the same mode the staged write carries.
///
/// Some filesystems refuse links: Android shared storage (FUSE) answers
/// `EPERM`, and some iOS file-provider folders do the same. There the bytes
/// are written straight to the target with `create_new` (`O_EXCL` on unix, so
/// it still never replaces; mode `0600`), and a failed write after the file
/// was created removes the partial file. The trade: that fallback is not
/// atomic for readers - a reader can catch the target at an empty size for the
/// length of the write - while a reader of the linked publish never sees
/// anything but the whole file. `AlreadyExists` is always a refusal and never
/// falls back.
pub fn publish_new(abs: &Path, bytes: &[u8], vault: &Path) -> io::Result<()> {
    publish_with(|s, t| fs::hard_link(s, t), abs, bytes, vault)
}

/// The link step of [`publish_new`], factored out so tests can force it to
/// fail the way a link-free filesystem would.
fn link_unsupported(e: &io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(e.kind(), PermissionDenied | Unsupported | CrossesDevices)
        || matches!(
            e.raw_os_error(),
            // EPERM, EXDEV, ENOTSUP/EOPNOTSUPP (Linux 95, macOS/BSD 45).
            Some(1) | Some(18) | Some(45) | Some(95)
        )
}

/// [`publish_new`] with the link step supplied: folders are created, the
/// canonical parent is re-checked against the canonical vault (a folder
/// swapped for a symlink out of the vault after the check is refused), the
/// bytes are staged, and `link` is asked to publish the staged file at the
/// target. An existing target is refused (`AlreadyExists`), an
/// unsupported-link error falls back to a direct `create_new` write of the
/// target, and a failed write after that fallback removes the partial file.
pub fn publish_with(
    link: impl Fn(&Path, &Path) -> io::Result<()>,
    abs: &Path,
    bytes: &[u8],
    vault: &Path,
) -> io::Result<()> {
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
        // After mkdir: the parent must still resolve inside the vault, or the
        // staged rename would publish through a swapped-out folder.
        if !parent.canonicalize()?.starts_with(
            vault.canonicalize().map_err(|e| io::Error::other(format!("vault {}: {e}", vault.display())))?,
        ) {
            return Err(io::Error::other(format!(
                "{}: a folder on the way escapes the vault",
                abs.display()
            )));
        }
    }
    let staged = stage(abs, bytes)?;
    let result = match link(&staged.0, abs) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) if link_unsupported(&e) => write_direct(abs, bytes),
        Err(e) => Err(e),
    };
    // The staged temporary is a second hard link of a successful publish; its
    // Drop removal clears the temporary in every path above.
    result
}

/// Write bytes straight to `abs` with `create_new` so an existing target is
/// never replaced. A write error after the file was created removes the
/// partial file, since a half-written note must not be left as the target.
fn write_direct(abs: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(abs)?;
    match file.write_all(bytes).and_then(|_| file.sync_all()) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(abs);
            Err(e)
        }
    }
}

/// Write the final bytes of a note over an existing path, atomically: parent
/// directories are real directories, the write is staged, and a rename swaps
/// the whole text in. Rename replaces the target, so only callers that
/// legitimately replace a file use this; the create path takes
/// [`publish_new`]'s link, and never overwrites.
pub fn write_atomic(abs: &Path, text: &str) -> io::Result<u64> {
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    replace(abs, text.as_bytes(), None)?;
    Ok(text.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-fsutil-{name}-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A link that fails like a FUSE mount falls back to a direct `create_new`
    /// write of the target.
    #[test]
    fn an_unsupported_link_falls_back_to_a_direct_write() {
        let v = dir("fallback");
        publish_with(|_, _| Err(io::Error::from_raw_os_error(1)), &v.join("note.md"), b"body\n", &v).unwrap();
        assert_eq!(std::fs::read(v.join("note.md")).unwrap(), b"body\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = v.join("note.md").metadata().unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the direct write keeps the staged mode");
        }
        let _ = std::fs::remove_dir_all(v);
    }

    /// The fallback never replaces: an existing target stays an existing
    /// target, and its bytes are exactly as they were.
    #[test]
    fn the_fallback_refuses_an_existing_target() {
        let v = dir("fallback-existing");
        std::fs::write(v.join("note.md"), b"first bytes").unwrap();
        match publish_with(
            |_, _| Err(io::Error::from_raw_os_error(1)),
            &v.join("note.md"),
            b"later bytes",
            &v,
        ) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{e}"),
            Ok(()) => panic!("the fallback replaced the first bytes"),
        }
        assert_eq!(std::fs::read(v.join("note.md")).unwrap(), b"first bytes");
        let _ = std::fs::remove_dir_all(v);
    }

    /// A failed publication leaves no staged temporary in the folder.
    #[test]
    fn a_failed_publication_leaves_no_temporary() {
        let v = dir("fallback-partial");
        std::fs::write(v.join("note.md"), b"first bytes").unwrap();
        publish_with(|_, _| Err(io::Error::from_raw_os_error(1)), &v.join("note.md"), b"later", &v)
            .unwrap_err();
        assert_eq!(std::fs::read(v.join("note.md")).unwrap(), b"first bytes");
        let temps: Vec<std::ffi::OsString> = std::fs::read_dir(&v)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".lapis-save-"))
            .map(|e| e.file_name())
            .collect();
        assert!(temps.is_empty(), "staged temporaries were left behind: {temps:?}");
        let _ = std::fs::remove_dir_all(v);
    }
}
