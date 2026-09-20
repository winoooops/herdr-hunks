use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git")
        .success();
    assert!(ok, "git {args:?} failed");
}

fn repo_with_external_diff() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "init"]);
    git(p, &["config", "diff.external", "/bin/echo"]);
    std::fs::write(p.join("a.txt"), "one\nTWO\n").unwrap();
    std::fs::write(p.join("new.txt"), "fresh\n").unwrap();
    dir
}

#[tokio::test]
async fn external_diff_does_not_replace_the_unified_diff() {
    let dir = repo_with_external_diff();
    let cwd = dir.path().to_string_lossy().to_string();
    let tracked = crate::git::get_git_diff(cwd.clone(), "a.txt".into(), false, None)
        .await
        .unwrap();
    assert_eq!(
        tracked.file_diff.hunks.len(),
        1,
        "tracked file must parse to its real hunk"
    );
    let untracked = crate::git::get_git_diff(cwd, "new.txt".into(), false, Some(true))
        .await
        .unwrap();
    assert_eq!(
        untracked.file_diff.hunks.len(),
        1,
        "untracked file must parse to its real hunk"
    );
}
