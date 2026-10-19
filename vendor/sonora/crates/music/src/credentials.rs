use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context as _, Result};

/// The file every provider keeps its sign-in under, inside its own cache folder.
pub(crate) const FILE: &str = "credentials.json";
static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Lumen integration: isolate provider files from a separately installed Sonora.
pub fn configure_root(root: PathBuf) {
    storage::configure_root(root.join("storage"));
    let _ = ROOT.set(root);
}

/// Sonora's cache root, `$XDG_CACHE_HOME/sonora`, falling back to the temp dir.
pub(crate) fn root() -> PathBuf {
    if let Some(root) = ROOT.get() { return root.clone(); }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("sonora")
}

/// A provider's own cache folder, named by its slug.
pub(crate) fn dir(slug: &str) -> PathBuf {
    root().join(slug)
}

/// Writes a credential file readable by the owner alone, creating its folder first.
pub(crate) fn write(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("cannot write {}", path.display()))?;
    secure(path);
    file.write_all(contents).with_context(|| format!("cannot write {}", path.display()))?;
    secure(path);
    Ok(())
}

/// Restricts an existing credential file to its owner. A missing file is left alone.
pub(crate) fn secure(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("credentials: cannot restrict {}: {error}", path.display());
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Deletes a credential file, treating one that is already gone as success.
pub(crate) fn remove(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("credentials: cannot remove {}: {error}", path.display());
    }
}
