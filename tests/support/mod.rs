use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn pane_info(pane: serde_json::Value) -> serde_json::Value {
    let mut complete = serde_json::json!({
        "terminal_id": format!("terminal-{}", pane["pane_id"].as_str().unwrap()),
        "workspace_id": "w1", "tab_id": "w1:t1", "focused": false,
        "agent_status": "unknown", "revision": 1,
    });
    complete
        .as_object_mut()
        .unwrap()
        .extend(pane.as_object().unwrap().clone());
    complete
}

pub struct FakeHerdr {
    pub socket_path: PathBuf,
    recorded: Arc<Mutex<Vec<serde_json::Value>>>,
    responses: Arc<Mutex<Vec<serde_json::Value>>>,
    panes: Arc<Mutex<serde_json::Value>>,
    fail_focus: Arc<AtomicBool>,
    popup_error: Arc<Mutex<Option<String>>>,
    listener_thread: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl FakeHerdr {
    pub fn start(dir: &Path) -> Self {
        let socket_path = dir.join("fake-herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake herdr socket");
        listener.set_nonblocking(true).unwrap();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let responses = Arc::new(Mutex::new(Vec::new()));
        let sent = responses.clone();
        let panes = Arc::new(Mutex::new(serde_json::json!({ "panes": [] })));
        let fail_focus = Arc::new(AtomicBool::new(false));
        let popup_error = Arc::new(Mutex::new(None::<String>));
        let popup_failure = popup_error.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let (requests, pane_response, focus_failure, stop) = (
            recorded.clone(),
            panes.clone(),
            fail_focus.clone(),
            shutdown.clone(),
        );
        let listener_thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // On BSD and macOS an accepted socket inherits the
                        // listener's `O_NONBLOCK`, so a client that has
                        // connected but not yet written makes `read_line`
                        // answer `WouldBlock` -- which the arm below reads as
                        // a dead client and drops the request. The timeout is
                        // what keeps a genuinely stuck one from holding the
                        // loop instead.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let mut line = String::new();
                        if BufReader::new(stream.try_clone().unwrap())
                            .read_line(&mut line)
                            .is_err()
                            || line.is_empty()
                        {
                            continue;
                        }
                        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                        requests.lock().unwrap().push(request.clone());
                        let id = request["id"].clone();
                        let response = if !request["params"].is_object() {
                            serde_json::json!({
                                "id": id,
                                "error": {
                                    "code": "invalid_params",
                                    "message": "params object required",
                                },
                            })
                        } else {
                            let result = match request["method"].as_str().unwrap_or_default() {
                                "pane.get" => pane_response.lock().unwrap()["panes"]
                                    .as_array().unwrap().iter()
                                    .find(|pane| pane["pane_id"] == request["params"]["pane_id"])
                                    .map(|pane| serde_json::json!({ "type": "pane_info", "pane": pane }))
                                    .ok_or_else(|| serde_json::json!({ "code": "pane_not_found", "message": "no such pane" })),
                                "plugin.pane.open" if request["params"]["placement"] == "popup" => {
                                    match popup_failure.lock().unwrap().as_ref() {
                                        Some(message) => Err(serde_json::json!({"code": "invalid_params", "message": message})),
                                        None => Ok(serde_json::json!({"type": "ok"})),
                                    }
                                }
                                "plugin.pane.open" => Ok(serde_json::json!({
                                    "type": "plugin_pane_opened",
                                    "plugin_pane": {
                                        "plugin_id": request["params"]["plugin_id"],
                                        "entrypoint": request["params"]["entrypoint"],
                                        "pane": pane_info(serde_json::json!({
                                            "pane_id": "w1:p9", "label": "Hunks", "cwd": request["params"]["cwd"],
                                            "focused": request["params"]["focus"].as_bool().unwrap_or(false),
                                        })),
                                    },
                                })),
                                "plugin.pane.focus" if focus_failure.load(Ordering::Relaxed) =>
                                    Err(serde_json::json!({ "code": "pane_not_found", "message": "focus failed" })),
                                "plugin.pane.focus" => pane_response.lock().unwrap()["panes"]
                                    .as_array().unwrap().iter()
                                    .find(|pane| pane["pane_id"] == request["params"]["pane_id"])
                                    .map(|pane| serde_json::json!({
                                        "type": "plugin_pane_focused",
                                        "plugin_pane": { "plugin_id": "test.hunks", "entrypoint": "viewer", "pane": pane },
                                    }))
                                    .ok_or_else(|| serde_json::json!({ "code": "pane_not_found", "message": "no such pane" })),
                                _ => Err(serde_json::json!({ "code": "unknown_method", "message": "unknown method" })),
                            };
                            match result {
                                Ok(result) => serde_json::json!({ "id": id, "result": result }),
                                Err(error) => serde_json::json!({ "id": id, "error": error }),
                            }
                        };
                        sent.lock().unwrap().push(response.clone());
                        let mut writer = stream;
                        let _ =
                            writer.write_all(serde_json::to_string(&response).unwrap().as_bytes());
                        let _ = writer.write_all(b"\n");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            socket_path,
            recorded,
            responses,
            panes,
            fail_focus,
            popup_error,
            listener_thread: Some(listener_thread),
            shutdown,
        }
    }

    pub fn set_panes(&self, panes: serde_json::Value) {
        let panes: Vec<_> = panes
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(pane_info)
            .collect();
        *self.panes.lock().unwrap() = serde_json::json!({ "panes": panes });
    }

    pub fn fail_focus(&self, fail: bool) {
        self.fail_focus.store(fail, Ordering::Relaxed);
    }

    pub fn popup_error(&self, message: &str) {
        *self.popup_error.lock().unwrap() = Some(message.into());
    }

    pub fn calls_named(&self, method: &str) -> Vec<serde_json::Value> {
        self.recorded
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["method"] == method)
            .cloned()
            .collect()
    }

    pub fn responses(&self) -> Vec<serde_json::Value> {
        self.responses.lock().unwrap().clone()
    }

    pub fn stop(mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.socket_path);
        if let Some(thread) = self.listener_thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn wait_for(mut check: impl FnMut() -> bool, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for condition");
}
