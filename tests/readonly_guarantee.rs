use herdr_hunks::engine::{init_process_env, spawn, Command, DiffState, Scope, SessionConfig};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
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

#[test]
fn the_engine_never_mutates_the_repository() {
    let original_cwd = std::env::current_dir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    std::env::set_current_dir(scratch.path()).unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").expect("test HOME"));
    let home_before: Vec<_> = [
        "bases.json",
        "marks.json",
        "split-panes.json",
        "split-panes.lock",
        "config-problems.log",
    ]
    .into_iter()
    .map(|name| {
        let path = home.join(name);
        let existed = path.symlink_metadata().is_ok();
        (path, existed)
    })
    .collect();
    let real = real_git();
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    git(&real, p, &["init", "-q", "-b", "main"]);
    git(&real, p, &["config", "user.email", "t@example.com"]);
    git(&real, p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a.txt"), "one\n").unwrap();
    git(&real, p, &["add", "-A"]);
    git(&real, p, &["commit", "-q", "-m", "init"]);
    std::fs::write(p.join("b.txt"), "b\n").unwrap();
    git(&real, p, &["add", "b.txt"]);
    git(&real, p, &["commit", "-q", "-m", "b"]);
    git(&real, p, &["switch", "-q", "-c", "feat"]);
    std::fs::write(p.join("c.txt"), "c\n").unwrap();
    git(&real, p, &["add", "c.txt"]);
    git(&real, p, &["commit", "-q", "-m", "c"]);
    git(&real, p, &["branch", "other", "main"]);
    std::fs::write(p.join("b.txt"), "B\n").unwrap();
    git(&real, p, &["add", "b.txt"]); // a staged row
    std::fs::write(p.join("a.txt"), "ONE\n").unwrap(); // an unstaged row
    std::fs::write(p.join("new.txt"), "n\n").unwrap(); // an untracked row

    // recording wrapper, first in PATH
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("git.log");
    let wrapper = bin.path().join("git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\t%s\\t%s\\t%s\\n' \"$*\" \"$GIT_OPTIONAL_LOCKS\" \"$GIT_LITERAL_PATHSPECS\" \"$GIT_NO_LAZY_FETCH\" >> '{}'\nexec '{}' \"$@\"\n",
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

    // taken after all harness setup, so only the engine's effect is measured
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let refs_before = git(&real, p, &["for-each-ref"]);
    let tree_before = tree_hash(p);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_millis(100);
    config.state_dir = Some(state.path().to_path_buf());
    let handle = spawn(rt.handle(), config);
    // Load every worktree row before switching scope.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut loaded = std::collections::BTreeSet::new();
    let mut rows = 0usize;
    while Instant::now() < deadline && !(rows > 0 && loaded.len() == rows) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        rows = s.files.len();
        if let DiffState::Ready(d) = &s.diff {
            if loaded.insert((d.key.path.clone(), d.key.staged)) {
                handle.commands.send(Command::SelectNext).unwrap();
                handle.commands.send(Command::Refresh).unwrap();
            }
        }
    }
    assert_eq!(
        rows, 3,
        "expected a staged, an unstaged and an untracked row"
    );
    assert_eq!(loaded.len(), rows, "not every row was loaded: {loaded:?}");
    // Branch scope: every row, the ref list, a different base, and back.
    handle
        .commands
        .send(Command::SetScope(Scope::Branch))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut branch_loaded = std::collections::BTreeSet::new();
    let mut branch_rows = 0usize;
    while Instant::now() < deadline && !(branch_rows > 0 && branch_loaded.len() == branch_rows) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        if s.scope != Scope::Branch {
            continue;
        }
        branch_rows = s.files.len();
        if let DiffState::Ready(d) = &s.diff {
            if branch_loaded.insert((d.key.path.clone(), d.key.untracked)) {
                handle.commands.send(Command::SelectNext).unwrap();
            }
        }
    }
    assert_eq!(
        branch_rows, 4,
        "a.txt, b.txt, c.txt and new.txt against main"
    );
    assert_eq!(
        branch_loaded.len(),
        branch_rows,
        "not every branch row was loaded: {branch_loaded:?}"
    );
    handle.commands.send(Command::LoadRefs(1)).unwrap();
    handle
        .commands
        .send(Command::SetBase(Some("refs/heads/other".into())))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut saw_refs, mut picked) = (false, false);
    while Instant::now() < deadline && !(saw_refs && picked) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        saw_refs |= s
            .refs
            .as_ref()
            .is_some_and(|r| r.iter().any(|r| r == "refs/heads/other"));
        picked |= s.pick_seq == 1
            && s.base
                .as_ref()
                .is_some_and(|b| b.requested == "refs/heads/other")
            && s.pick_error.is_none();
    }
    assert!(saw_refs, "the ref list never listed refs/heads/other");
    assert!(picked, "the pick was not published");
    handle
        .commands
        .send(Command::SetScope(Scope::Worktree))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut back = false;
    let mut head = None;
    while Instant::now() < deadline && !(back && head.is_some()) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        back = s.scope == Scope::Worktree && s.files.len() == 3;
        if back {
            head = s.head.clone();
        }
    }
    assert!(back, "worktree scope did not come back");
    // Mark a commit that is not the head, and open the picker again, without widening the
    // allow-list. Older on purpose: marking the head short-circuits classification before any
    // git call, and classification is where 8.3's two commands run.
    assert!(head.is_some(), "nothing was markable in worktree scope");
    let older = git(&real, p, &["rev-parse", "HEAD~1"]).trim().to_string();
    handle.commands.send(Command::MarkReviewed(older)).unwrap();
    handle.commands.send(Command::LoadRefs(2)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut marked, mut refs) = (false, false);
    while Instant::now() < deadline && !(marked && refs) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        marked |= s.mark_seq == 1 && s.mark_error.is_none();
        refs |= s.refs_seq == 2 && s.quick.is_some();
    }
    assert!(marked, "the mark was never answered");
    assert!(refs, "the quick rows never arrived for this opening");
    // Classification runs only in branch scope. Without this switch the allow-list below would
    // prove nothing about the commands this feature added.
    handle
        .commands
        .send(Command::SetScope(Scope::Branch))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut classified = false;
    while Instant::now() < deadline && !classified {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        classified = s.scope == Scope::Branch
            && s.mark
                .as_ref()
                .is_some_and(|mark| mark.classified_at.is_some());
    }
    assert!(classified, "the mark was never classified in branch scope");
    // Now the harness switches HEAD underneath the engine, through the real git.
    git(&real, p, &["symbolic-ref", "HEAD", "refs/heads/other"]);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut saw_other = false;
    while Instant::now() < deadline && !saw_other {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        saw_other = matches!(&s.repo, herdr_hunks::engine::RepoState::Repo { branch: Some(b), .. } if b == "other");
    }
    assert!(saw_other, "the branch switch never reached the engine");
    std::thread::sleep(Duration::from_millis(400)); // a few D2 ticks
    handle.commands.send(Command::Shutdown).unwrap();
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
    assert!(!recorded.is_empty(), "wrapper recorded nothing");
    for entry in recorded.lines() {
        let fields: Vec<&str> = entry.split('\t').collect();
        // the subcommand is the first argument after any `-C <dir>` pairs
        let args: Vec<&str> = fields[0].split_whitespace().collect();
        let mut i = 0;
        while args.get(i) == Some(&"-C") {
            i += 2;
        }
        let sub = args.get(i).copied().unwrap_or("");
        assert!(
            ALLOWED.contains(&sub),
            "unexpected git subcommand `{sub}` in `{}`",
            fields[0]
        );
        assert_eq!(
            &fields[1..],
            ["0", "1", "1"],
            "D3 variables missing in `{}`",
            fields[0]
        );
    }
    let subs: std::collections::BTreeSet<&str> = recorded
        .lines()
        .map(|entry| {
            let args: Vec<&str> = entry
                .split('\t')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            let mut i = 0;
            while args.get(i) == Some(&"-C") {
                i += 2;
            }
            args.get(i).copied().unwrap_or("")
        })
        .collect();
    for expected in ["merge-base", "for-each-ref", "symbolic-ref", "rev-parse"] {
        assert!(subs.contains(expected), "{expected} never ran: {subs:?}");
    }
    assert!(
        recorded
            .lines()
            .any(|l| l.contains("diff ") && l.contains("--name-status -M -z --")),
        "no name-status against the merge-base"
    );
    // The two commands 8.3 adds, both reads, both with shape-validated endpoints.
    assert!(
        recorded
            .lines()
            .any(|l| l.contains("merge-base --is-ancestor ")),
        "the mark's ancestry was never asked about"
    );
    assert!(
        recorded
            .lines()
            .any(|l| l.contains("diff ") && l.contains("--name-only --no-renames -z --")),
        "the unread set was never read"
    );
    assert!(
        !recorded.lines().any(|l| l.contains("--merge-base")),
        "the merge-base must be pinned, never recomputed by git diff"
    );
    let mut written: Vec<String> = std::fs::read_dir(state.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        ["bases.json", "marks.json", "split-panes.lock"],
        "the viewer wrote something else"
    );
    let picks: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(state.path().join("bases.json")).unwrap())
            .unwrap();
    assert_eq!(
        picks.values().collect::<Vec<_>>(),
        [&"refs/heads/other".to_string()]
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
    // Writes to arbitrary absolute paths are outside what this test can see.
    // A child under a restricted mount namespace or a syscall trace is not portable to macOS CI.
    assert!(
        std::fs::read_dir(scratch.path()).unwrap().next().is_none(),
        "the viewer wrote to cwd"
    );
    for (path, existed) in home_before {
        assert_eq!(
            path.symlink_metadata().is_ok(),
            existed,
            "unexpected HOME entry: {}",
            path.display()
        );
    }
    std::env::set_current_dir(original_cwd).unwrap();
}
