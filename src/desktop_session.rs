//! Per-vault desktop metadata, outside the source workspace and lattice database.
use lapis_desktop::session::Session;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
const LIMIT: u64 = 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    vault: String,
    session: Session,
}
fn location(root: &Path, config: &Path) -> Result<(PathBuf, String), String> {
    let vault = root.canonicalize().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
    let hash = crate::notes::content_hash(vault.as_bytes()).replace(':', "-");
    Ok((config.join("workspaces").join(format!("{hash}.json")), vault))
}
pub fn load(root: &Path, config: &Path) -> Result<Option<Session>, String> {
    let (path, vault) = location(root, config)?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut bytes = vec![];
    file.take(LIMIT + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("Workspace session exceeds 1 MiB".into());
    }
    let envelope: Envelope =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if envelope.vault != vault {
        return Err("Saved workspace belongs to another vault; retained unchanged".into());
    }
    envelope.session.validate()?;
    Ok(Some(envelope.session))
}
pub fn save(root: &Path, config: &Path, session: &Session) -> Result<(), String> {
    session.validate()?;
    // Never overwrite unsupported, corrupt, or colliding workspace metadata automatically.
    load(root, config)?;
    let (path, vault) = location(root, config)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&Envelope { vault, session: session.clone() })
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("Workspace session exceeds 1 MiB".into());
    }
    crate::safe_file::replace(&path, &bytes, None).map_err(|e| format!("{}: {e}", path.display()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_atomic_roundtrip_and_corrupt_metadata_is_retained() {
        let temp = std::env::temp_dir().join(format!("lapis-session-{}", std::process::id()));
        let root = temp.join("vault");
        let config = temp.join("config");
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session::default();
        session.tabs.push(lapis_desktop::session::Tab::new("missing.md".into()));
        session.active = Some("missing.md".into());
        save(&root, &config, &session).unwrap();
        assert_eq!(load(&root, &config).unwrap(), Some(session.clone()));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "no notes or index writes");
        let (path, _) = location(&root, &config).unwrap();
        std::fs::write(&path, b"broken json").unwrap();
        assert!(load(&root, &config).is_err());
        assert!(save(&root, &config, &session).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken json");
        std::fs::remove_dir_all(temp).unwrap();
    }
}
