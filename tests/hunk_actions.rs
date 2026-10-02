//! Spec 9.6's second guarantee: every `apply` the viewer runs is the form of a confirmed action,
//! the refs never change, each form changes exactly the side the row names, and every refusal
//! decided before git records no `apply` at all.
use herdr_hunks::engine::actions::{NOTICE_BINARY, NOTICE_CHANGED, NOTICE_SCOPE};
use herdr_hunks::engine::{
    init_process_env, spawn, Action, ActionKind, Command, DiffState, FileKey, Scope, SessionConfig,
    Snapshot,
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The reads of spec 7.6 plus the one mutating subcommand of 9.6.
const ALLOWED: [&str; 11] = [
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
    "apply",
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

struct Session {
    handle: herdr_hunks::engine::EngineHandle,
    log: PathBuf,
}

impl Session {
    fn recv(&self, what: &str, pred: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last = None;
        while Instant::now() < deadline {
            if let Ok(s) = self
                .handle
                .snapshots
                .recv_timeout(Duration::from_millis(200))
            {
                if pred(&s) {
                    return s;
                }
                last = Some(s);
            }
        }
        panic!("timed out waiting for {what}; last = {last:?}");
    }

    /// A `Refresh` first, so the listing is republished even when its last snapshot was consumed;
    /// then a `Select` for a listed key, answered by a settled `Ready` diff for it. The refresh's
    /// own reload can publish the key first and the `Select` then replaces that Arc, so the
    /// engine's last word is taken: the snapshot that still stands once nothing new has arrived.
    fn select(&self, path: &str, staged: bool, untracked: bool) -> Arc<Snapshot> {
        let key = FileKey {
            path: path.into(),
            staged,
            untracked,
        };
        self.handle.commands.send(Command::Refresh).unwrap();
        self.recv(&format!("{key:?} listed"), |s| {
            s.files.iter().any(|f| FileKey::of(f) == key)
        });
        self.handle
            .commands
            .send(Command::Select(key.clone()))
            .unwrap();
        let settled =
            |s: &Snapshot| !s.refreshing && matches!(&s.diff, DiffState::Ready(d) if d.key == key);
        let mut last = self.recv(&format!("{key:?} ready"), settled);
        loop {
            std::thread::sleep(Duration::from_millis(150));
            let mut newer = None;
            while let Ok(s) = self.handle.snapshots.try_recv() {
                newer = Some(s);
            }
            match newer {
                None => return last,
                Some(s) if settled(&s) => last = s,
                Some(_) => last = self.recv(&format!("{key:?} ready again"), settled),
            }
        }
    }

    /// The `apply` argv lines recorded after line `before`.
    fn applies_since(&self, before: usize) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .skip(before)
            .map(|l| l.split('\t').next().unwrap_or("").to_string())
            .filter(|l| subcommand(l) == "apply")
            .collect()
    }

    fn log_lines(&self) -> usize {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .count()
    }

    /// Sends the action against the published diff; returns the answer and the `apply` lines it caused.
    fn act(
        &self,
        s: &Snapshot,
        kind: ActionKind,
        hunk: Option<usize>,
    ) -> (Arc<Snapshot>, Vec<String>) {
        let diff = match &s.diff {
            DiffState::Ready(d) => d.clone(),
            _ => panic!("no ready diff"),
        };
        let before = self.log_lines();
        let seq = s.action_seq + 1;
        self.handle
            .commands
            .send(Command::Act(Action { kind, diff, hunk }))
            .unwrap();
        let answer = self.recv("the answer", |n| n.action_seq == seq);
        (answer, self.applies_since(before))
    }
}

fn subcommand(argv: &str) -> &str {
    let args: Vec<&str> = argv.split_whitespace().collect();
    let mut i = 0;
    while matches!(args.get(i), Some(&"-C") | Some(&"-c")) {
        i += 2;
    }
    args.get(i).copied().unwrap_or("")
}

fn arc_of(s: &Snapshot) -> Arc<herdr_hunks::engine::LoadedDiff> {
    match &s.diff {
        DiffState::Ready(d) => d.clone(),
        _ => unreachable!("no ready diff"),
    }
}

/// The fixture of spec 9.8 item 4 and the refusal cases of 9.6, plus the nested untracked file of the
/// review focus. Every commit precedes every staged change, so nothing staged is committed away.
fn fixture(real: &Path, p: &Path) {
    git(real, p, &["init", "-q", "-b", "main"]);
    git(real, p, &["config", "user.email", "t@example.com"]);
    git(real, p, &["config", "user.name", "t"]);
    let ctx = "c1\nc2\nc3\nc4\nc5\nc6\nc7\n";
    // Baseline.
    std::fs::write(
        p.join("mm.txt"),
        format!("alpha\n{ctx}beta\n{ctx}gamma\n{ctx}delta\n"),
    )
    .unwrap();
    std::fs::write(p.join("md.txt"), "m\n").unwrap();
    std::fs::write(p.join("bin.dat"), [0u8, 1, 2]).unwrap();
    std::fs::write(p.join("tools"), "x\n").unwrap();
    std::fs::write(p.join("cc.txt"), format!("one\n{ctx}two\n")).unwrap();
    git(real, p, &["add", "-A"]);
    git(real, p, &["commit", "-q", "-m", "init"]);
    let head = git(real, p, &["rev-parse", "HEAD"]).trim().to_string();
    // Staged changes.
    std::fs::write(
        p.join("mm.txt"),
        format!("ALPHA\n{ctx}BETA\n{ctx}gamma\n{ctx}delta\n"),
    )
    .unwrap();
    git(real, p, &["add", "mm.txt"]);
    std::fs::write(p.join("md.txt"), "M\n").unwrap();
    git(real, p, &["add", "md.txt"]);
    git(
        real,
        p,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{head},sub"),
        ],
    );
    // An empty directory stands in for the submodule's checkout, so the gitlink is `A  sub`.
    std::fs::create_dir(p.join("sub")).unwrap();
    git(real, p, &["rm", "-q", "tools"]);
    std::fs::create_dir(p.join("tools")).unwrap();
    std::fs::write(p.join("tools/run"), "r\n").unwrap();
    git(real, p, &["add", "tools/run"]);
    // Working-tree changes.
    std::fs::write(
        p.join("mm.txt"),
        format!("ALPHA\n{ctx}BETA\n{ctx}GAMMA\n{ctx}DELTA\n"),
    )
    .unwrap();
    std::fs::remove_file(p.join("md.txt")).unwrap();
    std::fs::write(p.join("bin.dat"), [0u8, 1, 3]).unwrap();
    std::fs::write(p.join("cc.txt"), format!("ONE\n{ctx}TWO\n")).unwrap();
    std::fs::create_dir_all(p.join("newdir/deep")).unwrap();
    std::fs::write(p.join("newdir/deep/u.txt"), "u\n").unwrap();
    std::fs::write(p.join("u2.txt"), "two\n").unwrap();
    let big: String = (0..200_001).map(|i| format!("{i}\n")).collect();
    std::fs::write(p.join("big.txt"), big).unwrap();
    // The starting state every case below counts on.
    let status = git(
        real,
        p,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    );
    for line in [
        "MM mm.txt",
        "MD md.txt",
        " M bin.dat",
        "A  sub",
        "D  tools",
        "A  tools/run",
        " M cc.txt",
        "?? newdir/deep/u.txt",
        "?? u2.txt",
        "?? big.txt",
    ] {
        assert!(
            status.lines().any(|l| l == line),
            "fixture lacks `{line}`:\n{status}"
        );
    }
}

#[test]
fn every_action_runs_exactly_its_apply_forms_and_changes_exactly_what_the_row_shows() {
    let real = real_git();
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture(&real, p);
    // recording wrapper, first in PATH (the readonly test's, verbatim)
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
    let refs_before = git(&real, p, &["for-each-ref"]);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut config = SessionConfig::production(p.to_path_buf());
    config.poll_interval = Duration::from_secs(3600); // the helpers refresh; the real watcher still runs
    config.state_dir = Some(state.path().to_path_buf());
    let session = Session {
        handle: spawn(rt.handle(), config),
        log: log.clone(),
    };
    let top = p.canonicalize().unwrap();
    let top = top.to_string_lossy();
    let form = |flags: &str| {
        format!("-c apply.ignoreWhitespace=no -C {top} apply --whitespace=nowarn{flags}")
    };

    // After every action: the refs are untouched, and the side the form does not name is untouched.
    let refs_same = |what: &str| {
        assert_eq!(
            git(&real, p, &["for-each-ref"]),
            refs_before,
            "refs changed after {what}"
        )
    };

    // 1. Stage one unstaged hunk: one --cached apply; the index gains GAMMA only, the worktree is untouched.
    let s = session.select("mm.txt", false, false);
    let index_before = std::fs::read(p.join(".git/index")).unwrap();
    let tree_before = tree_hash(p);
    let (a, applies) = session.act(&s, ActionKind::Stage, Some(0));
    assert_eq!((a.action_error.as_deref(), a.action_applied), (None, true));
    assert_eq!(applies, [form(" --cached")]);
    assert_ne!(
        std::fs::read(p.join(".git/index")).unwrap(),
        index_before,
        "the index changed"
    );
    assert_eq!(
        tree_hash(p),
        tree_before,
        "a --cached form left the worktree alone"
    );
    let cached = git(&real, p, &["diff", "--cached", "--", "mm.txt"]);
    assert!(
        cached.contains("+GAMMA") && !cached.contains("+DELTA"),
        "{cached}"
    );
    assert_eq!(
        git(&real, p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(),
        "MM mm.txt"
    );
    refs_same("stage");

    // 2. Unstage it again from the staged row (its third hunk): one --cached -R apply; the worktree is untouched.
    let s = session.select("mm.txt", true, false);
    assert!(matches!(&s.diff, DiffState::Ready(d) if d.file_diff.hunks.len() == 3));
    let tree_before = tree_hash(p);
    let (a, applies) = session.act(&s, ActionKind::Stage, Some(2));
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" --cached -R")]);
    assert_eq!(tree_hash(p), tree_before);
    assert!(!git(&real, p, &["diff", "--cached", "--", "mm.txt"]).contains("+GAMMA"));
    refs_same("unstage");

    // 3. Discard both unstaged hunks: two -R applies; the index is untouched, the worktree loses GAMMA then DELTA.
    // The index is compared by what it holds (mode, object, stage, path): git's own `diff` refreshes
    // the stat data of a content-clean entry even under GIT_OPTIONAL_LOCKS=0, which changes its bytes.
    let index_entries = || git(&real, p, &["ls-files", "-s"]);
    let s = session.select("mm.txt", false, false);
    assert!(matches!(&s.diff, DiffState::Ready(d) if d.file_diff.hunks.len() == 2));
    let index_before = index_entries();
    let (a, applies) = session.act(&s, ActionKind::Discard, Some(0));
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" -R")]);
    assert_eq!(
        index_entries(),
        index_before,
        "a worktree form left the index alone"
    );
    assert!(std::fs::read_to_string(p.join("mm.txt"))
        .unwrap()
        .contains("gamma\n"));
    let s = session.select("mm.txt", false, false);
    assert!(matches!(&s.diff, DiffState::Ready(d) if d.file_diff.hunks.len() == 1));
    let (a, applies) = session.act(&s, ActionKind::Discard, Some(0));
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" -R")]);
    assert_eq!(index_entries(), index_before);
    assert_eq!(
        git(&real, p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(),
        "M  mm.txt"
    );
    refs_same("discard");

    // 4. D on the staged row of the now-clean file: one --index -R apply; both sides return to HEAD.
    let s = session.select("mm.txt", true, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" --index -R")]);
    assert_eq!(
        git(&real, p, &["status", "--porcelain=v1", "--", "mm.txt"]).trim(),
        ""
    );
    refs_same("discard file");

    // 5. MD: refused by the engine before git; the file stays absent; no apply recorded.
    let s = session.select("md.txt", true, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(
        a.action_error.as_deref(),
        Some("md.txt: does not match index")
    );
    assert!(applies.is_empty() && !a.action_applied && !p.join("md.txt").exists());

    // 6. A binary row: refused, no apply.
    let s = session.select("bin.dat", false, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error.as_deref(), Some(NOTICE_BINARY));
    assert!(applies.is_empty());

    // 7. Branch scope: refused, no apply, nothing changes.
    session
        .handle
        .commands
        .send(Command::SetScope(Scope::Branch))
        .unwrap();
    let s = session.recv("branch rows", |s| {
        s.scope == Scope::Branch && matches!(s.diff, DiffState::Ready(_)) && !s.refreshing
    });
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error.as_deref(), Some(NOTICE_SCOPE));
    assert!(applies.is_empty());
    session
        .handle
        .commands
        .send(Command::SetScope(Scope::Worktree))
        .unwrap();

    // 8. A stale Arc: refused, no apply.
    session.recv("worktree back", |s| {
        s.scope == Scope::Worktree && !s.refreshing
    });
    let s = session.select("md.txt", true, false);
    let stale = Action {
        kind: ActionKind::Stage,
        diff: Arc::new((*arc_of(&s)).clone()),
        hunk: None,
    };
    let before = session.log_lines();
    session.handle.commands.send(Command::Act(stale)).unwrap();
    let a = session.recv("stale answer", |n| n.action_seq == s.action_seq + 1);
    assert_eq!(a.action_error.as_deref(), Some(NOTICE_CHANGED));
    assert!(session.applies_since(before).is_empty());

    // 9. Delete the nested untracked file: one -R apply; the file goes, the row goes, the index is untouched.
    let s = session.select("newdir/deep/u.txt", false, true);
    let index_before = index_entries();
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" -R")]);
    assert!(!p.join("newdir/deep/u.txt").exists());
    assert_eq!(index_entries(), index_before);
    let s = session.recv("row gone", |s| {
        !s.files.iter().any(|f| f.path == "newdir/deep/u.txt") && !s.refreshing
    });
    assert!(s.files.iter().any(|f| Some(FileKey::of(f)) == s.selected));
    refs_same("delete");

    // 10. Stage an untracked file: one --cached apply; the worktree is untouched.
    let s = session.select("u2.txt", false, true);
    let tree_before = tree_hash(p);
    let (a, applies) = session.act(&s, ActionKind::Stage, None);
    assert_eq!(a.action_error, None);
    assert_eq!(applies, [form(" --cached")]);
    assert_eq!(tree_hash(p), tree_before);
    assert_eq!(
        git(&real, p, &["status", "--porcelain=v1", "--", "u2.txt"]).trim(),
        "A  u2.txt"
    );
    refs_same("stage untracked");

    // 11-16. The remaining pre-git refusals of spec 9.2 and 9.4: each answers, and none records an apply.
    let s = session.select("big.txt", false, true);
    let (a, applies) = session.act(&s, ActionKind::Stage, None);
    assert_eq!(
        (a.action_error.as_deref(), applies.len()),
        (Some(herdr_hunks::engine::actions::NOTICE_CUT), 0)
    );
    let s = session.select("sub", true, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(
        (a.action_error.as_deref(), applies.len()),
        (Some(herdr_hunks::engine::actions::NOTICE_SUBMODULE), 0)
    );
    let s = session.select("cc.txt", false, false);
    let (a, applies) = session.act(&s, ActionKind::Stage, Some(99));
    assert_eq!(
        (a.action_error.as_deref(), applies.len()),
        (Some(herdr_hunks::engine::actions::NOTICE_NO_HUNK), 0)
    );
    let s = session.select("tools", true, false);
    let (a, applies) = session.act(&s, ActionKind::DiscardFile, None);
    assert_eq!(
        (a.action_error.as_deref(), applies.len()),
        (Some(herdr_hunks::engine::actions::NOTICE_NOT_A_FILE), 0)
    );
    // An edit after the frame: the real watcher may republish first (identity) or not (pre-image);
    // either guard answers `the diff changed` and neither runs git.
    let s = session.select("cc.txt", false, false);
    std::fs::write(
        p.join("cc.txt"),
        "ONE\nc1\nc2\nc3\nc4\nc5\nc6\nc7\nTWO\nthree\n",
    )
    .unwrap();
    let (a, applies) = session.act(&s, ActionKind::Discard, Some(0));
    assert_eq!(
        (a.action_error.as_deref(), applies.len()),
        (Some(NOTICE_CHANGED), 0)
    );
    assert!(std::fs::read_to_string(p.join("cc.txt"))
        .unwrap()
        .starts_with("ONE\n"));
    // Two keys back to back: one form runs, the other is refused as running.
    let s = session.select("cc.txt", false, false);
    let diff = arc_of(&s);
    let before = session.log_lines();
    session
        .handle
        .commands
        .send(Command::Act(Action {
            kind: ActionKind::Stage,
            diff: diff.clone(),
            hunk: Some(0),
        }))
        .unwrap();
    session
        .handle
        .commands
        .send(Command::Act(Action {
            kind: ActionKind::Stage,
            diff,
            hunk: Some(1),
        }))
        .unwrap();
    let both = session.recv("both answered", |n| n.action_seq == s.action_seq + 2);
    assert_eq!(
        both.action_error, None,
        "the queued one is answered last, and it applied"
    );
    assert_eq!(session.applies_since(before), [form(" --cached")]);
    let cached = git(&real, p, &["diff", "--cached", "--", "cc.txt"]);
    assert!(
        cached.contains("+ONE") && !cached.contains("+TWO"),
        "{cached}"
    );
    refs_same("concurrent pair");

    session.handle.commands.send(Command::Shutdown).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let recorded = std::fs::read_to_string(&log).unwrap();
    for entry in recorded.lines() {
        let fields: Vec<&str> = entry.split('\t').collect();
        let sub = subcommand(fields[0]);
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
    // Every apply is a confirmed action's form: exactly these eight ran, in this order.
    let applies: Vec<&str> = recorded
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .filter(|l| subcommand(l) == "apply")
        .collect();
    let expected = [
        " --cached",
        " --cached -R",
        " -R",
        " -R",
        " --index -R",
        " -R",
        " --cached",
        " --cached",
    ]
    .map(form);
    assert_eq!(
        applies,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        git(&real, p, &["for-each-ref"]),
        refs_before,
        "refs changed"
    );
    let mut written: Vec<String> = std::fs::read_dir(state.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert!(
        written
            .iter()
            .all(|f| ["bases.json", "marks.json", "split-panes.lock"].contains(&f.as_str())),
        "the viewer wrote something else: {written:?}"
    );
}
