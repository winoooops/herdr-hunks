use std::path::Path;
use std::process::Command;

const REPO: &str = "winoooops/herdr-hunks";

fn version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.').map(|part| part.parse::<u64>());
    let version = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    parts.next().is_none().then_some(version)
}

pub fn latest_tag(ls_remote_output: &str) -> Option<String> {
    ls_remote_output
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .filter(|t| !t.ends_with("^{}"))
        .filter_map(|t| Some((version(t.strip_prefix('v')?)?, t.to_string())))
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag)
}

pub fn run_with(host: &Path, git: &str, plugin_id: &str) -> i32 {
    match update(host, git, plugin_id) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("herdr-hunks: {}", crate::text::sanitize(&error.to_string()));
            1
        }
    }
}

fn update(host: &Path, git: &str, plugin_id: &str) -> Result<i32, Box<dyn std::error::Error>> {
    let output = Command::new(host)
        .args(["plugin", "list", "--json"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "plugin list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let plugin = value["result"]["plugins"].as_array().and_then(|plugins| {
        plugins
            .iter()
            .find(|plugin| plugin["plugin_id"].as_str() == Some(plugin_id))
    });
    if plugin.and_then(|plugin| plugin["source"]["kind"].as_str()) != Some("github") {
        return Err(
            "update requires a GitHub install; linked or unknown installs must be updated locally"
                .into(),
        );
    }
    let output = Command::new(git)
        .args(["ls-remote", "--tags", &format!("https://github.com/{REPO}")])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot list release tags: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let tag = latest_tag(&String::from_utf8(output.stdout)?)
        .ok_or("no release tags found (expected vMAJOR.MINOR.PATCH)")?;
    let newest = version(&tag[1..]).ok_or("invalid release version")?;
    let current = version(env!("CARGO_PKG_VERSION")).ok_or("invalid running version")?;
    if newest <= current {
        eprintln!("herdr-hunks: already up to date");
        return Ok(0);
    }
    Ok(Command::new(host)
        .args(["plugin", "install", REPO, "--ref", &tag, "--yes"])
        .status()?
        .code()
        .unwrap_or(1))
}

pub fn run() -> i32 {
    let Some(host) = std::env::var_os("HERDR_BIN_PATH").filter(|host| !host.is_empty()) else {
        eprintln!(
            "herdr-hunks: HERDR_BIN_PATH is not set (run this through a herdr plugin action)"
        );
        return 1;
    };
    let Some(plugin_id) = std::env::var("HERDR_PLUGIN_ID")
        .ok()
        .filter(|id| !id.is_empty())
    else {
        eprintln!(
            "herdr-hunks: HERDR_PLUGIN_ID is not set (run this through a herdr plugin action)"
        );
        return 1;
    };
    run_with(Path::new(&host), "git", &plugin_id)
}

#[cfg(test)]
mod tests {
    use super::latest_tag;

    #[test]
    fn picks_the_highest_semver_tag() {
        let out = "aaa\trefs/tags/v0.2.0\nbbb\trefs/tags/v0.10.0\nccc\trefs/tags/v0.9.3\nddd\trefs/tags/v0.10.0^{}\neee\trefs/tags/nightly\n";
        assert_eq!(latest_tag(out).as_deref(), Some("v0.10.0"));
        assert_eq!(latest_tag("eee\trefs/tags/nightly\n"), None);
    }

    /// Writes an executable shell script and returns its path.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        // Keep writable descriptors out of children spawned by parallel tests.
        assert!(std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "printf '%s\\n' \"$1\" > \"$2\" && chmod +x \"$2\"",
                "fixture",
            ])
            .arg(format!("#!/bin/sh\n{body}"))
            .arg(&path)
            .status()
            .unwrap()
            .success());
        path
    }

    fn host(dir: &std::path::Path, kind: &str, install_exit: i32) -> std::path::PathBuf {
        let log = dir.join("host.log");
        script(dir, "host", &format!(
            "echo \"$*\" >> '{}'\nif [ \"$1 $2\" = 'plugin list' ]; then echo '{{\"result\":{{\"plugins\":[{{\"plugin_id\":\"test.hunks\",\"source\":{{\"kind\":\"{kind}\"}}}}]}}}}'; exit 0; fi\nexit {install_exit}",
            log.display()
        ))
    }

    #[test]
    fn a_linked_install_is_refused_and_nothing_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), "local", 0);
        let git = script(dir.path(), "git", "echo 'aaa\trefs/tags/v9.9.9'");
        assert_ne!(
            super::run_with(&host, git.to_str().unwrap(), "test.hunks"),
            0
        );
        assert!(!std::fs::read_to_string(dir.path().join("host.log"))
            .unwrap()
            .contains("plugin install"));
    }

    #[test]
    fn a_github_install_installs_the_newest_tag_and_propagates_failure() {
        let dir = tempfile::tempdir().unwrap();
        let current = env!("CARGO_PKG_VERSION");
        let (major, minor, patch) = super::version(current).unwrap();
        let newer = format!("v{major}.{minor}.{}", patch + 1);
        let git = script(
            dir.path(),
            "git",
            &format!("printf 'aaa\\trefs/tags/v{current}\\nbbb\\trefs/tags/{newer}\\n'"),
        );
        let ok = host(dir.path(), "github", 0);
        assert_eq!(super::run_with(&ok, git.to_str().unwrap(), "test.hunks"), 0);
        let log = std::fs::read_to_string(dir.path().join("host.log")).unwrap();
        assert!(
            log.contains(&format!(
                "plugin install winoooops/herdr-hunks --ref {newer} --yes"
            )),
            "{log}"
        );
        let failing = host(dir.path(), "github", 7);
        assert_eq!(
            super::run_with(&failing, git.to_str().unwrap(), "test.hunks"),
            7
        );
    }

    #[test]
    fn missing_tags_and_current_or_older_versions_never_install() {
        let current = env!("CARGO_PKG_VERSION");
        let (major, minor, patch) = super::version(current).unwrap();
        let older = match (major, minor, patch) {
            (_, _, 1..) => Some((major, minor, patch - 1)),
            (_, 1.., 0) => Some((major, minor - 1, 0)),
            (1.., 0, 0) => Some((major - 1, 0, 0)),
            _ => None, // no stable version precedes 0.0.0
        };
        let mut cases = vec![("nightly".to_string(), 1), (format!("v{current}"), 0)];
        if let Some((major, minor, patch)) = older {
            cases.push((format!("v{major}.{minor}.{patch}"), 0));
        }
        for (tags, expected) in cases {
            let dir = tempfile::tempdir().unwrap();
            let host = host(dir.path(), "github", 0);
            let git = script(
                dir.path(),
                "git",
                &format!("printf 'aaa\\trefs/tags/{tags}\\n'"),
            );
            assert_eq!(
                super::run_with(&host, git.to_str().unwrap(), "test.hunks"),
                expected
            );
            assert!(!std::fs::read_to_string(dir.path().join("host.log"))
                .unwrap()
                .contains("plugin install"));
        }
    }

    #[test]
    fn an_unknown_plugin_and_command_errors_do_not_install() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path(), "github", 0);
        let unused_git = script(dir.path(), "git", "touch \"$0.called\"; exit 1");
        assert_eq!(
            super::run_with(&host, unused_git.to_str().unwrap(), "another.plugin"),
            1
        );
        assert!(!dir.path().join("git.called").exists());
        assert_eq!(
            super::run_with(&host, unused_git.to_str().unwrap(), "test.hunks"),
            1
        );
        assert!(!std::fs::read_to_string(dir.path().join("host.log"))
            .unwrap()
            .contains("plugin install"));
        let bad_host = script(
            dir.path(),
            "bad-host",
            "echo 'list unavailable' >&2; exit 7",
        );
        assert_eq!(
            super::run_with(&bad_host, unused_git.to_str().unwrap(), "test.hunks"),
            1
        );
        let malformed = script(dir.path(), "bad-host", "echo 'not json'");
        assert_eq!(
            super::run_with(&malformed, unused_git.to_str().unwrap(), "test.hunks"),
            1
        );
    }
}
