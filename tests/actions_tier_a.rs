mod support;

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use herdr_hunks::actions::{reuse, run_open, Placement};
use serde_json::{json, Value};
use support::FakeHerdr;

static LOCK: Mutex<()> = Mutex::new(());

fn setup() -> (tempfile::TempDir, FakeHerdr) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    let fake = FakeHerdr::start(dir.path());
    fake.set_panes(json!([opener(dir.path())]));
    std::env::set_var("HERDR_SOCKET_PATH", &fake.socket_path);
    std::env::set_var("HERDR_PLUGIN_ID", "test.hunks");
    std::env::set_var("HERDR_PLUGIN_STATE_DIR", dir.path().join("state"));
    std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", json!({
        "focused_pane_id": "w1:p1", "focused_pane_cwd": "/context", "workspace_cwd": "/workspace",
    }).to_string());
    (dir, fake)
}

fn opener(dir: &Path) -> Value {
    json!({ "pane_id": "w1:p1", "label": "shell", "cwd": dir.join("repo"), "foreground_cwd": dir.join("repo/sub") })
}

fn with_viewer(dir: &Path, fake: &FakeHerdr, label: &str, cwd: &Path) {
    fake.set_panes(json!([opener(dir), { "pane_id": "w1:p9", "label": label, "cwd": cwd }]));
}

fn finish(fake: FakeHerdr) {
    let schema: Value =
        serde_json::from_str(include_str!("fixtures/herdr-0.8.0-schema.json")).unwrap();
    for method in ["pane.get", "plugin.pane.open", "plugin.pane.focus"] {
        let variant = schema["schemas"]["request"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["properties"]["method"]["const"] == method)
            .unwrap();
        let reference = variant["properties"]["params"]["$ref"].as_str().unwrap();
        let definition = schema
            .pointer(reference.strip_prefix('#').unwrap())
            .unwrap();
        for request in fake.calls_named(method) {
            let params = request["params"].as_object().expect("object params");
            for key in params.keys() {
                assert!(
                    definition["properties"].get(key).is_some(),
                    "{method}: unknown param {key}"
                );
            }
            if let Some(required) = definition["required"].as_array() {
                for key in required {
                    assert!(
                        params.contains_key(key.as_str().unwrap()),
                        "{method}: missing {key}"
                    );
                }
            }
        }
    }
    fake.stop();
}

#[test]
fn overlay_uses_the_foreground_cwd_and_exact_params() {
    let _lock = LOCK.lock().unwrap();
    let (dir, fake) = setup();
    assert_eq!(run_open(Placement::Overlay), 0);
    support::wait_for(
        || fake.calls_named("plugin.pane.open").len() == 1,
        Duration::from_secs(1),
    );
    assert_eq!(
        fake.calls_named("plugin.pane.open")[0]["params"],
        json!({
            "plugin_id": "test.hunks", "entrypoint": "viewer", "placement": "overlay",
            "cwd": dir.path().join("repo/sub"), "env": {"HERDR_HUNKS_OPENER_PANE": "w1:p1"}, "focus": true,
        })
    );
    assert!(fake.calls_named("plugin.pane.focus").is_empty());
    assert!(!dir.path().join("state").exists());
    finish(fake);
}

#[test]
fn split_reuses_the_viewer_and_all_requests_match_the_schema() {
    let _lock = LOCK.lock().unwrap();
    let (dir, fake) = setup();
    assert_eq!(run_open(Placement::Split), 0);
    let records = reuse::load(&dir.path().join("state"));
    let record = &records[&reuse::key(fake.socket_path.to_str().unwrap(), "w1:p1")];
    assert_eq!(record.viewer_pane_id, "w1:p9");
    assert_eq!(
        record.repo_cwd,
        dir.path().join("repo/sub").to_str().unwrap()
    );
    assert_eq!(
        record.toplevel,
        dir.path()
            .join("repo")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(
        fake.calls_named("plugin.pane.open")[0]["params"],
        json!({
            "plugin_id": "test.hunks", "entrypoint": "viewer", "placement": "split",
            "target_pane_id": "w1:p1", "direction": "right", "cwd": dir.path().join("repo/sub"),
            "env": {"HERDR_HUNKS_OPENER_PANE": "w1:p1"}, "focus": true,
        })
    );
    with_viewer(dir.path(), &fake, "Hunks", &dir.path().join("repo/sub"));
    assert_eq!(run_open(Placement::Split), 0);
    assert_eq!(fake.calls_named("plugin.pane.open").len(), 1);
    assert_eq!(fake.calls_named("plugin.pane.focus").len(), 1);
    assert_eq!(
        fake.calls_named("plugin.pane.focus")[0]["params"],
        json!({"pane_id": "w1:p9"})
    );
    finish(fake);
}

#[test]
fn another_socket_does_not_reuse_the_record() {
    let _lock = LOCK.lock().unwrap();
    let (dir, first) = setup();
    assert_eq!(run_open(Placement::Split), 0);
    let second_dir = tempfile::tempdir().unwrap();
    let second = FakeHerdr::start(second_dir.path());
    with_viewer(dir.path(), &second, "Hunks", &dir.path().join("repo/sub"));
    std::env::set_var("HERDR_SOCKET_PATH", &second.socket_path);
    assert_eq!(run_open(Placement::Split), 0);
    assert_eq!(second.calls_named("plugin.pane.open").len(), 1);
    assert!(second.calls_named("plugin.pane.focus").is_empty());
    assert_eq!(reuse::load(&dir.path().join("state")).len(), 2);
    finish(first);
    finish(second);
}

#[test]
fn relabelled_moved_or_missing_viewers_are_replaced() {
    let _lock = LOCK.lock().unwrap();
    for (label, cwd, exists) in [
        ("zsh", "repo/sub", true),
        ("Hunks", "elsewhere", true),
        ("Hunks", "repo/sub", false),
    ] {
        let (dir, fake) = setup();
        assert_eq!(run_open(Placement::Split), 0);
        if exists {
            with_viewer(dir.path(), &fake, label, &dir.path().join(cwd));
        }
        assert_eq!(run_open(Placement::Split), 0);
        assert_eq!(fake.calls_named("plugin.pane.open").len(), 2);
        assert!(fake.calls_named("plugin.pane.focus").is_empty());
        finish(fake);
    }
}

#[test]
fn another_repository_does_not_reuse_the_viewer() {
    let _lock = LOCK.lock().unwrap();
    let (dir, fake) = setup();
    assert_eq!(run_open(Placement::Split), 0);
    let other = dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&other)
        .status()
        .unwrap()
        .success());
    let mut pane = opener(dir.path());
    pane["foreground_cwd"] = json!(other);
    fake.set_panes(
        json!([pane, {"pane_id": "w1:p9", "label": "Hunks", "cwd": dir.path().join("repo/sub")} ]),
    );
    assert_eq!(run_open(Placement::Split), 0);
    assert_eq!(fake.calls_named("plugin.pane.open").len(), 2);
    assert!(fake.calls_named("plugin.pane.focus").is_empty());
    finish(fake);
}

#[test]
fn failed_focus_opens_another_viewer() {
    let _lock = LOCK.lock().unwrap();
    let (dir, fake) = setup();
    assert_eq!(run_open(Placement::Split), 0);
    with_viewer(dir.path(), &fake, "Hunks", &dir.path().join("repo/sub"));
    fake.fail_focus(true);
    assert_eq!(run_open(Placement::Split), 0);
    assert_eq!(fake.calls_named("plugin.pane.focus").len(), 1);
    assert_eq!(fake.calls_named("plugin.pane.open").len(), 2);
    finish(fake);
}

#[test]
fn plugin_id_is_taken_from_the_environment_and_required_before_any_request() {
    let _lock = LOCK.lock().unwrap();
    let (_dir, fake) = setup();
    std::env::set_var("HERDR_PLUGIN_ID", "someone.else");
    assert_eq!(run_open(Placement::Overlay), 0);
    assert_eq!(
        fake.calls_named("plugin.pane.open")[0]["params"]["plugin_id"],
        "someone.else"
    );
    let pane_calls = fake.calls_named("pane.get").len();
    std::env::remove_var("HERDR_PLUGIN_ID");
    assert_eq!(run_open(Placement::Overlay), 1);
    assert_eq!(fake.calls_named("pane.get").len(), pane_calls);
    assert_eq!(fake.calls_named("plugin.pane.open").len(), 1);
    assert!(fake.calls_named("plugin.pane.focus").is_empty());
    finish(fake);
}

#[test]
fn the_focused_viewer_does_not_open_or_focus_itself() {
    let _lock = LOCK.lock().unwrap();
    let (_dir, fake) = setup();
    fake.set_panes(json!([{"pane_id": "w1:p1", "label": "Hunks"}]));
    for placement in [Placement::Overlay, Placement::Split] {
        assert_eq!(run_open(placement), 0);
    }
    assert!(fake.calls_named("plugin.pane.open").is_empty());
    assert!(fake.calls_named("plugin.pane.focus").is_empty());
    finish(fake);
}

#[test]
fn cwd_fallbacks_and_missing_cwd_are_handled() {
    let _lock = LOCK.lock().unwrap();
    let (_dir, fake) = setup();
    for (pane, context, expected) in [
        (
            json!({"pane_id": "w1:p1", "cwd": "/pane"}),
            json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": "/focused"}),
            "/pane",
        ),
        (
            json!({"pane_id": "w1:p1"}),
            json!({"focused_pane_id": "w1:p1", "focused_pane_cwd": "/focused", "workspace_cwd": "/workspace"}),
            "/focused",
        ),
        (
            json!({"pane_id": "w1:p1"}),
            json!({"focused_pane_id": "w1:p1", "workspace_cwd": "/workspace"}),
            "/workspace",
        ),
    ] {
        fake.set_panes(json!([pane]));
        std::env::set_var("HERDR_PLUGIN_CONTEXT_JSON", context.to_string());
        assert_eq!(run_open(Placement::Overlay), 0);
        assert_eq!(
            fake.calls_named("plugin.pane.open").last().unwrap()["params"]["cwd"],
            expected
        );
    }
    std::env::set_var(
        "HERDR_PLUGIN_CONTEXT_JSON",
        "{\"focused_pane_id\":\"w1:p1\"}",
    );
    assert_eq!(run_open(Placement::Overlay), 1);
    assert_eq!(fake.calls_named("plugin.pane.open").len(), 3);
    fake.set_panes(json!([]));
    assert_eq!(run_open(Placement::Overlay), 1);
    assert_eq!(fake.calls_named("plugin.pane.open").len(), 3);
    finish(fake);
}

#[test]
fn no_usable_state_directory_opens_without_reading_or_writing_records() {
    let _lock = LOCK.lock().unwrap();
    let (dir, fake) = setup();
    with_viewer(dir.path(), &fake, "Hunks", &dir.path().join("repo/sub"));
    let relative = dir.path().join("relative-state");
    let records = [(
        reuse::key(fake.socket_path.to_str().unwrap(), "w1:p1"),
        reuse::Record {
            viewer_pane_id: "w1:p9".into(),
            repo_cwd: dir.path().join("repo/sub").to_str().unwrap().into(),
            toplevel: dir
                .path()
                .join("repo")
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
        },
    )]
    .into();
    reuse::save(&relative, &records).unwrap();
    let before = std::fs::read(relative.join("split-panes.json")).unwrap();
    for action in ["open-split", "open-split", "open"] {
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-hunks"))
            .arg(action)
            .current_dir(dir.path())
            .env("HERDR_PLUGIN_STATE_DIR", "relative-state")
            .env("XDG_STATE_HOME", "relative-xdg")
            .env("HOME", "relative-home")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if action == "open-split" {
            assert!(String::from_utf8_lossy(&output.stderr).contains("opening without reuse"));
        } else {
            assert!(output.stderr.is_empty());
        }
    }
    assert_eq!(fake.calls_named("plugin.pane.open").len(), 3);
    assert!(fake.calls_named("plugin.pane.focus").is_empty());
    assert!(fake
        .calls_named("pane.get")
        .iter()
        .all(|call| call["params"]["pane_id"] == "w1:p1"));
    assert_eq!(
        std::fs::read(relative.join("split-panes.json")).unwrap(),
        before
    );
    assert!(!relative.join("split-panes.lock").exists());
    assert!(!dir.path().join("relative-xdg").exists());
    assert!(!dir.path().join("relative-home").exists());
    assert!(!dir.path().join("state").exists());
    finish(fake);
}

#[test]
fn host_binary_is_not_named_literally_in_rust_sources() {
    fn walk(dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                assert!(
                    !std::fs::read_to_string(&path)
                        .unwrap()
                        .contains("\"herdr\""),
                    "{} names the host binary",
                    path.display()
                );
            }
        }
    }
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}
