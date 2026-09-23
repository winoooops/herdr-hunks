use herdr_hunks::engine::{
    init_process_env, spawn, Command, DiffState, FileKey, SessionConfig, GIT_CHILD_ENV,
};
use std::time::{Duration, Instant};

fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success(),
        "git {args:?}"
    );
}

// The only test in this binary, so nothing else reads the environment while it is set.
#[test]
fn the_environment_policy_is_applied_and_makes_pathspecs_literal() {
    init_process_env();
    for (key, value) in GIT_CHILD_ENV {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    assert!(std::env::var_os("GIT_EXTERNAL_DIFF").is_none());

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a-first.txt"), "first\n").unwrap();
    std::fs::write(p.join("b*.txt"), "star\n").unwrap();
    std::fs::write(p.join("bb.txt"), "plain\n").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("a-first.txt"), "FIRST\n").unwrap();
    std::fs::write(p.join("b*.txt"), "STAR\n").unwrap();
    std::fs::write(p.join("bb.txt"), "PLAIN\n").unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let handle = spawn(rt.handle(), SessionConfig::production(p.to_path_buf()));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut selection_sent = false;
    loop {
        assert!(
            Instant::now() < deadline,
            "the glob-named file never loaded"
        );
        if let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) {
            if !selection_sent && s.files.iter().any(|f| f.path == "b*.txt" && !f.staged) {
                assert_eq!(
                    s.selected.as_ref().map(|key| key.path.as_str()),
                    Some("a-first.txt")
                );
                handle
                    .commands
                    .send(Command::Select(FileKey {
                        path: "b*.txt".into(),
                        staged: false,
                        untracked: false,
                    }))
                    .unwrap();
                selection_sent = true;
            }
            if let DiffState::Ready(d) = &s.diff {
                if d.key.path == "b*.txt" {
                    assert!(d.raw_diff.contains("STAR"), "its own change is missing");
                    assert!(
                        !d.raw_diff.contains("FIRST"),
                        "the initially selected file leaked into the diff"
                    );
                    assert!(
                        !d.raw_diff.contains("PLAIN"),
                        "a neighbouring file leaked into the diff"
                    );
                    assert_eq!(d.file_diff.hunks.len(), 1);
                    break;
                }
            }
        }
    }
}
