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
/// The hard link fails when the target exists, never mudling a file that got
/// there first; the staged temporary is removed either way, so an abandoned
/// publication leaves no litter and the first writer's bytes stay on disk.
/// New files are `0600`, the same mode the staged write carries.
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
pub fn publish_new(abs: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent)?;
    }
    let staged = stage(abs, bytes)?;
    match fs::hard_link(&staged.0, abs) {
        Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}
