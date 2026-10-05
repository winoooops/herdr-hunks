//! The host surface of spec 10.6: three methods behind one trait, injected like the watcher.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::herdr::client::{HerdrClient, HerdrClientError};

/// The agent's session as the host reports it: `kind` is `id` or `path`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRef {
    pub kind: String,
    pub value: String,
}

/// What `pane.get` and a `pane.list` row carry that this section reads.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct PaneRecord {
    pub pane_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: Option<String>,
    #[serde(default)]
    pub agent_session: Option<SessionRef>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, rename = "terminal_title_stripped")]
    pub title: Option<String>,
}

/// Why a host call failed, in the terms 10.4's settlement and 10.2's check need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostFailure {
    /// The socket path is unset, the file is missing, or the connection was refused.
    NoHost(String),
    /// Failed before the request line was written in full; the host never saw it.
    Before(String),
    /// Written in full; no complete, well-formed reply by the deadline. The host may have acted.
    After(String),
    /// The host's own error reply.
    Api { code: String, message: String },
}

impl HostFailure {
    pub fn message(&self) -> String {
        match self {
            Self::NoHost(m) | Self::Before(m) | Self::After(m) => m.clone(),
            Self::Api { message, .. } => message.clone(),
        }
    }
}

pub trait HostClient: Send + Sync + 'static {
    fn pane_get(&self, pane: &str) -> Result<PaneRecord, HostFailure>;
    fn pane_list(&self) -> Result<Vec<PaneRecord>, HostFailure>;
    /// `agent.prompt` without `wait`: the host pastes and presses Enter 300 ms later.
    fn agent_prompt(&self, pane: &str, text: &str) -> Result<(), HostFailure>;
}

/// How long the engine waits on a host call before the outcome is uncertain (10.6).
pub const ENGINE_WAIT: Duration = Duration::from_secs(10);

pub struct HerdrHost {
    client: HerdrClient,
}

impl HerdrHost {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            client: HerdrClient::new(socket_path),
        }
    }
}

fn classify(error: HerdrClientError) -> HostFailure {
    use std::io::ErrorKind;
    match error {
        HerdrClientError::Connect { source, path }
            if matches!(
                source.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) =>
        {
            HostFailure::NoHost(format!("no host at {}: {source}", path.display()))
        }
        HerdrClientError::Connect { source, .. } => HostFailure::Before(source.to_string()),
        HerdrClientError::Write(e) => HostFailure::Before(e.to_string()),
        HerdrClientError::Read(e) => HostFailure::After(e.to_string()),
        HerdrClientError::Decode(e) => HostFailure::After(e.to_string()),
        HerdrClientError::Api(body) => {
            let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            HostFailure::Api {
                code: value["code"].as_str().unwrap_or("error").to_string(),
                message: value["message"].as_str().unwrap_or(&body).to_string(),
            }
        }
    }
}

impl HostClient for HerdrHost {
    fn pane_get(&self, pane: &str) -> Result<PaneRecord, HostFailure> {
        let response = self
            .client
            .request("pane.get", json!({ "pane_id": pane }))
            .map_err(classify)?;
        serde_json::from_value(response["result"]["pane"].clone())
            .map_err(|e| HostFailure::After(format!("pane.get reply: {e}")))
    }

    fn pane_list(&self) -> Result<Vec<PaneRecord>, HostFailure> {
        let response = self
            .client
            .request("pane.list", json!({}))
            .map_err(classify)?;
        serde_json::from_value(response["result"]["panes"].clone())
            .map_err(|e| HostFailure::After(format!("pane.list reply: {e}")))
    }

    fn agent_prompt(&self, pane: &str, text: &str) -> Result<(), HostFailure> {
        let response = self
            .client
            .request("agent.prompt", json!({ "target": pane, "text": text }))
            .map_err(classify)?;
        // Only an agent_prompted reply confirms that the host accepted the prompt.
        if response["result"]["type"].as_str() == Some("agent_prompted") {
            Ok(())
        } else {
            Err(HostFailure::After(format!(
                "agent.prompt reply not understood: {response}"
            )))
        }
    }
}

/// `None` when `HERDR_SOCKET_PATH` is unset: every pane target is then `NoHost` (10.6).
pub fn from_env() -> Option<Arc<dyn HostClient>> {
    std::env::var_os("HERDR_SOCKET_PATH")
        .map(PathBuf::from)
        .map(|path| Arc::new(HerdrHost::new(path)) as Arc<dyn HostClient>)
}

/// A scripted host for tests: panes answered from a table the test edits mid-run, a result
/// for each `agent.prompt`, every call recorded. Compiled always so integration tests can use it.
#[doc(hidden)]
#[derive(Default)]
pub struct Scripted {
    pub panes: Mutex<BTreeMap<String, PaneRecord>>,
    /// `Err` fails every `pane.get` and `pane.list` with it until cleared.
    pub list_failure: Mutex<Option<HostFailure>>,
    /// `Some(record)` makes every `pane.get` answer that record whatever pane was asked for.
    pub answer_pane_get_with: Mutex<Option<PaneRecord>>,
    /// Pushed results for `agent_prompt`, consumed first to last; empty means `Ok(())`.
    pub prompt_results: Mutex<Vec<Result<(), HostFailure>>>,
    /// Every `agent_prompt` call as `(pane, text)`, and when each began (before its delay).
    pub prompts: Mutex<Vec<(String, String)>>,
    pub prompt_started: Mutex<Vec<std::time::Instant>>,
    /// Every method called, in order.
    pub calls: Mutex<Vec<String>>,
    /// A delay before each `agent_prompt` answers, for tests of the engine's wait.
    pub prompt_delay: Mutex<Option<Duration>>,
    /// Observes the comment store at the moment of the call.
    pub on_prompt: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl Scripted {
    pub fn with_panes(panes: Vec<PaneRecord>) -> Arc<Self> {
        let scripted = Self::default();
        *scripted.panes.lock().unwrap() =
            panes.into_iter().map(|p| (p.pane_id.clone(), p)).collect();
        Arc::new(scripted)
    }

    pub fn set_pane(&self, pane: PaneRecord) {
        self.panes
            .lock()
            .unwrap()
            .insert(pane.pane_id.clone(), pane);
    }

    pub fn remove_pane(&self, pane: &str) {
        self.panes.lock().unwrap().remove(pane);
    }
}

impl HostClient for Scripted {
    fn pane_get(&self, pane: &str) -> Result<PaneRecord, HostFailure> {
        self.calls.lock().unwrap().push("pane.get".into());
        if let Some(failure) = self.list_failure.lock().unwrap().clone() {
            return Err(failure);
        }
        if let Some(record) = self.answer_pane_get_with.lock().unwrap().clone() {
            return Ok(record);
        }
        self.panes
            .lock()
            .unwrap()
            .get(pane)
            .cloned()
            .ok_or(HostFailure::Api {
                code: "pane_not_found".into(),
                message: "no such pane".into(),
            })
    }

    fn pane_list(&self) -> Result<Vec<PaneRecord>, HostFailure> {
        self.calls.lock().unwrap().push("pane.list".into());
        if let Some(failure) = self.list_failure.lock().unwrap().clone() {
            return Err(failure);
        }
        Ok(self.panes.lock().unwrap().values().cloned().collect())
    }

    fn agent_prompt(&self, pane: &str, text: &str) -> Result<(), HostFailure> {
        self.calls.lock().unwrap().push("agent.prompt".into());
        self.prompt_started
            .lock()
            .unwrap()
            .push(std::time::Instant::now());
        if let Some(observe) = self.on_prompt.lock().unwrap().as_ref() {
            observe();
        }
        if let Some(delay) = *self.prompt_delay.lock().unwrap() {
            std::thread::sleep(delay);
        }
        self.prompts
            .lock()
            .unwrap()
            .push((pane.to_string(), text.to_string()));
        let mut results = self.prompt_results.lock().unwrap();
        if results.is_empty() {
            Ok(())
        } else {
            results.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_failures_are_classified_by_phase() {
        let missing = HerdrClientError::Connect {
            path: "/nowhere/h.sock".into(),
            source: std::io::ErrorKind::NotFound.into(),
        };
        assert!(matches!(classify(missing), HostFailure::NoHost(_)));
        let refused = HerdrClientError::Connect {
            path: "/x".into(),
            source: std::io::ErrorKind::ConnectionRefused.into(),
        };
        assert!(matches!(classify(refused), HostFailure::NoHost(_)));
        let slow = HerdrClientError::Connect {
            path: "/x".into(),
            source: std::io::ErrorKind::TimedOut.into(),
        };
        assert!(matches!(classify(slow), HostFailure::Before(_)));
        assert!(matches!(
            classify(HerdrClientError::Write(
                std::io::ErrorKind::BrokenPipe.into()
            )),
            HostFailure::Before(_)
        ));
        assert!(matches!(
            classify(HerdrClientError::Read(std::io::ErrorKind::TimedOut.into())),
            HostFailure::After(_)
        ));
        let api = classify(HerdrClientError::Api(
            r#"{"code":"agent_not_ready","message":"blocked"}"#.into(),
        ));
        assert_eq!(
            api,
            HostFailure::Api {
                code: "agent_not_ready".into(),
                message: "blocked".into()
            }
        );
    }

    #[test]
    fn a_pane_record_reads_the_hosts_shape() {
        let pane: PaneRecord = serde_json::from_value(serde_json::json!({
            "pane_id": "w4:p2", "agent": "codex", "agent_status": "idle",
            "agent_session": { "agent": "codex", "kind": "id", "source": "herdr:codex", "value": "01a0" },
            "cwd": "/home/u/repo", "terminal_title_stripped": "fix the cart", "focused": false, "revision": 3
        }))
        .unwrap();
        assert_eq!(pane.agent.as_deref(), Some("codex"));
        assert_eq!(
            pane.agent_session.as_ref().map(|s| s.value.as_str()),
            Some("01a0")
        );
        assert_eq!(pane.title.as_deref(), Some("fix the cart"));
        // A shell pane: no agent, status unknown.
        let shell: PaneRecord = serde_json::from_value(
            serde_json::json!({ "pane_id": "w4:p1", "agent_status": "unknown" }),
        )
        .unwrap();
        assert!(shell.agent.is_none() && shell.agent_session.is_none());
    }
}
