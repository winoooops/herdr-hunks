//! Display rows for one LoadedDiff in one view mode. Built once per diff or mode change.
use std::collections::HashMap;

use crate::engine::comments::{
    Anchor, AnchorComparison, Comment, CommentState, Span as AnchorSpan,
};
use crate::engine::nav::{self, Side, ViewMode};
use crate::engine::LoadedDiff;
use crate::engine::{Comparison, FileKey, Scope, Snapshot};
use crate::git::DiffLineType;
use crate::tui::cards::{self, CardLine, MIN_WIDTH};
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
    Card {
        id: String,
        target: Option<usize>,
        line: CardLine,
    },
    Orphans {
        count: usize,
    },
    Editor {
        line: usize,
    },
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
    pub orphan_tops: Vec<(usize, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorPlace {
    pub after: EditorAnchor,
    pub editing: Option<String>,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorAnchor {
    Anchor(Anchor),
    End,
}

pub fn in_place<'a>(comments: &'a [Comment], diff: &LoadedDiff, branch: bool) -> Vec<&'a Comment> {
    comments
        .iter()
        .filter(|c| c.anchor.key == diff.key)
        .filter(|c| matches!(c.anchor.comparison, AnchorComparison::Branch { .. }) == branch)
        .collect()
}

pub fn orphans<'a>(comments: &'a [Comment], snapshot: &Snapshot) -> Vec<&'a Comment> {
    let branch = snapshot.scope == Scope::Branch;
    let listed: std::collections::HashSet<FileKey> =
        snapshot.files.iter().map(FileKey::of).collect();
    comments
        .iter()
        .filter(|c| {
            matches!(
                c.state,
                CommentState::Pending | CommentState::Unconfirmed { .. }
            )
        })
        .filter(|c| matches!(c.anchor.comparison, AnchorComparison::Branch { .. }) == branch)
        .filter(|c| !listed.contains(&c.anchor.key))
        .collect()
}

pub fn anchor_target(diff: &LoadedDiff, anchor: &Anchor) -> Option<usize> {
    match anchor.span {
        AnchorSpan::File => None,
        AnchorSpan::Line => nav::find(&diff.targets, anchor.side, anchor.line),
        AnchorSpan::Range { end } => nav::find(&diff.targets, anchor.side, end),
    }
}

pub fn attach_row(diff: &LoadedDiff, row_of_target: &[usize], anchor: &Anchor) -> Option<usize> {
    if anchor.key != diff.key
        || matches!(anchor.comparison, AnchorComparison::Branch { .. })
            != matches!(diff.comparison, Comparison::Branch { .. })
    {
        return None;
    }
    match anchor.span {
        AnchorSpan::File => Some(0),
        _ => anchor_target(diff, anchor).and_then(|t| row_of_target.get(t).copied()),
    }
}

fn append_orphans(rows: &mut Rows, orphans: &[&Comment], width: usize) {
    if !orphans.is_empty() {
        rows.rows.push(Row::Orphans {
            count: orphans.len(),
        });
        for comment in orphans {
            rows.orphan_tops.push((rows.rows.len(), comment.id.clone()));
            rows.rows
                .extend(
                    cards::lines(comment, width, true)
                        .into_iter()
                        .map(|line| Row::Card {
                            id: comment.id.clone(),
                            target: None,
                            line,
                        }),
                );
        }
    }
}

fn splice_editor(rows: &mut Rows, diff: Option<&LoadedDiff>, editor: Option<&EditorPlace>) {
    let Some(editor) = editor else { return };
    let end = rows.rows.len();
    let card = editor.editing.as_ref().and_then(|id| {
        let first = rows
            .rows
            .iter()
            .position(|r| matches!(r, Row::Card { id: card, .. } if card == id));
        let last = rows
            .rows
            .iter()
            .rposition(|r| matches!(r, Row::Card { id: card, .. } if card == id));
        first.zip(last).map(|(first, last)| first..last + 1)
    });
    let replaced = card.unwrap_or_else(|| {
        let at = match &editor.after {
            EditorAnchor::Anchor(anchor) => diff
                .and_then(|d| attach_row(d, &rows.row_of_target, anchor))
                .map_or(end, |r| r + 1),
            EditorAnchor::End => end,
        };
        at..at
    });
    let delta = editor.lines as isize - replaced.len() as isize;
    rows.rows.splice(
        replaced.clone(),
        (0..editor.lines).map(|line| Row::Editor { line }),
    );
    for row in rows
        .row_of_target
        .iter_mut()
        .chain(rows.orphan_tops.iter_mut().map(|(row, _)| row))
    {
        if *row >= replaced.end {
            *row = row.saturating_add_signed(delta);
        } else if *row >= replaced.start {
            *row = replaced.start;
        }
    }
}

pub fn orphans_only(orphans: &[&Comment], width: usize, editor: Option<&EditorPlace>) -> Rows {
    let mut rows = Rows {
        rows: Vec::new(),
        max_text_width: 0,
        row_of_target: Vec::new(),
        orphan_tops: Vec::new(),
    };
    append_orphans(&mut rows, orphans, width.max(MIN_WIDTH));
    splice_editor(&mut rows, None, editor);
    rows
}

pub fn build(
    diff: &LoadedDiff,
    mode: ViewMode,
    comments: &[&Comment],
    orphans: &[&Comment],
    card_width: usize,
    editor: Option<&EditorPlace>,
) -> Rows {
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
    let plain = rows;
    let mut after: Vec<Vec<Row>> = vec![Vec::new(); plain.len()];
    let mut orphans = orphans.to_vec();
    for comment in comments {
        if let Some(row) = attach_row(diff, &row_of_target, &comment.anchor) {
            let target = anchor_target(diff, &comment.anchor);
            after[row].extend(
                cards::lines(comment, card_width.max(MIN_WIDTH), false)
                    .into_iter()
                    .map(|line| Row::Card {
                        id: comment.id.clone(),
                        target,
                        line,
                    }),
            );
        } else if comment.is_editable()
            && comment.anchor.key == diff.key
            && matches!(comment.anchor.comparison, AnchorComparison::Branch { .. })
                == matches!(diff.comparison, Comparison::Branch { .. })
        {
            orphans.push(comment);
        }
    }
    let mut rows = Vec::with_capacity(plain.len());
    let mut remap = vec![0; plain.len()];
    for (i, row) in plain.into_iter().enumerate() {
        remap[i] = rows.len();
        rows.push(row);
        rows.append(&mut after[i]);
    }
    for row in &mut row_of_target {
        *row = remap[*row];
    }
    let mut rows = Rows {
        rows,
        max_text_width,
        row_of_target,
        orphan_tops: Vec::new(),
    };
    append_orphans(&mut rows, &orphans, card_width.max(MIN_WIDTH));
    splice_editor(&mut rows, Some(diff), editor);
    rows
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
            None,
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
            Vec::new(),
            None,
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
                Row::Card { .. } => 'C',
                Row::Orphans { .. } => 'O',
                Row::Editor { .. } => 'E',
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
        let rows = build(&d, ViewMode::Unified, &[], &[], 80, None);
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
        let rows = build(&d, ViewMode::Unified, &[], &[], 80, None);
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
        let rows = build(&d, ViewMode::Split, &[], &[], 80, None);
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
        let rows = build(&d, ViewMode::Unified, &[], &[], 80, None);
        assert!(matches!(
            rows.rows.last(),
            Some(Row::Truncated { lines: 7 })
        ));
    }

    #[test]
    fn text_is_sanitized() {
        let d = loaded(&[(1, 1, &[('+', "a\x1bb")])]);
        let rows = build(&d, ViewMode::Unified, &[], &[], 80, None);
        match &rows.rows[2] {
            Row::Unified { text, .. } => assert_eq!(text, "a\u{241b}b"),
            _ => panic!(),
        }
    }

    use crate::engine::comments::{
        Anchor, AnchorComparison, Category, Comment, CommentState, Span, Stamp,
    };
    use crate::engine::DiffState;
    use crate::tui::cards::CardLine;

    fn stamp() -> Stamp {
        Stamp {
            at: 1,
            nonce: "abc123".into(),
            item: 1,
            to: crate::engine::target::Destination::clipboard(),
        }
    }

    fn comment(id: &str, key: &FileKey, side: Side, line: u32, span: Span) -> Comment {
        Comment {
            id: id.into(),
            anchor: Anchor {
                key: key.clone(),
                side,
                line,
                span,
                comparison: AnchorComparison::Worktree,
            },
            category: Category::Bug,
            text: "needs a test".into(),
            created_at: 1,
            state: CommentState::Pending,
        }
    }
    #[test]
    fn cards_sit_under_their_lines_ranges_and_header_and_orphans_close_the_body() {
        let snap = crate::tui::state::tests::snapshot("f", "raw", &[(10, " --+ "), (40, " + ")]);
        let diff = match &snap.diff {
            DiffState::Ready(d) => d.clone(),
            _ => unreachable!(),
        };
        let line = comment("l", &diff.key, Side::Additions, 11, Span::Line);
        let range = comment("r", &diff.key, Side::Deletions, 11, Span::Range { end: 12 });
        let file = comment("f", &diff.key, Side::Additions, 0, Span::File);
        let orphan = comment(
            "o",
            &FileKey {
                path: "gone.rs".into(),
                staged: false,
                untracked: false,
            },
            Side::Additions,
            3,
            Span::Line,
        );
        let rows = build(
            &diff,
            ViewMode::Unified,
            &[&line, &range, &file],
            &[&orphan],
            60,
            None,
        );
        let kinds: Vec<String> = rows
            .rows
            .iter()
            .map(|r| match r {
                Row::FileHeader { .. } => "H".into(),
                Row::HunkHeader { .. } => "@".into(),
                Row::Unified { old_no, new_no, .. } => format!(
                    "{}/{}",
                    old_no.map_or("-".into(), |n| n.to_string()),
                    new_no.map_or("-".into(), |n| n.to_string())
                ),
                Row::Card {
                    id,
                    line: CardLine::Top { .. },
                    ..
                } => format!("[{id}"),
                Row::Card {
                    line: CardLine::Text(_),
                    ..
                } => "|".into(),
                Row::Card {
                    line: CardLine::Bottom,
                    ..
                } => "]".into(),
                Row::Orphans { count } => format!("orphans {count}"),
                _ => "?".into(),
            })
            .collect();
        // The file card follows the header; the range card the last deleted line (12); the line card line 11.
        assert_eq!(kinds[..3], ["H", "[f", "|"]);
        assert!(
            kinds
                .windows(4)
                .any(|w| w == ["10/10", "11/-", "12/-", "[r"]),
            "{kinds:?}"
        );
        let line_pos = kinds.iter().position(|k| k == "[l").unwrap();
        assert_eq!(kinds[line_pos - 1], "-/11");
        let tail = &kinds[kinds.len() - 4..];
        assert_eq!(tail, ["orphans 1", "[o", "|", "]"]);
        assert_eq!(rows.orphan_tops, vec![(kinds.len() - 3, "o".to_string())]);
        // Every target still maps to its own row, cards notwithstanding.
        for (t, &row) in rows.row_of_target.iter().enumerate() {
            assert!(
                matches!(&rows.rows[row], Row::Unified { target, .. } if *target == t),
                "target {t} at row {row}"
            );
        }
        // Card rows carry the anchor's own target so a click lands on the line, on the anchor's side.
        let card = rows
            .rows
            .iter()
            .find(|r| matches!(r, Row::Card { id, .. } if id == "l"))
            .unwrap();
        assert!(
            matches!(card, Row::Card { target: Some(t), .. } if diff.targets[*t].side == Side::Additions && diff.targets[*t].line_number == 11)
        );
        let range_card = rows
            .rows
            .iter()
            .find(|r| matches!(r, Row::Card { id, .. } if id == "r"))
            .unwrap();
        assert!(
            matches!(range_card, Row::Card { target: Some(t), .. } if diff.targets[*t].side == Side::Deletions && diff.targets[*t].line_number == 12)
        );
        assert!(rows
            .rows
            .iter()
            .any(|r| matches!(r, Row::Card { id, target: None, .. } if id == "o")));
    }

    #[test]
    fn editable_comments_on_missing_lines_join_the_orphans() {
        let diff = loaded(&[(10, 10, &[('+', "a"), ('+', "b"), ('+', "c")])]);
        let mut missing = comment("m", &diff.key, Side::Additions, 999, Span::Line);
        let mut gone = missing.clone();
        gone.id = "g".into();
        gone.anchor.key.path = "gone.rs".into();
        for state in [
            CommentState::Pending,
            CommentState::Unconfirmed {
                stamp: stamp(),
                before: Vec::new(),
            },
        ] {
            missing.state = state;
            for mode in [ViewMode::Unified, ViewMode::Split] {
                let rows = build(&diff, mode, &[&missing], &[], 80, None);
                let top = rows.rows.len() - 3;
                assert_eq!(rows.orphan_tops, [(top, missing.id.clone())]);
                assert!(matches!(rows.rows[top - 1], Row::Orphans { count: 1 }));
                assert!(matches!(
                    &rows.rows[top],
                    Row::Card { target: None, line: CardLine::Top { title, .. }, .. }
                        if title == &format!("src/f.rs:999 · Bug · {}", cards::state_word(&missing.state))
                ));

                let rows = build(&diff, mode, &[&missing], &[&gone], 80, None);
                assert_eq!(
                    rows.orphan_tops
                        .iter()
                        .map(|(_, id)| id.as_str())
                        .collect::<Vec<_>>(),
                    ["g", "m"]
                );
                assert_eq!(
                    rows.rows
                        .iter()
                        .filter(|r| matches!(r, Row::Orphans { count: 2 }))
                        .count(),
                    1
                );
            }
        }
        missing.state = CommentState::Sent(stamp());
        let rows = build(&diff, ViewMode::Unified, &[&missing], &[], 80, None);
        assert!(rows.orphan_tops.is_empty());
        assert!(!rows
            .rows
            .iter()
            .any(|r| matches!(r, Row::Card { .. } | Row::Orphans { .. })));
    }

    #[test]
    fn only_this_rows_comments_under_this_comparison_kind_are_in_place() {
        let snap = crate::tui::state::tests::snapshot("f", "raw", &[(10, " + ")]);
        let diff = match &snap.diff {
            DiffState::Ready(d) => d.clone(),
            _ => unreachable!(),
        };
        let mine = comment("a", &diff.key, Side::Additions, 10, Span::Line);
        let other_half = comment(
            "b",
            &FileKey {
                staged: true,
                ..diff.key.clone()
            },
            Side::Additions,
            10,
            Span::Line,
        );
        let mut branch = mine.clone();
        branch.id = "c".into();
        branch.anchor.comparison = AnchorComparison::Branch {
            merge_base: "0".repeat(40),
            label: "main".into(),
        };
        let all = vec![mine.clone(), other_half, branch.clone()];
        let shown = in_place(&all, &diff, false);
        assert_eq!(
            shown.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        let two = vec![mine, branch];
        assert_eq!(
            in_place(&two, &diff, true)
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["c"]
        );
    }

    #[test]
    fn orphans_are_unsent_comments_whose_row_left_the_list() {
        let mut snap = crate::tui::state::tests::snapshot("f", "raw", &[(10, " + ")]);
        // `snapshot` lists no files; the present row must be listed for its comment not to be an orphan.
        snap.files = vec![crate::git::ChangedFile {
            path: "f".into(),
            status: crate::git::ChangedFileStatus::Modified,
            staged: false,
            insertions: None,
            deletions: None,
        }];
        let key = FileKey {
            path: "f".into(),
            staged: false,
            untracked: false,
        };
        let present = comment("p", &key, Side::Additions, 10, Span::Line);
        let gone = comment(
            "g",
            &FileKey {
                path: "gone".into(),
                staged: false,
                untracked: false,
            },
            Side::Additions,
            1,
            Span::Line,
        );
        let mut sent_gone = gone.clone();
        sent_gone.id = "s".into();
        sent_gone.state = CommentState::Sent(stamp());
        let mut other_kind = gone.clone();
        other_kind.id = "k".into();
        other_kind.anchor.comparison = AnchorComparison::Branch {
            merge_base: "0".repeat(40),
            label: "main".into(),
        };
        snap.comments = std::sync::Arc::new(vec![present, gone, sent_gone, other_kind]);
        let ids: Vec<_> = orphans(&snap.comments, &snap)
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["g"],
            "present rows, sent comments and the other comparison kind are not orphans"
        );
    }
}
