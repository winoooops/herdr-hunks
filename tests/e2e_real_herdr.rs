use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Isolated {
    host: PathBuf,
    home: PathBuf,
    config: PathBuf,
    state: PathBuf,
    socket: PathBuf,
    session: String,
}

impl Isolated {
    fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        // No child inherits live HERDR socket, session, pane, or plugin context.
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH"))
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_STATE_HOME", &self.state)
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn host_command(&self) -> Command {
        let mut command = self.command(&self.host);
        command.args(["--session", &self.session]);
        command
    }

    fn herdr(&self, args: &[&str]) -> Value {
        let out = self
            .host_command()
            .args(args)
            .output()
            .expect("run isolated host");
        assert!(
            out.status.success(),
            "host {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        if out.stdout.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|error| panic!("host {args:?} returned invalid JSON: {error}"))
    }

    fn viewers(&self) -> Vec<Value> {
        self.herdr(&["pane", "list"])["result"]["panes"]
            .as_array()
            .expect("pane list")
            .iter()
            .filter(|pane| pane["label"] == herdr_hunks::actions::VIEWER_TITLE)
            .cloned()
            .collect()
    }

    fn invoke_split(&self, plugin_id: &str, opener: &str) {
        let invoked = self.herdr(&[
            "plugin",
            "action",
            "invoke",
            "open-split",
            "--plugin",
            plugin_id,
        ]);
        assert_eq!(invoked["result"]["context"]["focused_pane_id"], opener);
        let log_id = invoked["result"]["log"]["log_id"]
            .as_str()
            .expect("action log id");
        wait_for("successful split action", || {
            let logs = self.herdr(&[
                "plugin", "log", "list", "--plugin", plugin_id, "--limit", "10",
            ]);
            let log = logs["result"]["logs"]
                .as_array()
                .expect("action logs")
                .iter()
                .find(|log| log["log_id"] == log_id)
                .expect("invoked action log");
            if log["finished_unix_ms"].is_null() {
                return false;
            }
            assert_eq!(log["exit_code"], 0, "split action failed: {log}");
            true
        });
    }
}

#[test]
#[ignore = "requires HERDR_BIN_PATH (0.8.0) and a fresh cargo build --release; see docs/acceptance-p1.md"]
fn open_split_creates_one_viewer_and_reuses_it() {
    let real_session = std::env::var_os("HERDR_E2E_REAL_SESSION")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HERDR_SOCKET_PATH")
                .map(PathBuf::from)
                .map(|socket| socket.with_file_name("session.json"))
        })
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                })
                .map(|config| config.join("herdr/session.json"))
        })
        .expect("real session.json path");
    assert!(
        real_session.is_absolute(),
        "real session path must be absolute"
    );
    let real_mtime = modified(&real_session);
    let result = std::panic::catch_unwind(|| {
        let host = PathBuf::from(std::env::var_os("HERDR_BIN_PATH").expect("set HERDR_BIN_PATH"));
        assert!(host.is_absolute(), "HERDR_BIN_PATH must be absolute");
        let release = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/herdr-hunks");
        assert!(release.is_file(), "run cargo build --release first");
        // Keep Unix socket paths below macOS's 103-byte limit.
        let tmp = tempfile::Builder::new()
            .prefix("hh-e2e-")
            .tempdir_in("/tmp")
            .unwrap();
        let session = format!("hh-e2e-{}", std::process::id());
        let config = tmp.path().join("xdg-config");
        let socket = config
            .join("herdr/sessions")
            .join(&session)
            .join("herdr.sock");
        let iso = Isolated {
            host,
            home: tmp.path().join("home"),
            config,
            state: tmp.path().join("xdg-state"),
            socket,
            session,
        };
        std::fs::create_dir_all(&iso.home).unwrap();
        let server_log = tmp.path().join("server.log");
        let mut server = ProcessGuard(
            iso.host_command()
                .arg("server")
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(&server_log).unwrap())
                .spawn()
                .expect("spawn isolated host server"),
        );
        wait_for("isolated session socket", || {
            assert!(
                server.0.try_wait().unwrap().is_none(),
                "isolated server exited: {}",
                std::fs::read_to_string(&server_log).unwrap_or_default()
            );
            iso.socket.exists()
        });

        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(iso
                .command("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("a.txt"), "ONE\n").unwrap();

        let manifest: toml::Table = include_str!("../herdr-plugin.toml").parse().unwrap();
        let plugin_id = manifest["id"].as_str().unwrap();
        iso.herdr(&["plugin", "link", env!("CARGO_MANIFEST_DIR")]);
        let created = iso.herdr(&[
            "workspace",
            "create",
            "--cwd",
            repo.to_str().unwrap(),
            "--focus",
        ]);
        let opener = created["result"]["root_pane"]["pane_id"]
            .as_str()
            .expect("root pane id");
        iso.invoke_split(plugin_id, opener);
        wait_for("one viewer pane", || iso.viewers().len() == 1);
        let viewer = iso.viewers().remove(0);
        let viewer_id = viewer["pane_id"].as_str().expect("viewer pane id");
        assert_eq!(
            viewer["cwd"].as_str().map(PathBuf::from),
            Some(std::fs::canonicalize(&repo).unwrap())
        );

        let focused = iso.herdr(&["pane", "focus", "--direction", "left", "--pane", viewer_id]);
        assert_eq!(focused["result"]["focus"]["focused_pane_id"], opener);
        iso.invoke_split(plugin_id, opener);
        let viewers = iso.viewers();
        assert_eq!(viewers.len(), 1, "the second invoke must reuse the viewer");
        assert_eq!(viewers[0]["pane_id"], viewer_id);
        assert_eq!(viewers[0]["focused"], true);

        let state_file = iso
            .state
            .join("herdr/plugins")
            .join(plugin_id)
            .join("split-panes.json");
        let records: Value =
            serde_json::from_str(&std::fs::read_to_string(state_file).expect("split-panes.json"))
                .unwrap();
        let records = records.as_object().expect("reuse records");
        assert_eq!(records.len(), 1);
        let key = herdr_hunks::actions::reuse::key(iso.socket.to_str().unwrap(), opener);
        assert_eq!(records[&key]["viewer_pane_id"], viewer_id);
        iso.herdr(&["server", "stop"]);
        wait_for("isolated server shutdown", || {
            server.0.try_wait().unwrap().is_some()
        });
    });
    assert_eq!(
        modified(&real_session),
        real_mtime,
        "the real herdr session was touched"
    );
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
