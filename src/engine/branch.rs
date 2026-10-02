//! Branch scope: rows and diffs against a merge-base pinned once per refresh (spec 7.2).

use std::collections::{BTreeMap, HashMap};

use crate::engine::base;
use crate::engine::sections::{keep_sections, sections};
use crate::git::{
    parse_git_diff, parse_numstat, validate_file_path, ChangedFile, ChangedFileStatus, FileDiff,
    GetGitDiffResponse,
};

/// One `--name-status -z` record; `old` is set for `R` and `C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameStatus {
    pub status: char,
    pub old: Option<String>,
    pub path: String,
}

/// `<status>\0<path>\0`, or `<status>\0<old>\0<new>\0` for renames and copies (the status may carry a score).
pub fn parse_name_status(output: &[u8]) -> Vec<NameStatus> {
    let mut out = Vec::new();
    let mut fields = output
        .split(|&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(status) = fields.next() {
        let Some(code) = status.chars().next() else {
            break;
        };
        let first = fields.next().unwrap_or_default();
        if matches!(code, 'R' | 'C') {
            let path = fields.next().unwrap_or_default();
            out.push(NameStatus {
                status: code,
                old: Some(first),
                path,
            });
        } else {
            out.push(NameStatus {
                status: code,
                old: None,
                path: first,
            });
        }
    }
    out
}

fn status_of(code: char) -> ChangedFileStatus {
    match code {
        'A' | 'C' => ChangedFileStatus::Added,
        'D' => ChangedFileStatus::Deleted,
        'R' => ChangedFileStatus::Renamed,
        _ => ChangedFileStatus::Modified,
    }
}

pub struct BranchRows {
    pub files: Vec<ChangedFile>,
    pub rename_sources: BTreeMap<String, String>,
}

/// name-status against the pinned merge-base, plus the untracked rows of the same refresh, sorted by path.
pub(crate) async fn rows(
    toplevel: &str,
    merge_base: &str,
    untracked: Vec<ChangedFile>,
) -> Result<BranchRows, String> {
    let names = base::git(
        toplevel,
        &["diff", merge_base, "--name-status", "-M", "-z", "--"],
    )
    .await?;
    if !names.status.success() {
        return Err(format!(
            "git diff --name-status failed: {}",
            String::from_utf8_lossy(&names.stderr).trim()
        ));
    }
    let stats = base::git(
        toplevel,
        &["diff", merge_base, "--numstat", "-M", "-z", "--"],
    )
    .await?;
    let counts: HashMap<String, (u32, u32)> = if stats.status.success() {
        parse_numstat(&stats.stdout)
    } else {
        HashMap::new()
    };
    let mut rename_sources = BTreeMap::new();
    let mut files: Vec<ChangedFile> = parse_name_status(&names.stdout)
        .into_iter()
        .map(|record| {
            let status = status_of(record.status);
            if let (ChangedFileStatus::Renamed, Some(old)) = (&status, &record.old) {
                rename_sources.insert(record.path.clone(), old.clone());
            }
            let (insertions, deletions) = match counts.get(&record.path) {
                Some(&(added, removed)) => (Some(added), Some(removed)),
                None => (None, None),
            };
            ChangedFile {
                path: record.path,
                status,
                staged: false,
                insertions,
                deletions,
            }
        })
        .collect();
    files.extend(
        untracked
            .into_iter()
            .filter(|f| matches!(f.status, ChangedFileStatus::Untracked)),
    );
    // A path deleted on the branch and recreated untracked is two rows; the untracked one comes second.
    files.sort_by(|a, b| {
        a.path.cmp(&b.path).then_with(|| {
            matches!(a.status, ChangedFileStatus::Untracked)
                .cmp(&matches!(b.status, ChangedFileStatus::Untracked))
        })
    });
    // Rows must have distinct keys, or selection stalls. Status can precede a git add, and two
    // paths that differ only in undecodable bytes display alike: both collapse to the first row.
    // Only the deleted-plus-recreated-untracked pair keeps two rows, with distinct keys.
    files.dedup_by(|current, previous| {
        current.path == previous.path
            && !(matches!(previous.status, ChangedFileStatus::Deleted)
                && matches!(current.status, ChangedFileStatus::Untracked))
    });
    Ok(BranchRows {
        files,
        rename_sources,
    })
}

/// A tracked row's diff against the pinned merge-base, parsed section by section by the frozen parser.
pub(crate) async fn diff(
    toplevel: &str,
    merge_base: &str,
    path: &str,
    old: Option<&str>,
) -> Result<(GetGitDiffResponse, Vec<u8>), String> {
    validate_file_path(path)?;
    if let Some(old) = old {
        validate_file_path(old)?;
    }
    let mut args = vec![
        "diff",
        merge_base,
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "-U3",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    if old.is_some() {
        args.push("-M");
    }
    args.push("--");
    if let Some(old) = old {
        args.push(old);
    }
    args.push(path);
    let output = base::git(toplevel, &args).await?;
    if !output.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let patch = keep_sections(&output.stdout, path, old);
    let raw_diff = String::from_utf8_lossy(&patch).into_owned();
    let mut file_diff = FileDiff {
        file_path: path.to_string(),
        old_path: None,
        new_path: None,
        hunks: Vec::new(),
    };
    for section in sections(&patch) {
        let parsed = parse_git_diff(&String::from_utf8_lossy(section), path);
        file_diff.old_path = file_diff.old_path.or(parsed.old_path);
        file_diff.new_path = file_diff.new_path.or(parsed.new_path);
        file_diff.hunks.extend(parsed.hunks);
    }
    Ok((
        GetGitDiffResponse {
            file_diff,
            old_text: String::new(),
            new_text: String::new(),
            raw_diff,
            repo_root: toplevel.to_string(),
        },
        patch,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two paths that differ only in undecodable bytes display alike; they become one row so
    /// every key stays unique (the frozen `ChangedFile.path` is a `String`, so the raw bytes
    /// cannot be kept). The limit is documented in the README.
    #[cfg(target_os = "linux")]
    #[test]
    fn paths_that_are_not_utf8_are_shown_lossily_as_one_row() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::process::Command as Proc;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let git = |args: &[&str]| {
            assert!(Proc::new("git")
                .arg("-C")
                .arg(p)
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "a\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        git(&["switch", "-q", "-c", "feat"]);
        std::fs::write(p.join(OsStr::from_bytes(b"n\xfe.txt")), "one\n").unwrap();
        std::fs::write(p.join(OsStr::from_bytes(b"n\xff.txt")), "two\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "odd names"]);
        let toplevel = p.canonicalize().unwrap().to_string_lossy().into_owned();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let merge_base = rt
            .block_on(base::merge_base_of(&toplevel, "HEAD", "refs/heads/main"))
            .unwrap();
        let rows = rt
            .block_on(rows(&toplevel, &merge_base, Vec::new()))
            .unwrap();
        let names: Vec<&str> = rows.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            names,
            ["n\u{fffd}.txt"],
            "one displayed path, one row, one key"
        );
        assert!(matches!(rows.files[0].status, ChangedFileStatus::Added));
        // An untracked twin of a displayed path collapses too, except onto a deletion.
        let twin = ChangedFile {
            path: "n\u{fffd}.txt".into(),
            status: ChangedFileStatus::Untracked,
            staged: false,
            insertions: None,
            deletions: None,
        };
        let rows = rt
            .block_on(super::rows(&toplevel, &merge_base, vec![twin]))
            .unwrap();
        assert_eq!(rows.files.len(), 1);
    }

    #[test]
    fn name_status_records_carry_the_old_path_for_renames_and_copies() {
        let out = b"M\0a.txt\0R100\0old.txt\0new.txt\0A\0d.txt\0T\0link\0C75\0src\0copy\0D\0gone\0";
        let records = parse_name_status(out);
        let brief: Vec<(char, Option<&str>, &str)> = records
            .iter()
            .map(|r| (r.status, r.old.as_deref(), r.path.as_str()))
            .collect();
        assert_eq!(
            brief,
            vec![
                ('M', None, "a.txt"),
                ('R', Some("old.txt"), "new.txt"),
                ('A', None, "d.txt"),
                ('T', None, "link"),
                ('C', Some("src"), "copy"),
                ('D', None, "gone"),
            ]
        );
        assert!(parse_name_status(b"").is_empty());
    }
}
