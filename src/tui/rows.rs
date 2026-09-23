//! Display rows for one LoadedDiff in one view mode. Built once per diff or mode change.
use std::collections::HashMap;

use crate::engine::nav::{Side, ViewMode};
use crate::engine::LoadedDiff;
use crate::git::DiffLineType;
use crate::tui::sanitize::sanitize;

#[derive(Debug, Clone)]
pub struct Cell {
    pub target: Option<usize>,
    pub no: Option<u32>,
    pub sign: char,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum Row {
    FileHeader {
        path: String,
    },
    HunkHeader {
        hunk_index: usize,
        text: String,
    },
    Gap {
        lines: u32,
    },
    Unified {
        target: usize,
        old_no: Option<u32>,
        new_no: Option<u32>,
        sign: char,
        text: String,
    },
    Split {
        left: Option<Cell>,
        right: Option<Cell>,
    },
    Truncated {
        lines: usize,
    },
}

pub struct Rows {
    pub rows: Vec<Row>,
    pub max_text_width: usize,
    /// Row index that displays each target.
    pub row_of_target: Vec<usize>,
}

pub fn build(diff: &LoadedDiff, mode: ViewMode) -> Rows {
    let index: HashMap<(usize, Side, u32), usize> = diff
        .targets
        .iter()
        .enumerate()
        .map(|(i, t)| ((t.hunk_index, t.side, t.line_number), i))
        .collect();
    let mut rows = vec![Row::FileHeader {
        path: sanitize(&diff.file_diff.file_path),
    }];
    let mut row_of_target = vec![0usize; diff.targets.len()];
    let mut previous_end = 1u32;

    for (hunk_index, hunk) in diff.file_diff.hunks.iter().enumerate() {
        if hunk.new_start > previous_end {
            rows.push(Row::Gap {
                lines: hunk.new_start - previous_end,
            });
        }
        previous_end = hunk.new_start + hunk.new_lines;
        rows.push(Row::HunkHeader {
            hunk_index,
            text: sanitize(&hunk.header),
        });

        let mut old_no = hunk.old_start;
        let mut new_no = hunk.new_start;
        let mut deletions: Vec<Cell> = Vec::new();
        let mut additions: Vec<Cell> = Vec::new();

        let flush = |rows: &mut Vec<Row>,
                     row_of_target: &mut Vec<usize>,
                     deletions: &mut Vec<Cell>,
                     additions: &mut Vec<Cell>| {
            for i in 0..deletions.len().max(additions.len()) {
                let left = deletions.get(i).cloned();
                let right = additions.get(i).cloned();
                for cell in [&left, &right].into_iter().flatten() {
                    if let Some(t) = cell.target {
                        row_of_target[t] = rows.len();
                    }
                }
                rows.push(Row::Split { left, right });
            }
            deletions.clear();
            additions.clear();
        };

        for line in &hunk.lines {
            let text = sanitize(&line.content);
            match line.line_type {
                DiffLineType::Removed => {
                    let no = line.old_line_number.unwrap_or(old_no);
                    let target = index.get(&(hunk_index, Side::Deletions, no)).copied();
                    match mode {
                        ViewMode::Unified => {
                            if let Some(t) = target {
                                row_of_target[t] = rows.len();
                                rows.push(Row::Unified {
                                    target: t,
                                    old_no: Some(no),
                                    new_no: None,
                                    sign: '-',
                                    text,
                                });
                            }
                        }
                        ViewMode::Split => deletions.push(Cell {
                            target,
                            no: Some(no),
                            sign: '-',
                            text,
                        }),
                    }
                    old_no += 1;
                }
                DiffLineType::Added => {
                    let no = line.new_line_number.unwrap_or(new_no);
                    let target = index.get(&(hunk_index, Side::Additions, no)).copied();
                    match mode {
                        ViewMode::Unified => {
                            if let Some(t) = target {
                                row_of_target[t] = rows.len();
                                rows.push(Row::Unified {
                                    target: t,
                                    old_no: None,
                                    new_no: Some(no),
                                    sign: '+',
                                    text,
                                });
                            }
                        }
                        ViewMode::Split => additions.push(Cell {
                            target,
                            no: Some(no),
                            sign: '+',
                            text,
                        }),
                    }
                    new_no += 1;
                }
                DiffLineType::Context => {
                    if mode == ViewMode::Split {
                        flush(
                            &mut rows,
                            &mut row_of_target,
                            &mut deletions,
                            &mut additions,
                        );
                    }
                    let shown_new = line.new_line_number.unwrap_or(new_no);
                    let shown_old = line.old_line_number.unwrap_or(old_no);
                    let target = index
                        .get(&(hunk_index, Side::Additions, shown_new))
                        .copied();
                    if let Some(t) = target {
                        row_of_target[t] = rows.len();
                        rows.push(match mode {
                            ViewMode::Unified => Row::Unified {
                                target: t,
                                old_no: Some(shown_old),
                                new_no: Some(shown_new),
                                sign: ' ',
                                text,
                            },
                            ViewMode::Split => Row::Split {
                                left: Some(Cell {
                                    target: None,
                                    no: Some(shown_old),
                                    sign: ' ',
                                    text: text.clone(),
                                }),
                                right: Some(Cell {
                                    target: Some(t),
                                    no: Some(shown_new),
                                    sign: ' ',
                                    text,
                                }),
                            },
                        });
                    }
                    old_no += 1;
                    new_no += 1;
                }
            }
        }
        if mode == ViewMode::Split {
            flush(
                &mut rows,
                &mut row_of_target,
                &mut deletions,
                &mut additions,
            );
        }
    }
    if diff.truncated_lines > 0 {
        rows.push(Row::Truncated {
            lines: diff.truncated_lines,
        });
    }
    let max_text_width = rows
        .iter()
        .map(|row| {
            use crate::tui::format::width;
            match row {
                Row::FileHeader { path } => width(path),
                Row::HunkHeader { text, .. } | Row::Unified { text, .. } => width(text),
                Row::Split { left, right } => [left, right]
                    .into_iter()
                    .flatten()
                    .map(|cell| width(&cell.text))
                    .max()
                    .unwrap_or(0),
                _ => 0,
            }
        })
        .max()
        .unwrap_or(0);
    Rows {
        rows,
        max_text_width,
        row_of_target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::nav::ViewMode;
    use crate::engine::{Comparison, FileKey, LoadedDiff};
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff, GetGitDiffResponse};

    type HunkSpec<'a> = (u32, u32, &'a [(char, &'a str)]);

    fn loaded(hunks: &[HunkSpec]) -> LoadedDiff {
        let hunks = hunks
            .iter()
            .map(|(old_start, new_start, lines)| DiffHunk {
                id: String::new(),
                header: format!("@@ -{old_start} +{new_start} @@"),
                old_start: *old_start,
                old_lines: lines.iter().filter(|(k, _)| *k != '+').count() as u32,
                new_start: *new_start,
                new_lines: lines.iter().filter(|(k, _)| *k != '-').count() as u32,
                lines: lines
                    .iter()
                    .map(|(k, text)| DiffLine {
                        line_type: match k {
                            '+' => DiffLineType::Added,
                            '-' => DiffLineType::Removed,
                            _ => DiffLineType::Context,
                        },
                        content: text.to_string(),
                        old_line_number: None,
                        new_line_number: None,
                    })
                    .collect(),
            })
            .collect();
        LoadedDiff::build(
            FileKey {
                path: "src/f.rs".into(),
                staged: false,
                untracked: false,
            },
            Comparison::Worktree,
            GetGitDiffResponse {
                file_diff: FileDiff {
                    file_path: "src/f.rs".into(),
                    old_path: None,
                    new_path: None,
                    hunks,
                },
                old_text: String::new(),
                new_text: String::new(),
                raw_diff: String::new(),
                repo_root: String::new(),
            },
        )
    }

    fn kinds(rows: &Rows) -> String {
        rows.rows
            .iter()
            .map(|r| match r {
                Row::FileHeader { .. } => 'F',
                Row::HunkHeader { .. } => 'H',
                Row::Gap { .. } => 'G',
                Row::Unified { sign, .. } => *sign,
                Row::Split { .. } => 'S',
                Row::Truncated { .. } => 'T',
            })
            .collect()
    }

    #[test]
    fn unified_rows_follow_the_hunk_with_a_leading_gap() {
        let d = loaded(&[(
            10,
            10,
            &[(' ', "a"), ('-', "b"), ('-', "c"), ('+', "B"), (' ', "d")],
        )]);
        let rows = build(&d, ViewMode::Unified);
        assert_eq!(kinds(&rows), "FGH --+ ");
        assert!(matches!(rows.rows[1], Row::Gap { lines: 9 }));
        // every target maps to the row that shows it
        for (target, row) in rows.row_of_target.iter().enumerate() {
            match &rows.rows[*row] {
                Row::Unified { target: t, .. } => assert_eq!(*t, target),
                other => panic!(
                    "target {target} maps to {:?}",
                    std::mem::discriminant(other)
                ),
            }
        }
    }

    #[test]
    fn a_gap_between_hunks_counts_the_unmodified_lines() {
        let d = loaded(&[(1, 1, &[('+', "x")]), (30, 31, &[('-', "y")])]);
        let rows = build(&d, ViewMode::Unified);
        assert_eq!(kinds(&rows), "FH+GH-");
        assert!(matches!(rows.rows[3], Row::Gap { lines: 29 }));
    }

    #[test]
    fn split_rows_pair_a_replacement_and_fill_the_short_side() {
        let d = loaded(&[(
            10,
            10,
            &[(' ', "a"), ('-', "b"), ('-', "c"), ('+', "B"), (' ', "d")],
        )]);
        let rows = build(&d, ViewMode::Split);
        assert_eq!(kinds(&rows), "FGHSSSS");
        match &rows.rows[4] {
            Row::Split {
                left: Some(l),
                right: Some(r),
            } => {
                assert_eq!((l.no, l.sign, l.text.as_str()), (Some(11), '-', "b"));
                assert_eq!((r.no, r.sign, r.text.as_str()), (Some(11), '+', "B"));
            }
            _ => panic!("row 4 must be a full pair"),
        }
        assert!(matches!(
            &rows.rows[5],
            Row::Split {
                left: Some(_),
                right: None
            }
        ));
    }

    #[test]
    fn a_truncated_diff_ends_with_a_truncated_row() {
        let mut d = loaded(&[(1, 1, &[('+', "x")])]);
        d.truncated_lines = 7;
        let rows = build(&d, ViewMode::Unified);
        assert!(matches!(
            rows.rows.last(),
            Some(Row::Truncated { lines: 7 })
        ));
    }

    #[test]
    fn text_is_sanitized() {
        let d = loaded(&[(1, 1, &[('+', "a\x1bb")])]);
        let rows = build(&d, ViewMode::Unified);
        match &rows.rows[2] {
            Row::Unified { text, .. } => assert_eq!(text, "a\u{241b}b"),
            _ => panic!(),
        }
    }
}
