//! Port-surface shim for the frozen git tree. Policy differs from vimeflow: D1.
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Expand `~` to the user's home directory.
///
/// Returns the input unchanged if it does not start with `~`, or if the
/// home directory cannot be determined (in which case downstream checks
/// will reject the path anyway).
pub(crate) fn expand_home(path: &str) -> PathBuf {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            if path == "~" {
                return home;
            }
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// Resolve the canonical home directory.
///
/// Failure here means we cannot enforce the sandbox at all, so every
/// caller treats it as a fatal `access denied`.
pub(crate) fn home_canonical() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "cannot determine home directory".to_string())?;
    fs::canonicalize(&home).map_err(|e| format!("cannot resolve home dir: {}", e))
}

/// Reject any `..` component in a path.
///
/// Called before any filesystem mutation so a forged path like
/// `~/../etc/evil.txt` cannot trigger `create_dir_all` or any other
/// side effect outside of home. Legitimate UI paths are built from
/// the current working directory plus a basename, so `..` is always
/// suspicious and blocking it sidesteps the subtle interaction between
/// lexical `Path::parent()` walks and OS-level `..` resolution.
pub(crate) fn reject_parent_refs(path: &Path) -> Result<(), String> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!(
            "access denied: path contains parent traversal segments: {}",
            path.display()
        ));
    }
    Ok(())
}

/// Accepts any canonical absolute path. vimeflow restricts to $HOME because its
/// cwd arrives over IPC; here it comes from the user's own pane or command line.
pub(crate) fn ensure_within_home(canonical: &Path, _home_canonical: &Path) -> Result<(), String> {
    if canonical.is_absolute() {
        Ok(())
    } else {
        Err(format!("path is not absolute: {}", canonical.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_canonical_path_outside_home() {
        let home = home_canonical().expect("home");
        assert!(ensure_within_home(Path::new("/tmp"), &home).is_ok());
    }

    #[test]
    fn still_rejects_parent_refs() {
        assert!(reject_parent_refs(Path::new("/tmp/../etc")).is_err());
    }

    #[test]
    fn still_expands_tilde() {
        let home = std::env::var("HOME").expect("HOME");
        assert_eq!(expand_home("~/x"), PathBuf::from(home).join("x"));
    }
}
