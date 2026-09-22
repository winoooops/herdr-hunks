#[test]
fn manifest_and_crate_versions_match() {
    let manifest: toml::Table = std::fs::read_to_string("herdr-plugin.toml")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        manifest["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let lock: toml::Table = std::fs::read_to_string("Cargo.lock")
        .unwrap()
        .parse()
        .unwrap();
    let package = lock["package"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"].as_str() == Some(env!("CARGO_PKG_NAME")))
        .expect("crate package in Cargo.lock");
    assert_eq!(package["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(manifest["id"].as_str(), Some("winoooops.hunks"));
    assert_eq!(
        manifest["panes"][0]["title"].as_str(),
        Some(herdr_hunks::actions::VIEWER_TITLE)
    );
}

#[test]
fn update_requires_host_and_plugin_environment() {
    for missing in ["HERDR_BIN_PATH", "HERDR_PLUGIN_ID"] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_herdr-hunks"))
            .arg("update")
            .env("HERDR_BIN_PATH", "/nonexistent-host")
            .env("HERDR_PLUGIN_ID", "test.hunks")
            .env_remove(missing)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains(&format!("{missing} is not set")));
    }
}

#[test]
fn manifest_viewer_is_a_popup_dialog() {
    let manifest: toml::Table = std::fs::read_to_string("herdr-plugin.toml")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(manifest["panes"][0]["placement"].as_str(), Some("popup"));
    assert_eq!(manifest["panes"][0]["width"].as_str(), Some("80%"));
    assert_eq!(manifest["panes"][0]["height"].as_str(), Some("80%"));
    assert_eq!(
        manifest["actions"][0]["title"].as_str(),
        Some("Open the hunk viewer in a dialog")
    );
}

#[cfg(feature = "tui")]
#[test]
fn non_utf8_path_arguments_reach_the_terminal_check_without_panicking() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(OsString::from_vec(b"repo-\xff".to_vec()));
    std::fs::create_dir(&path).unwrap();
    let path = path.into_os_string();
    for args in [vec![path.clone()], vec![OsString::from("tui"), path]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_herdr-hunks"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"herdr-hunks: stdout is not a terminal\n");
    }
}
