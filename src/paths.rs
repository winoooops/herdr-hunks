use std::ffi::OsString;
use std::path::PathBuf;

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
