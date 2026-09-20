use herdr_hunks::engine::{init_process_env, spawn, Command, DiffState, SessionConfig};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::time::{Duration, Instant};

const ALLOWED: [&str; 8] = [
    "--version",
    "rev-parse",
    "status",
    "diff",
    "ls-files",
    "show",
    "cat-file",
    "symbolic-ref",
];

fn real_git() -> PathBuf {
    let out = Proc::new("sh")
        .arg("-c")
        .arg("command -v git")
        .output()
        .unwrap();
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

fn git(real: &Path, dir: &Path, args: &[&str]) -> String {
    let out = Proc::new(real)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).to_string()
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
    git(&real, p, &["branch", "other"]); // same commit, so switching HEAD touches neither index nor worktree
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
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_millis(100);
    let handle = spawn(rt.handle(), config);
    // Load every row, then switch branches underneath the engine.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut loaded = std::collections::BTreeSet::new();
    let mut rows = 0usize;
    let mut switched = false;
    let mut saw_other = false;
    while Instant::now() < deadline && !(rows > 0 && loaded.len() == rows && saw_other) {
        let Ok(s) = handle.snapshots.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        rows = s.files.len();
        saw_other |= matches!(&s.repo, herdr_hunks::engine::RepoState::Repo { branch: Some(b), .. } if b == "other");
        if let DiffState::Ready(d) = &s.diff {
            if loaded.insert((d.key.path.clone(), d.key.staged)) {
                handle.commands.send(Command::SelectNext).unwrap();
                handle.commands.send(Command::Refresh).unwrap();
            }
        }
        if rows > 0 && loaded.len() == rows && !switched {
            switched = true;
            git(&real, p, &["symbolic-ref", "HEAD", "refs/heads/other"]); // the harness, through the real git
        }
    }
    assert_eq!(
        rows, 3,
        "expected a staged, an unstaged and an untracked row"
    );
    assert_eq!(loaded.len(), rows, "not every row was loaded: {loaded:?}");
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
}
