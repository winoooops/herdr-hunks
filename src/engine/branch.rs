//! Branch scope: rows and diffs against a merge-base pinned once per refresh (spec 7.2).

use std::collections::{BTreeMap, HashMap};

use crate::engine::base;
use crate::git::{
    decode_git_patch_path, get_git_diff_inner, parse_git_diff, parse_numstat, validate_file_path,
    ChangedFile, ChangedFileStatus, FileDiff, GetGitDiffResponse,
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
    // Status can precede a git add; only a deletion may coexist with an untracked row.
    files.dedup_by(|current, previous| {
        current.path == previous.path && !matches!(previous.status, ChangedFileStatus::Deleted)
    });
    Ok(BranchRows {
        files,
        rename_sources,
    })
}

/// Index just past the closing quote of a C-quoted token that starts at byte 0.
fn quoted_end(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, ch) in text.char_indices().skip(1) {
        match ch {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(i + 1),
            _ => escaped = false,
        }
    }
    None
}

/// `a/<old> b/<new>` from a `diff --git` header line, with a quoted side decoded.
/// `None` when both sides are plain: a plain path may contain spaces, so the
/// caller compares the whole header against what it expects instead.
pub fn split_header(header: &str) -> Option<(String, String)> {
    if header.starts_with('"') {
        let end = quoted_end(header)?;
        let a = decode_git_patch_path(&header[..end]);
        let b = header.get(end + 1..)?;
        let b = if b.starts_with('"') {
            decode_git_patch_path(b)
        } else {
            b.to_string()
        };
        return Some((a, b));
    }
    if header.ends_with('"') {
        let start = header.find('"')?;
        let a = header.get(..start.checked_sub(1)?)?.to_string();
        return Some((a, decode_git_patch_path(&header[start..])));
    }
    None
}

fn names_row(header: &str, wanted_a: &str, wanted_b: &str) -> bool {
    if header == format!("{wanted_a} {wanted_b}") {
        return true;
    }
    matches!(split_header(header), Some((a, b)) if a == wanted_a && b == wanted_b)
}

/// The `diff --git` sections whose header names the row: `a/<old-or-path> b/<path>`.
/// A pathspec matches its descendants, so `git diff -- tools` also prints `tools/run`.
pub fn keep_sections(output: &str, path: &str, old: Option<&str>) -> String {
    let wanted_b = format!("b/{path}");
    let wanted_a = format!("a/{}", old.unwrap_or(path));
    let mut kept = String::new();
    let mut keep = false;
    for line in output.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("diff --git ") {
            keep = names_row(header.trim_end_matches('\n'), &wanted_a, &wanted_b);
        }
        if keep {
            kept.push_str(line);
        }
    }
    kept
}

/// Each kept section on its own; a type change is two sections for one path.
fn sections(raw: &str) -> Vec<&str> {
    let starts: Vec<usize> = raw
        .match_indices("diff --git ")
        .map(|(i, _)| i)
        .filter(|&i| i == 0 || raw.as_bytes()[i - 1] == b'\n')
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| &raw[start..starts.get(n + 1).copied().unwrap_or(raw.len())])
        .collect()
}

/// A tracked row's diff against the pinned merge-base, parsed section by section by the frozen parser.
pub(crate) async fn diff(
    toplevel: &str,
    merge_base: &str,
    path: &str,
    old: Option<&str>,
) -> Result<GetGitDiffResponse, String> {
    validate_file_path(path)?;
    if let Some(old) = old {
        validate_file_path(old)?;
    }
    let mut args = vec![
        "diff",
        merge_base,
        "--no-color",
        "--no-ext-diff",
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
    let raw_diff = keep_sections(&String::from_utf8_lossy(&output.stdout), path, old);
    let mut file_diff = FileDiff {
        file_path: path.to_string(),
        old_path: None,
        new_path: None,
        hunks: Vec::new(),
    };
    for section in sections(&raw_diff) {
        let parsed = parse_git_diff(section, path);
        file_diff.old_path = file_diff.old_path.or(parsed.old_path);
        file_diff.new_path = file_diff.new_path.or(parsed.new_path);
        file_diff.hunks.extend(parsed.hunks);
    }
    Ok(GetGitDiffResponse {
        file_diff,
        old_text: String::new(),
        new_text: String::new(),
        raw_diff,
        repo_root: toplevel.to_string(),
    })
}

/// An untracked row in branch scope uses the Phase 1 path: `--no-index` against `/dev/null`.
pub(crate) async fn untracked_diff(
    cwd: String,
    path: String,
) -> Result<GetGitDiffResponse, String> {
    get_git_diff_inner(cwd, path, false, Some(true)).await
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn headers_split_plain_quoted_and_mixed_sides() {
        assert_eq!(
            split_header("a/x y b/x y"),
            None,
            "ambiguous without quoting"
        );
        assert_eq!(
            split_header(r#""a/sp\303\244ce" "b/sp\303\244ce""#),
            Some(("a/späce".into(), "b/späce".into()))
        );
        assert_eq!(
            split_header(r#""a/q\"uote" b/plain"#),
            Some(("a/q\"uote".into(), "b/plain".into()))
        );
        assert_eq!(
            split_header(r#"a/plain "b/tab\there""#),
            Some(("a/plain".into(), "b/tab\there".into()))
        );
    }

    #[test]
    fn only_sections_naming_the_row_are_kept_and_type_changes_keep_both() {
        let two_paths = "diff --git a/tools b/tools\ndeleted file mode 100644\n--- a/tools\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git a/tools/run b/tools/run\nnew file mode 100644\n--- /dev/null\n+++ b/tools/run\n@@ -0,0 +1 @@\n+y\n";
        assert_eq!(
            keep_sections(two_paths, "tools", None),
            "diff --git a/tools b/tools\ndeleted file mode 100644\n--- a/tools\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n"
        );
        let type_change = "diff --git a/f b/f\ndeleted file mode 100644\n@@ -1 +0,0 @@\n-x\ndiff --git a/f b/f\nnew file mode 120000\n@@ -0,0 +1 @@\n+target\n";
        assert_eq!(keep_sections(type_change, "f", None), type_change);
        let rename = "diff --git a/old b/new\nsimilarity index 90%\nrename from old\nrename to new\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/new/inner b/new/inner\nnew file mode 100644\n@@ -0,0 +1 @@\n+z\n";
        assert_eq!(
            keep_sections(rename, "new", Some("old")),
            &rename[..rename.find("diff --git a/new/inner").unwrap()]
        );
        let quoted = "diff --git \"a/sp\\303\\244ce\" \"b/sp\\303\\244ce\"\n@@ -1 +1 @@\n-x\n+y\n";
        assert_eq!(keep_sections(quoted, "späce", None), quoted);
        assert_eq!(keep_sections(quoted, "space", None), "");
    }
}
