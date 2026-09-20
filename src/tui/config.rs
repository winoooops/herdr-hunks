use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSetting {
    Auto,
    Unified,
    Split,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesSetting {
    Auto,
    Pinned,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub mode: ModeSetting,
    pub files: FilesSetting,
    pub mouse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: ModeSetting::Auto,
            files: FilesSetting::Auto,
            mouse: true,
        }
    }
}

/// `$HERDR_PLUGIN_CONFIG_DIR`, else `${XDG_CONFIG_HOME:-~/.config}/herdr-hunks`.
pub fn config_dir(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    directory(
        lookup,
        "HERDR_PLUGIN_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        ".config",
    )
}

/// `$HERDR_PLUGIN_STATE_DIR`, else `${XDG_STATE_HOME:-~/.local/state}/herdr-hunks`.
pub fn state_dir(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    directory(
        lookup,
        "HERDR_PLUGIN_STATE_DIR",
        "XDG_STATE_HOME",
        ".local/state",
    )
}

fn directory(
    lookup: impl Fn(&str) -> Option<OsString>,
    plugin: &str,
    xdg: &str,
    home_suffix: &str,
) -> Option<PathBuf> {
    let absolute = |key| {
        lookup(key)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute(plugin).or_else(|| {
        absolute(xdg)
            .or_else(|| absolute("HOME").map(|home| home.join(home_suffix)))
            .map(|base| base.join("herdr-hunks"))
    })
}

pub fn load(dir: &Path) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut problems = Vec::new();
    let text = match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                problems.push(format!("config.toml: {error}"));
            }
            return (config, problems);
        }
    };
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            problems.push(format!("config.toml: {e}"));
            return (config, problems);
        }
    };
    let get = |section: &str, key: &str| table.get(section).and_then(|s| s.get(key)).cloned();
    match get("view", "mode").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if s == "auto" => config.mode = ModeSetting::Auto,
        Some(Some(s)) if s == "unified" => config.mode = ModeSetting::Unified,
        Some(Some(s)) if s == "split" => config.mode = ModeSetting::Split,
        Some(other) => problems.push(format!(
            "view.mode: expected \"auto\", \"unified\" or \"split\", got {other:?}"
        )),
    }
    match get("view", "files").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if s == "auto" => config.files = FilesSetting::Auto,
        Some(Some(s)) if s == "pinned" => config.files = FilesSetting::Pinned,
        Some(Some(s)) if s == "hidden" => config.files = FilesSetting::Hidden,
        Some(other) => problems.push(format!(
            "view.files: expected \"auto\", \"pinned\" or \"hidden\", got {other:?}"
        )),
    }
    match get("input", "mouse").map(|v| v.as_bool()) {
        None => {}
        Some(Some(b)) => config.mouse = b,
        Some(None) => problems.push("input.mouse: expected true or false".to_string()),
    }
    (config, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_empty_and_relative_directories_are_unset() {
        for value in [None, Some(""), Some("relative/path")] {
            let lookup = |_: &str| value.map(OsString::from);
            assert_eq!(config_dir(lookup), None);
            assert_eq!(state_dir(lookup), None);
        }
        assert_eq!(
            state_dir(|key| (key == "XDG_STATE_HOME").then(|| "relative/state".into())),
            None
        );
    }

    #[test]
    fn absolute_directories_follow_plugin_xdg_then_home_precedence() {
        for (plugin, xdg, expected_config, expected_state) in [
            ("/plugin", "/xdg", "/plugin", "/plugin"),
            ("relative", "/xdg", "/xdg/herdr-hunks", "/xdg/herdr-hunks"),
            (
                "",
                "relative",
                "/home/user/.config/herdr-hunks",
                "/home/user/.local/state/herdr-hunks",
            ),
        ] {
            let lookup = |key: &str| {
                Some(OsString::from(match key {
                    "HERDR_PLUGIN_CONFIG_DIR" | "HERDR_PLUGIN_STATE_DIR" => plugin,
                    "XDG_CONFIG_HOME" | "XDG_STATE_HOME" => xdg,
                    "HOME" => "/home/user",
                    _ => unreachable!(),
                }))
            };
            assert_eq!(config_dir(lookup), Some(PathBuf::from(expected_config)));
            assert_eq!(state_dir(lookup), Some(PathBuf::from(expected_state)));
        }
    }

    #[test]
    fn defaults_when_the_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!(
            (config.mode, config.files, config.mouse),
            (ModeSetting::Auto, FilesSetting::Auto, true)
        );
        assert!(problems.is_empty());
    }

    #[test]
    fn one_bad_value_costs_one_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[view]\nmode = \"sideways\"\nfiles = \"pinned\"\n[input]\nmouse = false\n",
        )
        .unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!(
            (config.mode, config.files, config.mouse),
            (ModeSetting::Auto, FilesSetting::Pinned, false)
        );
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("view.mode"));
    }

    #[test]
    fn unparseable_toml_falls_back_entirely() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[view\n").unwrap();
        let (config, problems) = load(dir.path());
        assert!(config.mouse && problems.len() == 1);
    }

    #[test]
    fn wrong_types_fall_back_per_key_and_read_errors_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        std::fs::write(
            &file,
            "[view]\nmode = 12\nfiles = false\n[input]\nmouse = 'yes'\n",
        )
        .unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!(config, Config::default());
        assert_eq!(problems.len(), 3);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!(config, Config::default());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("config.toml:"));
    }
}
