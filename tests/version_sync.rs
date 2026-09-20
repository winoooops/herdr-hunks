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
