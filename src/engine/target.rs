//! The target of a review (spec 10.2): chosen once, remembered per worktree, re-verified every refresh.
use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::host::{HostFailure, PaneRecord, SessionRef};
use crate::actions::reuse;

const TARGETS_FILE: &str = "targets.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Pane {
        pane: String,
        socket: String,
        agent: String,
        session: Option<SessionRef>,
        title: String,
    },
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetState {
    Unverified,
    /// The status word the host reported: idle, working, blocked, done, unknown.
    Live(String),
    Restarted(String),
    Left,
    Gone,
    NoHost,
    Clipboard,
}

/// Where a send went, as the send verified it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Destination {
    Pane {
        pane: String,
        agent: String,
        session: Option<SessionRef>,
    },
    Clipboard {
        clipboard: bool,
    },
}

impl Destination {
    pub fn clipboard() -> Self {
        Self::Clipboard { clipboard: true }
    }

    /// The shape checks of a target record, on the three fields a destination carries.
    pub fn is_well_formed(&self) -> bool {
        match self {
            Self::Clipboard { clipboard } => *clipboard,
            Self::Pane {
                pane,
                agent,
                session,
            } => is_pane_id(pane) && is_agent(agent) && session.as_ref().is_none_or(check_session),
        }
    }
}

/// How the target was chosen; only `Picked` is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Remembered,
    Opener,
    Picked,
}

/// `w<digits>:p<alphanumerics>`, the host's grammar; a pane id is only ever a request parameter.
pub fn is_pane_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('w') else {
        return false;
    };
    let Some((workspace, pane)) = rest.split_once(":p") else {
        return false;
    };
    !workspace.is_empty()
        && workspace.bytes().all(|b| b.is_ascii_digit())
        && !pane.is_empty()
        && pane.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn printable(text: &str) -> bool {
    !text.chars().any(|c| c.is_control())
}

fn is_agent(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn check_session(session: &SessionRef) -> bool {
    match session.kind.as_str() {
        "id" => !session.value.is_empty() && printable(&session.value),
        "path" => printable(&session.value) && Path::new(&session.value).is_absolute(),
        _ => false,
    }
}

/// The on-disk record: a pane with its socket, or `{"clipboard": true}`; `at` in both.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clipboard: Option<bool>,
    #[serde(default)]
    at: u64,
}

impl Record {
    fn of(target: &Target) -> Self {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match target {
            Target::Clipboard => Self {
                clipboard: Some(true),
                at,
                ..Self::default()
            },
            Target::Pane {
                pane,
                socket,
                agent,
                session,
                title,
            } => Self {
                pane: Some(pane.clone()),
                socket: Some(socket.clone()),
                agent: Some(agent.clone()),
                session: session.clone(),
                title: Some(title.clone()),
                clipboard: None,
                at,
            },
        }
    }

    /// Every field checked by shape before any of it is used.
    fn target(self) -> Option<Target> {
        if self.clipboard == Some(true) {
            return Some(Target::Clipboard);
        }
        let (pane, socket, agent) = (self.pane?, self.socket?, self.agent?);
        let title = self.title.unwrap_or_default();
        let well_formed = is_pane_id(&pane)
            && Path::new(&socket).is_absolute()
            && printable(&socket)
            && is_agent(&agent)
            && self.session.as_ref().is_none_or(check_session)
            && printable(&title);
        well_formed.then_some(Target::Pane {
            pane,
            socket,
            agent,
            session: self.session,
            title,
        })
    }
}

/// Keyed by canonical toplevel; a record failing its shape is dropped, as is an unreadable
/// or malformed file, with the reason.
pub fn load_targets(state_dir: &Path) -> (BTreeMap<String, Target>, Option<String>) {
    let text = match std::fs::read_to_string(state_dir.join(TARGETS_FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (BTreeMap::new(), None),
        Err(e) => return (BTreeMap::new(), Some(format!("{TARGETS_FILE}: {e}"))),
        Ok(text) => text,
    };
    // Each record is decoded on its own, so one wrong field type drops that record, not every worktree's.
    let records: BTreeMap<String, serde_json::Value> = match serde_json::from_str(&text) {
        Ok(records) => records,
        Err(e) => return (BTreeMap::new(), Some(format!("{TARGETS_FILE}: {e}"))),
    };
    let total = records.len();
    let kept: BTreeMap<String, Target> = records
        .into_iter()
        .filter_map(|(toplevel, value)| {
            serde_json::from_value::<Record>(value)
                .ok()
                .and_then(Record::target)
                .map(|t| (toplevel, t))
        })
        .collect();
    let dropped = total - kept.len();
    let problem = (dropped > 0).then(|| format!("{TARGETS_FILE}: {dropped} unusable record(s)"));
    (kept, problem)
}

/// Read-modify-write under the state lock, then an atomic replace, as `save_mark` does.
pub fn save_target(state_dir: &Path, toplevel: &str, target: &Target) -> std::io::Result<()> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || {
        // Records that fail their shape are rewritten out, as `load_marks` drops a bad mark.
        let (targets, _) = load_targets(state_dir);
        let mut records: BTreeMap<String, Record> = targets
            .iter()
            .map(|(k, t)| (k.clone(), Record::of(t)))
            .collect();
        records.insert(toplevel.to_string(), Record::of(target));
        let tmp = state_dir.join(format!("{TARGETS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&records).unwrap_or_default(),
        )?;
        std::fs::rename(tmp, state_dir.join(TARGETS_FILE))
    })?
}

/// `save_target` guarded by a write ticket: the comparison happens under the lock, so of two writers
/// the one requested last lands whatever order their tasks ran in, across picks and within one.
pub fn save_target_if(
    state_dir: &Path,
    toplevel: &str,
    target: &Target,
    ticket: u64,
    latest: &std::sync::atomic::AtomicU64,
) -> std::io::Result<bool> {
    if !state_dir.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "state directory must be absolute",
        ));
    }
    reuse::with_lock(state_dir, || -> std::io::Result<bool> {
        if latest.load(std::sync::atomic::Ordering::SeqCst) != ticket {
            return Ok(false);
        }
        let (targets, _) = load_targets(state_dir);
        let mut records: BTreeMap<String, Record> = targets
            .iter()
            .map(|(k, t)| (k.clone(), Record::of(t)))
            .collect();
        records.insert(toplevel.to_string(), Record::of(target));
        let tmp = state_dir.join(format!("{TARGETS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&records).unwrap_or_default(),
        )?;
        std::fs::rename(tmp, state_dir.join(TARGETS_FILE))?;
        Ok(true)
    })?
}

/// The state a fresh check yields; `None` when the check could not run, so the previous state
/// stands (spec 10.2 "Re-verification").
pub fn compare(
    target: &Target,
    socket_path: Option<&str>,
    fresh: &Result<PaneRecord, HostFailure>,
) -> Option<TargetState> {
    let Target::Pane {
        pane,
        socket,
        agent,
        session,
        ..
    } = target
    else {
        return Some(TargetState::Clipboard);
    };
    let Some(socket_path) = socket_path else {
        return Some(TargetState::NoHost);
    };
    if socket_path != socket {
        // The record was picked on another host; this one never had that pane.
        return Some(TargetState::Gone);
    }
    match fresh {
        Err(HostFailure::NoHost(_)) => Some(TargetState::NoHost),
        Err(HostFailure::Api { code, .. })
            if code == "pane_not_found" || code == "target_pane_not_found" =>
        {
            Some(TargetState::Gone)
        }
        Err(_) => None,
        // A reply about another pane is no answer about this one: the check did not run.
        Ok(record) if record.pane_id != *pane => None,
        Ok(record) => {
            if record.agent.as_deref() != Some(agent.as_str()) {
                return Some(TargetState::Left);
            }
            let status = record
                .agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            // A known session matches only itself; a missing one matches by pane and kind.
            match (session, &record.agent_session) {
                (Some(known), Some(now)) if known == now => Some(TargetState::Live(status)),
                (Some(_), _) => Some(TargetState::Restarted(status)),
                (None, _) => Some(TargetState::Live(status)),
            }
        }
    }
}

/// The session a record without one learns from a fresh check (spec 10.2 "Session matching");
/// the engine rewrites the record with it on its next write.
pub fn adopted_session(target: &Target, fresh: &PaneRecord) -> Option<SessionRef> {
    match target {
        Target::Pane {
            session: None,
            agent,
            ..
        } if fresh.agent.as_deref() == Some(agent.as_str()) => fresh.agent_session.clone(),
        _ => None,
    }
}

/// `cwd` equals `toplevel` or lies under it, by path components: `/r2` is not under `/r`.
pub fn is_under(cwd: &str, toplevel: &str) -> bool {
    let cwd: Vec<Component<'_>> = Path::new(cwd).components().collect();
    let top: Vec<Component<'_>> = Path::new(toplevel).components().collect();
    cwd.len() >= top.len() && cwd.iter().zip(&top).all(|(a, b)| a == b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRow {
    pub record: PaneRecord,
    pub this_worktree: bool,
}

/// Agent panes other than the viewer's own, this worktree's first, the host's order within a group.
pub fn rows(panes: Vec<PaneRecord>, toplevel: &str, own_pane: Option<&str>) -> Vec<PaneRow> {
    let mut rows: Vec<PaneRow> = panes
        .into_iter()
        .filter(|p| p.agent.is_some() && Some(p.pane_id.as_str()) != own_pane)
        .map(|record| {
            let this_worktree = [&record.cwd, &record.foreground_cwd]
                .into_iter()
                .flatten()
                .any(|dir| is_under(dir, toplevel));
            PaneRow {
                record,
                this_worktree,
            }
        })
        .collect();
    // A stable sort keeps the host's order inside each group.
    rows.sort_by_key(|r| !r.this_worktree);
    rows
}

pub fn destination_of(record: &PaneRecord) -> Destination {
    Destination::Pane {
        pane: record.pane_id.clone(),
        agent: record.agent.clone().unwrap_or_default(),
        session: record.agent_session.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(agent: Option<&str>, status: &str, session: Option<&str>) -> PaneRecord {
        PaneRecord {
            pane_id: "w4:p2".into(),
            agent: agent.map(str::to_string),
            agent_status: Some(status.into()),
            agent_session: session.map(|v| SessionRef {
                kind: "id".into(),
                value: v.into(),
            }),
            cwd: Some("/home/u/repo".into()),
            ..PaneRecord::default()
        }
    }

    fn target(session: Option<&str>) -> Target {
        Target::Pane {
            pane: "w4:p2".into(),
            socket: "/run/h.sock".into(),
            agent: "codex".into(),
            session: session.map(|v| SessionRef {
                kind: "id".into(),
                value: v.into(),
            }),
            title: "fix".into(),
        }
    }

    #[test]
    fn every_state_of_the_table_is_reached() {
        let socket = Some("/run/h.sock");
        let t = target(Some("s1"));
        assert_eq!(
            compare(&t, socket, &Ok(pane(Some("codex"), "idle", Some("s1")))),
            Some(TargetState::Live("idle".into()))
        );
        assert_eq!(
            compare(&t, socket, &Ok(pane(Some("codex"), "blocked", Some("s2")))),
            Some(TargetState::Restarted("blocked".into()))
        );
        assert_eq!(
            compare(&t, socket, &Ok(pane(Some("claude"), "idle", Some("s1")))),
            Some(TargetState::Left)
        );
        assert_eq!(
            compare(&t, socket, &Ok(pane(None, "unknown", None))),
            Some(TargetState::Left)
        );
        let gone = Err(HostFailure::Api {
            code: "pane_not_found".into(),
            message: "x".into(),
        });
        assert_eq!(compare(&t, socket, &gone), Some(TargetState::Gone));
        assert_eq!(
            compare(&t, socket, &Err(HostFailure::NoHost("off".into()))),
            Some(TargetState::NoHost)
        );
        assert_eq!(
            compare(&t, None, &Ok(pane(Some("codex"), "idle", Some("s1")))),
            Some(TargetState::NoHost)
        );
        // A check that could not run changes nothing.
        assert_eq!(
            compare(&t, socket, &Err(HostFailure::After("timeout".into()))),
            None
        );
        assert_eq!(
            compare(&t, socket, &Err(HostFailure::Before("eintr".into()))),
            None
        );
        assert_eq!(
            compare(
                &t,
                socket,
                &Err(HostFailure::Api {
                    code: "internal".into(),
                    message: "x".into()
                })
            ),
            None
        );
        // Another socket: this host never had that pane.
        assert_eq!(
            compare(
                &t,
                Some("/run/other.sock"),
                &Ok(pane(Some("codex"), "idle", Some("s1")))
            ),
            Some(TargetState::Gone)
        );
        // A record without a session matches by pane and kind, and learns the session it now sees.
        let fresh = pane(Some("codex"), "working", Some("s9"));
        assert_eq!(
            compare(&target(None), socket, &Ok(fresh.clone())),
            Some(TargetState::Live("working".into()))
        );
        assert_eq!(
            adopted_session(&target(None), &fresh).map(|s| s.value),
            Some("s9".to_string())
        );
        assert_eq!(
            adopted_session(&target(Some("s1")), &fresh),
            None,
            "a known session is never replaced by a check"
        );
        // A known session that the host no longer reports is a restart, not continuity.
        assert_eq!(
            compare(
                &target(Some("s1")),
                socket,
                &Ok(pane(Some("codex"), "idle", None))
            ),
            Some(TargetState::Restarted("idle".into()))
        );
        assert_eq!(
            compare(
                &Target::Clipboard,
                None,
                &Err(HostFailure::NoHost("off".into()))
            ),
            Some(TargetState::Clipboard)
        );
        // A status the host did not name reads as unknown.
        let mut nameless = pane(Some("codex"), "idle", Some("s1"));
        nameless.agent_status = None;
        assert_eq!(
            compare(&t, socket, &Ok(nameless)),
            Some(TargetState::Live("unknown".into()))
        );
        // A reply about another pane is no answer about this one.
        let mut other_pane = pane(Some("codex"), "idle", Some("s1"));
        other_pane.pane_id = "w4:p9".into();
        assert_eq!(compare(&t, socket, &Ok(other_pane)), None);
    }

    #[test]
    fn records_are_checked_by_shape_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let good = serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex",
            "session": { "kind": "id", "value": "01a0" }, "title": "fix", "at": 1 });
        let clip = serde_json::json!({ "clipboard": true, "at": 2 });
        let bad = [
            serde_json::json!({ "pane": "w4;p2", "socket": "/run/h.sock", "agent": "codex", "session": null, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "run/h.sock", "agent": "codex", "session": null, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "Codex", "session": null, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex", "session": { "kind": "pid", "value": "1" }, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex", "session": { "kind": "path", "value": "rel/x" }, "title": "", "at": 1 }),
            serde_json::json!({ "pane": "w4:p2", "socket": "/run/h.sock", "agent": "codex", "session": { "kind": "id", "value": "a\u{1b}b" }, "title": "", "at": 1 }),
            serde_json::json!({ "at": 1 }),
        ];
        let mut all = serde_json::Map::new();
        all.insert("/good".into(), good);
        all.insert("/clip".into(), clip);
        for (i, record) in bad.iter().enumerate() {
            all.insert(format!("/bad{i}"), record.clone());
        }
        std::fs::write(
            dir.path().join("targets.json"),
            serde_json::to_vec(&all).unwrap(),
        )
        .unwrap();
        let (targets, problem) = load_targets(dir.path());
        assert_eq!(targets.len(), 2, "{targets:?}");
        assert!(matches!(targets.get("/good"), Some(Target::Pane { pane, .. }) if pane == "w4:p2"));
        assert_eq!(targets.get("/clip"), Some(&Target::Clipboard));
        assert_eq!(
            problem.as_deref(),
            Some("targets.json: 7 unusable record(s)")
        );
        // Malformed file: absent with the reason; a later save rewrites it.
        std::fs::write(dir.path().join("targets.json"), b"{").unwrap();
        let (targets, problem) = load_targets(dir.path());
        assert!(targets.is_empty() && problem.is_some());
        save_target(dir.path(), "/repo", &Target::Clipboard).unwrap();
        assert_eq!(
            load_targets(dir.path()).0.get("/repo"),
            Some(&Target::Clipboard)
        );
        assert!(save_target(
            std::path::Path::new("relative"),
            "/repo",
            &Target::Clipboard
        )
        .is_err());
        // A write with an older ticket is skipped under the lock; the latest requested record stands.
        let latest = std::sync::atomic::AtomicU64::new(2);
        assert!(!save_target_if(dir.path(), "/repo", &target(Some("old")), 1, &latest).unwrap());
        assert_eq!(
            load_targets(dir.path()).0.get("/repo"),
            Some(&Target::Clipboard)
        );
        assert!(save_target_if(dir.path(), "/repo", &target(Some("new")), 2, &latest).unwrap());
        assert!(
            matches!(load_targets(dir.path()).0.get("/repo"), Some(Target::Pane { session: Some(s), .. }) if s.value == "new")
        );
    }

    #[test]
    fn a_sibling_directory_with_a_shared_prefix_is_another_pane() {
        assert!(is_under("/home/u/repo", "/home/u/repo"));
        assert!(is_under("/home/u/repo/src", "/home/u/repo"));
        assert!(!is_under("/home/u/repo2", "/home/u/repo"));
        assert!(!is_under("/home/u", "/home/u/repo"));
        let mut here = PaneRecord {
            pane_id: "w1:p1".into(),
            agent: Some("codex".into()),
            cwd: Some("/home/u/repo2".into()),
            ..PaneRecord::default()
        };
        let listed = rows(vec![here.clone()], "/home/u/repo", None);
        assert!(!listed[0].this_worktree);
        here.foreground_cwd = Some("/home/u/repo/sub".into());
        assert!(rows(vec![here], "/home/u/repo", None)[0].this_worktree);
    }

    #[test]
    fn rows_drop_shells_and_the_viewer_and_put_this_worktree_first() {
        let p = |id: &str, agent: Option<&str>, cwd: &str| PaneRecord {
            pane_id: id.into(),
            agent: agent.map(str::to_string),
            cwd: Some(cwd.into()),
            ..PaneRecord::default()
        };
        let listed = rows(
            vec![
                p("w1:p1", None, "/r"),
                p("w1:p2", Some("codex"), "/other"),
                p("w1:p3", Some("claude"), "/r/x"),
                p("w1:p4", Some("kimi"), "/r"),
            ],
            "/r",
            Some("w1:p4"),
        );
        let ids: Vec<_> = listed.iter().map(|r| r.record.pane_id.as_str()).collect();
        assert_eq!(ids, ["w1:p3", "w1:p2"]);
        assert!(listed[0].this_worktree && !listed[1].this_worktree);
    }

    #[test]
    fn pane_ids_follow_the_hosts_grammar() {
        assert!(is_pane_id("w4:p2") && is_pane_id("w12:pA9"));
        assert!(
            !is_pane_id("w4")
                && !is_pane_id("4:p2")
                && !is_pane_id("w4:p")
                && !is_pane_id("w4:p2 ")
                && !is_pane_id("w4:p2;rm")
        );
    }
}
