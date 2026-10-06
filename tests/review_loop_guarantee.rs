use herdr_hunks::engine::{
    init_process_env, spawn, Command, DiffState, FileKey, SessionConfig, Snapshot,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ALLOWED: [&str; 10] = [
    "--version",
    "rev-parse",
    "status",
    "diff",
    "ls-files",
    "show",
    "cat-file",
    "symbolic-ref",
    "merge-base",
    "for-each-ref",
];

fn real_git() -> PathBuf {
    let out = Proc::new("sh")
        .arg("-c")
        .arg("command -v git")
        .output()
        .unwrap();
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

/// The engine polls this repository while the harness drives it, and both take `index.lock`.
/// That one failure is retried; anything else fails the test with git's own message.
fn git(real: &Path, dir: &Path, args: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = Proc::new(real)
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).to_string();
        }
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            err.contains("index.lock") && Instant::now() < deadline,
            "git {args:?}: {err}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Content hash of every file outside `.git`, in path order. Pure Rust, so it is the same on macOS.
fn tree_hash(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path
                .strip_prefix(root)
                .map(|p| p.starts_with(".git"))
                .unwrap_or(false)
            {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out)
            } else {
                out.push(path)
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    assert!(!files.is_empty(), "nothing was hashed");
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(file.strip_prefix(dir).unwrap().to_string_lossy().as_bytes());
        hasher.update(std::fs::read(&file).expect("read file"));
    }
    format!("{:x}", hasher.finalize())
}

fn subcommand_of(argv: &str) -> &str {
    let args: Vec<&str> = argv.split_whitespace().collect();
    let mut i = 0;
    while args.get(i) == Some(&"-C") {
        i += 2;
    }
    args.get(i).copied().unwrap_or("")
}

#[allow(dead_code)]
mod support;

#[test]
fn the_review_loop_talks_to_the_host_alone_and_writes_only_its_files() {
    // The one test of this file: it owns the process (PATH, cwd, init_process_env).
    use herdr_hunks::engine::comments::{Anchor, AnchorComparison, Category, Span};
    use herdr_hunks::engine::dispatch::{
        Accepted, CopyRequest, CopyWhat, ReviewScope, SendKind, SendRequest,
    };
    use herdr_hunks::engine::host::{HerdrHost, SessionRef};
    use herdr_hunks::engine::nav::Side;
    use herdr_hunks::engine::{Target, TargetState};

    let original_cwd = std::env::current_dir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    std::env::set_current_dir(scratch.path()).unwrap();
    let real = real_git();
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    git(&real, p, &["init", "-q", "-b", "main"]);
    git(&real, p, &["config", "user.email", "t@example.com"]);
    git(&real, p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a.txt"), "one\n").unwrap();
    std::fs::write(p.join("b.txt"), "b\n").unwrap();
    git(&real, p, &["add", "-A"]);
    git(&real, p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("a.txt"), "ONE\n").unwrap(); // an unstaged row
    std::fs::write(p.join("b.txt"), "B\n").unwrap();
    git(&real, p, &["add", "b.txt"]); // a staged row
    std::fs::write(p.join("new.txt"), "n\n").unwrap(); // an untracked row
    let toplevel = p.canonicalize().unwrap().to_string_lossy().into_owned();

    // Record every engine git invocation in this process.
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("git.log");
    let wrapper = bin.path().join("git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap()
        ),
    );
    init_process_env();
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let refs_before = git(&real, p, &["for-each-ref"]);
    let tree_before = tree_hash(p);

    let fake_dir = tempfile::tempdir().unwrap();
    let fake = support::FakeHerdr::start(fake_dir.path());
    fake.set_panes(serde_json::json!([
        { "pane_id": "w1:p1", "cwd": toplevel },
        { "pane_id": "w1:p2", "agent": "codex", "agent_status": "idle", "cwd": toplevel,
          "agent_session": { "kind": "id", "value": "s-1" }, "terminal_title_stripped": "codex" }
    ]));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_millis(100);
    config.state_dir = Some(state.path().to_path_buf());
    config.host = Some(Arc::new(HerdrHost::new(fake.socket_path.clone())));
    config.socket_path = Some(fake.socket_path.to_string_lossy().into_owned());
    config.nonce = Arc::new(|counter| format!("t{counter:05}"));
    let handle = spawn(rt.handle(), config);
    let recv = |what: &str, pred: &dyn Fn(&Snapshot) -> bool| -> Arc<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last = None;
        while Instant::now() < deadline {
            if let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) {
                if pred(&s) {
                    return s;
                }
                last = Some(s);
            }
        }
        panic!("timed out waiting for {what}; last = {last:?}");
    };
    recv("rows", &|s| s.files.len() == 3);
    // Copy pending comments before the send leaves none (spec 10.6).
    handle.commands.send(Command::LoadPanes(1)).unwrap();
    let s = recv("panes", &|s| s.panes_seq == 1);
    assert_eq!(
        s.panes.as_ref().unwrap().len(),
        1,
        "the shell pane is not a row"
    );
    handle
        .commands
        .send(Command::SetTarget {
            token: 0,
            target: Target::Pane {
                pane: "w1:p2".into(),
                socket: fake.socket_path.to_string_lossy().into_owned(),
                agent: "codex".into(),
                session: Some(SessionRef {
                    kind: "id".into(),
                    value: "s-1".into(),
                }),
                title: "codex".into(),
            },
        })
        .unwrap();
    recv("live", &|s| {
        s.target_state == TargetState::Live("idle".into())
    });
    handle
        .commands
        .send(Command::AddComment {
            token: 1,
            anchor: Anchor {
                key: FileKey {
                    path: "a.txt".into(),
                    staged: false,
                    untracked: false,
                },
                side: Side::Additions,
                line: 1,
                span: Span::Line,
                comparison: AnchorComparison::Worktree,
            },
            category: Category::Bug,
            text: "shouting".into(),
        })
        .unwrap();
    recv("comment", &|s| s.comment_seq == 1 && s.comments.len() == 1);
    // `c` first, while the comment is pending: a copy claims nothing, and the file is the evidence.
    handle
        .commands
        .send(Command::Copy(CopyRequest {
            what: CopyWhat::Review,
        }))
        .unwrap();
    let s = recv("copied", &|s| s.copy_seq == 1);
    assert!(
        s.copy
            .as_ref()
            .unwrap()
            .notice
            .starts_with("copied 1 comments"),
        "{:?}",
        s.copy
    );
    assert!(std::fs::read_to_string(state.path().join("clipboard.md"))
        .unwrap()
        .starts_with("> Inline review — 1 item."));
    assert!(s.comments[0].is_pending(), "a copy stamps nothing");
    handle
        .commands
        .send(Command::Send(SendRequest {
            kind: SendKind::Feedback,
            accepted: Accepted::default(),
        }))
        .unwrap();
    let s = recv("sent, diff ready", &|s| {
        s.send_seq == 1 && matches!(s.diff, DiffState::Ready(_))
    });
    assert!(s.send_error.is_none(), "{:?}", s.send_error);
    handle
        .commands
        .send(Command::Send(SendRequest {
            kind: SendKind::Review {
                scope: ReviewScope::All,
            },
            accepted: Accepted::default(),
        }))
        .unwrap();
    let s = recv("requested", &|s| s.send_seq == 2);
    assert!(s.send_error.is_none(), "{:?}", s.send_error);
    handle.commands.send(Command::Shutdown).unwrap();
    rt.shutdown_timeout(Duration::from_secs(5));

    // The host saw the three methods and nothing else; one prompt per confirmed send.
    let all_calls = fake.all_calls();
    assert!(!all_calls.is_empty());
    assert!(
        all_calls.iter().all(|c| matches!(
            c["method"].as_str(),
            Some("pane.get" | "pane.list" | "agent.prompt")
        )),
        "{all_calls:?}"
    );
    let prompts = fake.calls_named("agent.prompt");
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0]["params"]["text"]
        .as_str()
        .unwrap()
        .starts_with("> Inline review — 1 item."));
    assert!(prompts[0]["params"]["text"]
        .as_str()
        .unwrap()
        .contains(&format!("{toplevel}/a.txt:1 (additions) [unstaged]")));
    assert!(prompts[1]["params"]["text"]
        .as_str()
        .unwrap()
        .starts_with("> Delegate a code review of these 3 changes:"));
    assert!(prompts.iter().all(|p| p["params"].get("wait").is_none()));
    fake.stop();

    // Every git call is one of the ten reads (`--version` among them); the request's diff loads too.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_change = Instant::now();
    let mut log_len = std::fs::metadata(&log).unwrap().len();
    loop {
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            Instant::now() < deadline,
            "wrapper log did not settle after shutdown"
        );
        let len = std::fs::metadata(&log).unwrap().len();
        if len != log_len {
            log_len = len;
            last_change = Instant::now();
        }
        if last_change.elapsed() >= Duration::from_millis(500) {
            break;
        }
    }

    let recorded = std::fs::read_to_string(&log).unwrap();
    for line in recorded.lines() {
        let sub = subcommand_of(line);
        assert!(
            ALLOWED.contains(&sub),
            "`{line}` runs {sub}, which is not allow-listed"
        );
    }
    // The state directory holds what the script wrote and nothing else.
    let mut written: Vec<String> = std::fs::read_dir(state.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        [
            "clipboard.md",
            "comments.json",
            "requests.json",
            "send.lock",
            "split-panes.lock",
            "targets.json"
        ]
    );
    assert_eq!(
        std::fs::read(p.join(".git/index")).unwrap(),
        index_before,
        ".git/index changed"
    );
    assert_eq!(
        git(&real, p, &["for-each-ref"]),
        refs_before,
        "refs changed"
    );
    assert_eq!(tree_hash(p), tree_before, "worktree changed");
    assert!(
        std::fs::read_dir(scratch.path()).unwrap().next().is_none(),
        "the viewer wrote to cwd"
    );
    std::env::set_current_dir(original_cwd).unwrap();
}
