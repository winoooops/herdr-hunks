//! `git --version` parsing and the supported floor.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
}

/// The frozen watcher needs `rev-parse --path-format=absolute` (git 2.31).
pub const MIN_GIT: GitVersion = GitVersion {
    major: 2,
    minor: 31,
};

pub fn parse(output: &str) -> Option<GitVersion> {
    let rest = output.trim().strip_prefix("git version ")?;
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some(GitVersion { major, minor })
}

/// `Missing` is retryable (the user can fix PATH and press `r`); `TooOld` is fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCheckError {
    Missing(String),
    TooOld(String),
}

pub fn check() -> Result<GitVersion, GitCheckError> {
    let output = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map_err(|e| GitCheckError::Missing(format!("Failed to spawn git: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = parse(&text).ok_or_else(|| {
        GitCheckError::Missing(format!("unrecognised git version: {}", text.trim()))
    })?;
    if version < MIN_GIT {
        return Err(GitCheckError::TooOld(format!(
            "git {}.{} is too old: herdr-hunks needs git {}.{} or newer",
            version.major, version.minor, MIN_GIT.major, MIN_GIT.minor
        )));
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_version_strings() {
        assert_eq!(
            parse("git version 2.43.0\n"),
            Some(GitVersion {
                major: 2,
                minor: 43
            })
        );
        assert_eq!(
            parse("git version 2.39.5 (Apple Git-154)"),
            Some(GitVersion {
                major: 2,
                minor: 39
            })
        );
        assert_eq!(
            parse("git version 2.50.1.windows.1"),
            Some(GitVersion {
                major: 2,
                minor: 50
            })
        );
        assert_eq!(parse("not git"), None);
    }

    #[test]
    fn floor_is_2_31() {
        assert!(
            GitVersion {
                major: 2,
                minor: 31
            } >= MIN_GIT
        );
        assert!(
            GitVersion {
                major: 2,
                minor: 30
            } < MIN_GIT
        );
        assert!(GitVersion { major: 3, minor: 0 } >= MIN_GIT);
    }
}
