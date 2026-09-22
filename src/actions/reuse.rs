use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::herdr::api::PaneInfo;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub viewer_pane_id: String,
    pub repo_cwd: String,
    pub toplevel: String,
}

/// Pane ids are session-local and the state directory is shared, so the socket scopes the key.
pub fn key(socket_path: &str, opener: &str) -> String {
    format!("{socket_path}\u{1f}{opener}")
}

pub fn load(state_dir: &Path) -> BTreeMap<String, Record> {
    std::fs::read_to_string(state_dir.join("split-panes.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(state_dir: &Path, records: &BTreeMap<String, Record>) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let tmp = state_dir.join(format!("split-panes.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(records).unwrap_or_default())?;
    std::fs::rename(tmp, state_dir.join("split-panes.json"))
}

pub fn with_lock<T>(state_dir: &Path, f: impl FnOnce() -> T) -> std::io::Result<T> {
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};
    std::fs::create_dir_all(state_dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir.join("split-panes.lock"))?;
    // flock is released when `file` drops, including on panic.
    let start = Instant::now();
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ) {
            return Err(error);
        }
        if start.elapsed() >= Duration::from_secs(2) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "split reuse lock is busy",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(f())
}

pub fn may_reuse(record: &Record, toplevel: &str, viewer: Option<&PaneInfo>) -> bool {
    let Some(viewer) = viewer else { return false };
    record.toplevel == toplevel
        && viewer.label.as_deref() == Some(crate::actions::VIEWER_TITLE)
        && viewer.cwd.as_deref() == Some(record.repo_cwd.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::api::PaneInfo;

    fn record() -> Record {
        Record {
            viewer_pane_id: "w1:p9".into(),
            repo_cwd: "/repo/sub".into(),
            toplevel: "/repo".into(),
        }
    }

    fn viewer(label: &str, cwd: &str) -> PaneInfo {
        PaneInfo {
            pane_id: "w1:p9".into(),
            label: Some(label.into()),
            cwd: Some(cwd.into()),
            foreground_cwd: None,
        }
    }

    #[test]
    fn reuse_needs_the_same_toplevel_title_and_cwd() {
        assert!(may_reuse(
            &record(),
            "/repo",
            Some(&viewer("Hunks", "/repo/sub"))
        ));
        assert!(!may_reuse(
            &record(),
            "/other",
            Some(&viewer("Hunks", "/repo/sub"))
        ));
        assert!(!may_reuse(
            &record(),
            "/repo",
            Some(&viewer("zsh", "/repo/sub"))
        ));
        assert!(!may_reuse(
            &record(),
            "/repo",
            Some(&viewer("Hunks", "/elsewhere"))
        ));
        assert!(!may_reuse(&record(), "/repo", None));
    }

    #[test]
    fn keys_are_scoped_by_socket_path() {
        assert_ne!(key("/a/herdr.sock", "w1:p1"), key("/b/herdr.sock", "w1:p1"));
    }

    #[test]
    fn a_busy_lock_times_out_and_can_be_acquired_after_release() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_owned();
        let (ready, acquired) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            with_lock(&path, || {
                ready.send(()).unwrap();
                released.recv().unwrap();
            })
            .unwrap();
        });
        acquired.recv().unwrap();
        let path = dir.path().to_owned();
        let (done, result) = mpsc::channel();
        let start = Instant::now();
        let contender = std::thread::spawn(move || {
            done.send(with_lock(&path, || ())).unwrap();
        });
        let result = result.recv_timeout(Duration::from_secs(4));
        let elapsed = start.elapsed();
        release.send(()).unwrap();
        holder.join().unwrap();
        contender.join().unwrap();
        let error = result
            .expect("busy lock must return within four seconds")
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(error.to_string().contains("lock is busy"));
        assert!(elapsed >= Duration::from_secs(2) && elapsed < Duration::from_secs(4));
        assert_eq!(with_lock(dir.path(), || 42).unwrap(), 42);
    }

    #[test]
    fn concurrent_updates_under_the_lock_keep_every_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    with_lock(&path, || {
                        let mut map = load(&path);
                        std::thread::sleep(std::time::Duration::from_millis(15));
                        map.insert(key("/s/herdr.sock", &format!("w1:p{i}")), record());
                        save(&path, &map).unwrap();
                    })
                    .unwrap();
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        assert_eq!(load(&path).len(), 8, "a read-modify-write was lost");
    }

    #[test]
    fn an_unreadable_file_is_an_empty_map_and_save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("split-panes.json"), "{not json").unwrap();
        assert!(load(dir.path()).is_empty());
        let mut map = std::collections::BTreeMap::new();
        map.insert(key("/a/herdr.sock", "w1:p1"), record());
        save(dir.path(), &map).unwrap();
        assert_eq!(load(dir.path()).len(), 1);
    }
}
