//! Cutting a multi-section diff down to one row's sections (spec 7.2, 9.2), over bytes.

use crate::git::decode_git_patch_path;

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

/// The sections whose `diff --git` header names the row: `b/<path>`, `a/<path>` for a deletion,
/// `a/<old> b/<path>` for a rename. A pathspec matches its descendants, so `git diff -- tools`
/// also prints `tools/run`.
pub fn keep_sections(output: &[u8], path: &str, old: Option<&str>) -> Vec<u8> {
    let wanted_b = format!("b/{path}");
    let wanted_a = format!("a/{}", old.unwrap_or(path));
    let mut kept = Vec::new();
    for section in sections(output) {
        let first = section.split(|&b| b == b'\n').next().unwrap_or(&[]);
        // Only the header line is decoded, and only to match it; the section's bytes stay as read.
        let first = String::from_utf8_lossy(first);
        if let Some(header) = first.strip_prefix("diff --git ") {
            if names_row(header, &wanted_a, &wanted_b) {
                kept.extend_from_slice(section);
            }
        } else if let Some(named) = first
            .strip_prefix("diff --cc ")
            .or_else(|| first.strip_prefix("diff --combined "))
        {
            // A conflict's header carries the bare path, quoted the way git quotes it.
            if decode_git_patch_path(named.trim_end()) == path {
                kept.extend_from_slice(section);
            }
        }
    }
    kept
}

/// Each section on its own: a `diff --git `, `diff --cc ` or `diff --combined ` line at the start
/// of the text or after a newline opens one. The combined forms are a worktree conflict's output;
/// they parse to zero hunks (the frozen parser's rule) and the view reads them as unmerged.
pub fn sections(raw: &[u8]) -> Vec<&[u8]> {
    let markers: [&[u8]; 3] = [b"diff --git ", b"diff --cc ", b"diff --combined "];
    let mut starts = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let at_line_start = i == 0 || raw[i - 1] == b'\n';
        if at_line_start && markers.iter().any(|m| raw[i..].starts_with(m)) {
            starts.push(i);
        }
        i += 1;
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| &raw[start..starts.get(n + 1).copied().unwrap_or(raw.len())])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let two_paths = b"diff --git a/tools b/tools\ndeleted file mode 100644\n--- a/tools\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git a/tools/run b/tools/run\nnew file mode 100644\n--- /dev/null\n+++ b/tools/run\n@@ -0,0 +1 @@\n+y\n";
        assert_eq!(
            String::from_utf8_lossy(&keep_sections(two_paths, "tools", None)),
            "diff --git a/tools b/tools\ndeleted file mode 100644\n--- a/tools\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n"
        );
        let type_change = b"diff --git a/f b/f\ndeleted file mode 100644\n@@ -1 +0,0 @@\n-x\ndiff --git a/f b/f\nnew file mode 120000\n@@ -0,0 +1 @@\n+target\n";
        assert_eq!(keep_sections(type_change, "f", None), type_change);
        let rename = b"diff --git a/old b/new\nsimilarity index 90%\nrename from old\nrename to new\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/new/inner b/new/inner\nnew file mode 100644\n@@ -0,0 +1 @@\n+z\n";
        let inner = rename
            .windows(b"diff --git a/new/inner".len())
            .position(|w| w == b"diff --git a/new/inner")
            .unwrap();
        assert_eq!(keep_sections(rename, "new", Some("old")), &rename[..inner]);
        let quoted = b"diff --git \"a/sp\\303\\244ce\" \"b/sp\\303\\244ce\"\n@@ -1 +1 @@\n-x\n+y\n";
        assert_eq!(keep_sections(quoted, "späce", None), quoted);
        assert_eq!(keep_sections(quoted, "space", None), b"");
    }
}
