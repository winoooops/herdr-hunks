use std::path::Path;

use crate::engine::Scope;
pub use crate::paths::{config_dir, state_dir};

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
    pub scope: Scope,
    pub base: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: ModeSetting::Auto,
            files: FilesSetting::Auto,
            mouse: true,
            scope: Scope::Worktree,
            base: None,
        }
    }
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
    match get("view", "scope").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if s == "worktree" => config.scope = Scope::Worktree,
        Some(Some(s)) if s == "branch" => config.scope = Scope::Branch,
        Some(other) => problems.push(format!(
            "view.scope: expected \"worktree\" or \"branch\", got {other:?}"
        )),
    }
    match get("base", "ref").map(|v| v.as_str().map(str::to_owned)) {
        None => {}
        Some(Some(s)) if crate::engine::base::check_text(&s).is_ok() => config.base = Some(s),
        Some(other) => problems.push(format!(
            "base.ref: expected a revision that does not start with '-', got {other:?}"
        )),
    }
    (config, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

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

    #[test]
    fn scope_and_base_keys_are_parsed_and_bad_values_cost_only_their_key() {
        use crate::engine::Scope;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[view]\nscope = \"branch\"\n[base]\nref = \"origin/main\"\n",
        )
        .unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!(
            (config.scope, config.base.as_deref()),
            (Scope::Branch, Some("origin/main"))
        );
        assert!(problems.is_empty());
        std::fs::write(
            dir.path().join("config.toml"),
            "[view]\nscope = \"both\"\n[base]\nref = \"-x\"\n",
        )
        .unwrap();
        let (config, problems) = load(dir.path());
        assert_eq!((config.scope, config.base), (Scope::Worktree, None));
        assert_eq!(problems.len(), 2);
        assert!(
            problems[0].starts_with("view.scope: ") && problems[1].starts_with("base.ref: "),
            "{problems:?}"
        );
        assert_eq!(
            load(tempfile::tempdir().unwrap().path()).0.scope,
            Scope::Worktree
        );
    }
}
