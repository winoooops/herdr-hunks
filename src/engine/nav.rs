//! Pure navigation model, ported from vimeflow's useReviewTargetNavigation.
use crate::git::{DiffLineType, FileDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// New-file line numbers.
    Additions,
    /// Old-file line numbers.
    Deletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Unified,
    Split,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub line_number: u32,
    pub side: Side,
    pub hunk_index: usize,
    pub split_row_index: usize,
    pub changed: bool,
}

/// Row-ordered targets; a changed block is emitted pair-interleaved (split order).
pub fn targets_for_diff(file_diff: &FileDiff) -> Vec<Target> {
    let mut targets = Vec::new();
    for (hunk_index, hunk) in file_diff.hunks.iter().enumerate() {
        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        let mut split_row = 0usize;
        let mut deletions: Vec<Target> = Vec::new();
        let mut additions: Vec<Target> = Vec::new();

        if hunk.lines.is_empty() {
            let deleted_only = hunk.new_lines == 0;
            targets.push(Target {
                line_number: if deleted_only {
                    hunk.old_start
                } else {
                    hunk.new_start
                },
                side: if deleted_only {
                    Side::Deletions
                } else {
                    Side::Additions
                },
                hunk_index,
                split_row_index: 0,
                changed: true,
            });
            continue;
        }

        let flush = |targets: &mut Vec<Target>,
                     split_row: &mut usize,
                     deletions: &mut Vec<Target>,
                     additions: &mut Vec<Target>| {
            let rows = deletions.len().max(additions.len());
            for offset in 0..rows {
                if let Some(d) = deletions.get(offset) {
                    targets.push(Target {
                        split_row_index: *split_row + offset,
                        ..d.clone()
                    });
                }
                if let Some(a) = additions.get(offset) {
                    targets.push(Target {
                        split_row_index: *split_row + offset,
                        ..a.clone()
                    });
                }
            }
            *split_row += rows;
            deletions.clear();
            additions.clear();
        };

        for line in &hunk.lines {
            match line.line_type {
                DiffLineType::Removed => {
                    let number = line.old_line_number.unwrap_or(old_line);
                    deletions.push(Target {
                        line_number: number,
                        side: Side::Deletions,
                        hunk_index,
                        split_row_index: 0,
                        changed: true,
                    });
                }
                DiffLineType::Added => {
                    let number = line.new_line_number.unwrap_or(new_line);
                    additions.push(Target {
                        line_number: number,
                        side: Side::Additions,
                        hunk_index,
                        split_row_index: 0,
                        changed: true,
                    });
                }
                DiffLineType::Context => {
                    flush(&mut targets, &mut split_row, &mut deletions, &mut additions);
                    let number = line.new_line_number.unwrap_or(new_line);
                    targets.push(Target {
                        line_number: number,
                        side: Side::Additions,
                        hunk_index,
                        split_row_index: split_row,
                        changed: false,
                    });
                    split_row += 1;
                }
            }
            if !matches!(line.line_type, DiffLineType::Added) {
                old_line += 1;
            }
            if !matches!(line.line_type, DiffLineType::Removed) {
                new_line += 1;
            }
        }
        flush(&mut targets, &mut split_row, &mut deletions, &mut additions);
    }
    targets
}

/// Indices into `targets` in unified display order: a block's deletions, then its additions.
pub fn unified_order(targets: &[Target]) -> Vec<usize> {
    let mut order = Vec::with_capacity(targets.len());
    let mut i = 0;
    while i < targets.len() {
        if !targets[i].changed {
            order.push(i);
            i += 1;
            continue;
        }
        let hunk = targets[i].hunk_index;
        let mut end = i;
        while end < targets.len() && targets[end].changed && targets[end].hunk_index == hunk {
            end += 1;
        }
        order.extend((i..end).filter(|&k| targets[k].side == Side::Deletions));
        order.extend((i..end).filter(|&k| targets[k].side == Side::Additions));
        i = end;
    }
    order
}

pub fn target_index_for_hunk(targets: &[Target], hunk_index: usize) -> Option<usize> {
    targets
        .iter()
        .position(|t| t.hunk_index == hunk_index && t.changed)
        .or_else(|| targets.iter().position(|t| t.hunk_index == hunk_index))
}

/// `delta` is +1 or -1. Clamps at both ends. Returns the new cursor index.
pub fn move_line(
    targets: &[Target],
    unified: &[usize],
    cursor: usize,
    delta: i32,
    mode: ViewMode,
) -> usize {
    if targets.is_empty() || delta == 0 {
        return cursor;
    }
    let current = cursor.min(targets.len() - 1);
    let step = i64::from(delta.signum());
    match mode {
        ViewMode::Unified => {
            let pos = unified.iter().position(|&i| i == current).unwrap_or(0) as i64;
            let next = pos + step;
            if next < 0 || next >= unified.len() as i64 {
                current
            } else {
                unified[next as usize]
            }
        }
        ViewMode::Split => {
            let base = targets[current].clone();
            let len = targets.len() as i64;
            let mut row = current as i64;
            if row + step < 0 || row + step >= len {
                return current;
            }
            while row + step >= 0 && row + step < len {
                row += step;
                let t = &targets[row as usize];
                if t.hunk_index != base.hunk_index || t.split_row_index != base.split_row_index {
                    break;
                }
            }
            let landed = &targets[row as usize];
            targets
                .iter()
                .position(|t| {
                    t.hunk_index == landed.hunk_index
                        && t.split_row_index == landed.split_row_index
                        && t.side == base.side
                })
                .unwrap_or(row as usize)
        }
    }
}

pub fn move_side(targets: &[Target], cursor: usize, side: Side, mode: ViewMode) -> usize {
    if mode != ViewMode::Split || targets.is_empty() {
        return cursor;
    }
    let current = &targets[cursor.min(targets.len() - 1)];
    targets
        .iter()
        .position(|t| {
            t.hunk_index == current.hunk_index
                && t.split_row_index == current.split_row_index
                && t.side == side
        })
        .unwrap_or(cursor)
}

pub fn find(targets: &[Target], side: Side, line_number: u32) -> Option<usize> {
    targets
        .iter()
        .position(|t| t.side == side && t.line_number == line_number)
}

/// Same side, smallest line distance; used when a refresh removed the cursor's line.
pub fn nearest(targets: &[Target], side: Side, line_number: u32) -> Option<usize> {
    targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.side == side)
        .min_by_key(|(_, t)| t.line_number.abs_diff(line_number))
        .map(|(i, _)| i)
        .or(if targets.is_empty() { None } else { Some(0) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffHunk, DiffLine, DiffLineType, FileDiff};

    fn line(kind: char) -> DiffLine {
        DiffLine {
            line_type: match kind {
                '+' => DiffLineType::Added,
                '-' => DiffLineType::Removed,
                _ => DiffLineType::Context,
            },
            content: String::new(),
            old_line_number: None,
            new_line_number: None,
        }
    }

    fn diff(hunks: &[(u32, u32, &str)]) -> FileDiff {
        FileDiff {
            file_path: "f".into(),
            old_path: None,
            new_path: None,
            hunks: hunks
                .iter()
                .map(|(old_start, new_start, kinds)| DiffHunk {
                    id: format!("hunk-{old_start}-{new_start}"),
                    header: String::new(),
                    old_start: *old_start,
                    old_lines: kinds.chars().filter(|c| *c != '+').count() as u32,
                    new_start: *new_start,
                    new_lines: kinds.chars().filter(|c| *c != '-').count() as u32,
                    lines: kinds.chars().map(line).collect(),
                })
                .collect(),
        }
    }

    fn shape(t: &Target) -> (u32, Side, usize, usize, bool) {
        (
            t.line_number,
            t.side,
            t.hunk_index,
            t.split_row_index,
            t.changed,
        )
    }

    // vimeflow fixture: one context line, then one added line.
    #[test]
    fn context_then_addition() {
        let targets = targets_for_diff(&diff(&[(1, 1, " +")]));
        assert_eq!(
            targets.iter().map(shape).collect::<Vec<_>>(),
            vec![
                (1, Side::Additions, 0, 0, false),
                (2, Side::Additions, 0, 1, true)
            ]
        );
    }

    // Two deletions replaced by one addition: rows pair up, the extra deletion gets its own row.
    #[test]
    fn replacement_block_pairs_rows() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(
            targets.iter().map(shape).collect::<Vec<_>>(),
            vec![
                (10, Side::Additions, 0, 0, false),
                (11, Side::Deletions, 0, 1, true),
                (11, Side::Additions, 0, 1, true),
                (12, Side::Deletions, 0, 2, true),
                (12, Side::Additions, 0, 3, false),
            ]
        );
    }

    #[test]
    fn unified_order_puts_a_blocks_deletions_first() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(unified_order(&targets), vec![0, 1, 3, 2, 4]);
    }

    #[test]
    fn hunk_navigation_lands_on_the_first_changed_row() {
        let targets = targets_for_diff(&diff(&[(1, 1, "  +"), (20, 21, " - ")]));
        assert_eq!(target_index_for_hunk(&targets, 0), Some(2));
        assert_eq!(target_index_for_hunk(&targets, 1), Some(4));
        assert_eq!(target_index_for_hunk(&targets, 9), None);
    }

    #[test]
    fn move_line_unified_follows_display_order_and_clamps() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        let order = unified_order(&targets);
        assert_eq!(move_line(&targets, &order, 1, 1, ViewMode::Unified), 3);
        assert_eq!(move_line(&targets, &order, 3, 1, ViewMode::Unified), 2);
        assert_eq!(move_line(&targets, &order, 0, -1, ViewMode::Unified), 0);
        assert_eq!(move_line(&targets, &order, 4, 1, ViewMode::Unified), 4);
    }

    #[test]
    fn move_line_split_treats_a_pair_as_one_row_and_keeps_the_side() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        let order = unified_order(&targets);
        // from the deletion in row 1, down goes to the deletion in row 2
        assert_eq!(move_line(&targets, &order, 1, 1, ViewMode::Split), 3);
        // from the addition in row 1, down: row 2 has no addition, so land on its deletion
        assert_eq!(move_line(&targets, &order, 2, 1, ViewMode::Split), 3);
        // from row 0 down lands on the additions side of row 1
        assert_eq!(move_line(&targets, &order, 0, 1, ViewMode::Split), 2);
    }

    #[test]
    fn move_side_switches_within_a_row_in_split_mode_only() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(move_side(&targets, 1, Side::Additions, ViewMode::Split), 2);
        assert_eq!(move_side(&targets, 3, Side::Additions, ViewMode::Split), 3);
        assert_eq!(
            move_side(&targets, 1, Side::Additions, ViewMode::Unified),
            1
        );
    }

    #[test]
    fn find_and_nearest() {
        let targets = targets_for_diff(&diff(&[(10, 10, " --+ ")]));
        assert_eq!(find(&targets, Side::Deletions, 12), Some(3));
        assert_eq!(find(&targets, Side::Deletions, 99), None);
        assert_eq!(nearest(&targets, Side::Deletions, 99), Some(3));
        assert_eq!(nearest(&[], Side::Additions, 1), None);
    }

    #[test]
    fn a_hunk_without_lines_yields_one_target() {
        let mut d = diff(&[(5, 5, "")]);
        d.hunks[0].new_lines = 0;
        let targets = targets_for_diff(&d);
        assert_eq!(
            targets.iter().map(shape).collect::<Vec<_>>(),
            vec![(5, Side::Deletions, 0, 0, true)]
        );
    }
}
