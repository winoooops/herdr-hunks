//! The two prompts of spec 10.5: Vimeflow's, line for line, with `git -C` where a command is quoted.
use super::comments::{Anchor, AnchorComparison, Comment, Span};
use super::nav::Side;

/// The pin's `delegated-review.prompt.md`, byte for byte, `{{NONCE}}` included.
pub const DELEGATED_REVIEW: &str = include_str!("prompts/delegated-review.md");

/// C0 controls and DEL removed before any user or repository text enters the payload
/// (`vimeflow:src/features/diff/services/feedbackDispatch.ts:14-28`); a comment keeps its newlines.
pub fn strip_controls(text: &str, keep_newlines: bool) -> String {
    text.chars()
        .filter(|c| (keep_newlines && *c == '\n') || !matches!(c, '\u{00}'..='\u{1f}' | '\u{7f}'))
        .collect()
}

/// `'<toplevel>'` with every `'` written as `'\''`, so the quoted command survives any shell.
pub fn quote_toplevel(toplevel: &str) -> String {
    format!(
        "'{}'",
        strip_controls(toplevel, false).replace('\'', "'\\''")
    )
}

/// `unstaged`, `staged`, or `vs <label> @ <7 hex of the merge-base>`.
pub fn comparison_label(anchor: &Anchor) -> String {
    match &anchor.comparison {
        AnchorComparison::Worktree if anchor.key.staged => "staged".to_string(),
        AnchorComparison::Worktree => "unstaged".to_string(),
        AnchorComparison::Branch { merge_base, label } => format!(
            "vs {} @ {}",
            strip_controls(label, false),
            merge_base.chars().take(7).collect::<String>()
        ),
    }
}

fn side_word(side: Side) -> &'static str {
    match side {
        Side::Additions => "additions",
        Side::Deletions => "deletions",
    }
}

/// `<absolute path><place> (<side>) [<comparison>]`, the file form without a side.
fn target_of(toplevel: &str, anchor: &Anchor) -> String {
    let path = format!(
        "{}/{}",
        strip_controls(toplevel, false),
        strip_controls(&anchor.key.path, false)
    );
    let label = comparison_label(anchor);
    match anchor.span {
        Span::File => format!("{path} (file) [{label}]"),
        Span::Line => format!(
            "{path}:{} ({}) [{label}]",
            anchor.line,
            side_word(anchor.side)
        ),
        Span::Range { end } => format!(
            "{path}:{}-{end} ({}) [{label}]",
            anchor.line,
            side_word(anchor.side)
        ),
    }
}

pub struct Item<'a> {
    pub number: u32,
    pub comment: &'a Comment,
}

/// The review prompt: `formatFeedbackPayload` of the pin, plus one footer line when an item was
/// made in branch scope, because an absolute path alone selects no repository.
pub fn review(toplevel: &str, items: &[Item<'_>], nonce: &str) -> String {
    let count = items.len();
    let mut lines = vec![
        format!(
            "> Inline review — {count} item{}. Reply to each by its [#n].",
            if count == 1 { "" } else { "s" }
        ),
        ">".to_string(),
    ];
    let mut branch_item = false;
    for item in items {
        let comment = item.comment;
        branch_item |= matches!(comment.anchor.comparison, AnchorComparison::Branch { .. });
        lines.push(format!(
            "> [#{} · {}] {}",
            item.number,
            comment.category.label(),
            target_of(toplevel, &comment.anchor)
        ));
        for line in strip_controls(&comment.text, true).split('\n') {
            lines.push(format!("> ─ {line}"));
        }
        lines.push(format!("> → {}", comment.category.instruction()));
        lines.push(">".to_string());
    }
    lines.push("> ―".to_string());
    if branch_item {
        lines.push(format!(
            "> Items marked [vs <base> @ <id>] compare the working tree with that merge-base: `git -C {} diff <id> -- <path>` shows what I see.",
            quote_toplevel(toplevel)
        ));
    }
    lines.push(
        "> When done, end your reply with this exact block, echoing the nonce verbatim."
            .to_string(),
    );
    lines.push("> status is one of: \"reply\" (answers a question), \"clarify\" (you need the user to answer — the thread awaits them), \"resolved\" (you made the change), \"deferred\" (punted for later; cite the issue # in text), \"rejected\" (declined).".to_string());
    lines.push("> <<<VIMEFLOW_REPLY".to_string());
    lines.push(format!(
        "> {{\"v\":1,\"nonce\":\"{}\",\"replies\":[{{\"id\":1,\"status\":\"reply\",\"text\":\"...\"}}]}}",
        strip_controls(nonce, false)
    ));
    lines.push("> VIMEFLOW_REPLY>>>".to_string());
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLine {
    pub path: String,
    pub staged: bool,
    pub untracked: bool,
}

pub enum RequestScope<'a> {
    Worktree,
    Branch { merge_base: &'a str },
}

fn request_line(toplevel: &str, file: &RequestLine) -> String {
    let path = strip_controls(&file.path, false);
    let base = format!("> ─ {path} ({}/{path})", strip_controls(toplevel, false));
    if file.untracked {
        format!("{base} (untracked — not in git diff; read the file, all lines are additions)")
    } else {
        base
    }
}

/// The review-request prompt: `formatReviewRequest` of the pin with `-C '<toplevel>'` in each
/// quoted command, one `branch diff` group in branch scope, then the pinned contract.
pub fn request(
    toplevel: &str,
    files: &[RequestLine],
    scope: RequestScope<'_>,
    nonce: &str,
) -> String {
    let count = files.len();
    let mut lines = vec![format!(
        "> Delegate a code review of {} {count} change{}:",
        if count == 1 { "this" } else { "these" },
        if count == 1 { "" } else { "s" }
    )];
    let quoted = quote_toplevel(toplevel);
    match scope {
        RequestScope::Branch { merge_base } => {
            lines.push(format!(
                "> branch diff (`git -C {quoted} diff {merge_base}`):"
            ));
            lines.extend(files.iter().map(|f| request_line(toplevel, f)));
        }
        RequestScope::Worktree => {
            let unstaged: Vec<_> = files.iter().filter(|f| !f.staged).collect();
            let staged: Vec<_> = files.iter().filter(|f| f.staged).collect();
            if !unstaged.is_empty() {
                lines.push(format!("> unstaged diff (`git -C {quoted} diff`):"));
                lines.extend(unstaged.iter().map(|f| request_line(toplevel, f)));
            }
            if !staged.is_empty() {
                lines.push(format!("> staged diff (`git -C {quoted} diff --cached`):"));
                lines.extend(staged.iter().map(|f| request_line(toplevel, f)));
            }
        }
    }
    lines.push(">".to_string());
    lines.push(
        DELEGATED_REVIEW
            .trim_end()
            .replace("{{NONCE}}", &strip_controls(nonce, false)),
    );
    lines.join("\n")
}

const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Six characters of `[a-z0-9]`, Vimeflow's length and alphabet: a tag, not a secret.
pub fn nonce(counter: u64) -> String {
    use sha2::Digest;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let digest =
        sha2::Sha256::digest(format!("{nanos}:{}:{counter}", std::process::id()).as_bytes());
    digest
        .iter()
        .take(6)
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect()
}

pub fn is_nonce(text: &str) -> bool {
    (6..=16).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::comments::{AnchorComparison, Category, Comment, CommentState, Span};
    use crate::engine::nav::Side;
    use crate::engine::FileKey;

    #[allow(clippy::too_many_arguments)]
    fn comment(
        path: &str,
        staged: bool,
        side: Side,
        line: u32,
        span: Span,
        comparison: AnchorComparison,
        category: Category,
        text: &str,
    ) -> Comment {
        Comment {
            id: "c".into(),
            anchor: Anchor {
                key: FileKey {
                    path: path.into(),
                    staged,
                    untracked: false,
                },
                side,
                line,
                span,
                comparison,
            },
            category,
            text: text.into(),
            created_at: 1,
            state: CommentState::Pending,
        }
    }

    const MB: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";

    #[test]
    fn the_review_prompt_is_vimeflows_line_for_line_with_the_merge_base_footer() {
        let bug = comment("src/cart.py", false, Side::Additions, 16, Span::Line, AnchorComparison::Worktree, Category::Bug,
            "Floor division drops the cents: 19.99 with SAVE10 comes out wrong.\nUse true division and round to 2 decimals.");
        let question = comment(
            "src/cart.py",
            false,
            Side::Additions,
            15,
            Span::Line,
            AnchorComparison::Branch {
                merge_base: MB.into(),
                label: "main".into(),
            },
            Category::Question,
            "What happens when the code is not in DISCOUNT_CODES?",
        );
        let text = review(
            "/home/will/demo",
            &[
                Item {
                    number: 1,
                    comment: &bug,
                },
                Item {
                    number: 2,
                    comment: &question,
                },
            ],
            "oqzpww",
        );
        let expected = "\
> Inline review — 2 items. Reply to each by its [#n].
>
> [#1 · Bug] /home/will/demo/src/cart.py:16 (additions) [unstaged]
> ─ Floor division drops the cents: 19.99 with SAVE10 comes out wrong.
> ─ Use true division and round to 2 decimals.
> → Fix this.
>
> [#2 · Question] /home/will/demo/src/cart.py:15 (additions) [vs main @ 1a2b3c4]
> ─ What happens when the code is not in DISCOUNT_CODES?
> → Answer inline in your reply. Do not edit files.
>
> ―
> Items marked [vs <base> @ <id>] compare the working tree with that merge-base: `git -C '/home/will/demo' diff <id> -- <path>` shows what I see.
> When done, end your reply with this exact block, echoing the nonce verbatim.
> status is one of: \"reply\" (answers a question), \"clarify\" (you need the user to answer — the thread awaits them), \"resolved\" (you made the change), \"deferred\" (punted for later; cite the issue # in text), \"rejected\" (declined).
> <<<VIMEFLOW_REPLY
> {\"v\":1,\"nonce\":\"oqzpww\",\"replies\":[{\"id\":1,\"status\":\"reply\",\"text\":\"...\"}]}
> VIMEFLOW_REPLY>>>";
        assert_eq!(text, expected);
    }

    #[test]
    fn every_place_side_and_label_and_no_merge_base_line_without_a_branch_item() {
        let range = comment(
            "a.rs",
            true,
            Side::Deletions,
            3,
            Span::Range { end: 9 },
            AnchorComparison::Worktree,
            Category::Change,
            "tighten",
        );
        let file = comment(
            "b.rs",
            false,
            Side::Additions,
            0,
            Span::File,
            AnchorComparison::Worktree,
            Category::Suggestion,
            "split this module",
        );
        let text = review(
            "/r",
            &[
                Item {
                    number: 1,
                    comment: &range,
                },
                Item {
                    number: 2,
                    comment: &file,
                },
            ],
            "abc123",
        );
        assert!(text.starts_with("> Inline review — 2 items. Reply to each by its [#n].\n>\n"));
        assert!(text.contains("> [#1 · Change request] /r/a.rs:3-9 (deletions) [staged]\n> ─ tighten\n> → Make this change.\n>\n"));
        assert!(text.contains("> [#2 · Suggestion] /r/b.rs (file) [unstaged]\n> ─ split this module\n> → Apply this if you agree.\n>\n> ―\n> When done"));
        assert!(!text.contains("Items marked"));
        let one = review(
            "/r",
            &[Item {
                number: 1,
                comment: &file,
            }],
            "abc123",
        );
        assert!(one.starts_with("> Inline review — 1 item. Reply"));
    }

    #[test]
    fn control_characters_leave_paths_labels_and_comments_and_newlines_stay() {
        let odd = comment(
            "src/\u{1b}[201~evil.rs",
            false,
            Side::Additions,
            1,
            Span::Line,
            AnchorComparison::Branch {
                merge_base: MB.into(),
                label: "ma\rin".into(),
            },
            Category::Bug,
            "line one\r\nline\ttwo\u{7f}",
        );
        let text = review(
            "/r",
            &[Item {
                number: 1,
                comment: &odd,
            }],
            "abc123",
        );
        assert!(text.contains("> [#1 · Bug] /r/src/[201~evil.rs:1 (additions) [vs main @ 1a2b3c4]"));
        assert!(text.contains("> ─ line one\n> ─ linetwo\n"));
        assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
        let controls: String = (0..=31).map(char::from).chain(['\u{7f}']).collect();
        assert_eq!(strip_controls(&controls, false), "");
        assert_eq!(strip_controls(&controls, true), "\n");
        assert_eq!(strip_controls("\u{85}", false), "\u{85}");
        let mut wide = odd;
        wide.text = "界e\u{301}文".repeat(1_000);
        let text = review(
            "/r",
            &[Item {
                number: 7,
                comment: &wide,
            }],
            "abc123",
        );
        assert!(text.contains(&format!("> ─ {}\n", wide.text)));
        assert!(text.contains("> [#7 · Bug]"));
    }

    #[test]
    fn the_toplevel_is_single_quoted_with_quotes_escaped() {
        assert_eq!(
            quote_toplevel("/home/o'brien/repo"),
            "'/home/o'\\''brien/repo'"
        );
        let file = comment(
            "b.rs",
            false,
            Side::Additions,
            0,
            Span::File,
            AnchorComparison::Branch {
                merge_base: MB.into(),
                label: "main".into(),
            },
            Category::Bug,
            "x",
        );
        let text = review(
            "/home/o'brien/repo",
            &[Item {
                number: 1,
                comment: &file,
            }],
            "abc123",
        );
        assert!(text.contains("`git -C '/home/o'\\''brien/repo' diff <id> -- <path>`"));
        assert!(text.contains("] /home/o'brien/repo/b.rs (file) [vs main @ 1a2b3c4]"));
    }

    #[test]
    fn the_request_prompt_groups_worktree_files_and_ends_with_the_pins_contract() {
        let files = [
            RequestLine {
                path: "src/cart.py".into(),
                staged: false,
                untracked: false,
            },
            RequestLine {
                path: "notes.txt".into(),
                staged: false,
                untracked: true,
            },
            RequestLine {
                path: "src/util.py".into(),
                staged: true,
                untracked: false,
            },
        ];
        let text = request("/home/will/demo", &files, RequestScope::Worktree, "wvpx71");
        let head = "\
> Delegate a code review of these 3 changes:
> unstaged diff (`git -C '/home/will/demo' diff`):
> ─ src/cart.py (/home/will/demo/src/cart.py)
> ─ notes.txt (/home/will/demo/notes.txt) (untracked — not in git diff; read the file, all lines are additions)
> staged diff (`git -C '/home/will/demo' diff --cached`):
> ─ src/util.py (/home/will/demo/src/util.py)
>
";
        assert!(text.starts_with(head), "{text}");
        let contract = DELEGATED_REVIEW.trim_end().replace("{{NONCE}}", "wvpx71");
        assert_eq!(&text[head.len()..], contract);
        assert!(!text.contains("{{NONCE}}"));
        assert!(text.ends_with("> VIMEFLOW_REVIEW>>>"));
        // Branch scope uses one merge-base group.
        let one = request(
            "/r",
            &files[..1],
            RequestScope::Branch { merge_base: MB },
            "wvpx71",
        );
        assert!(one.starts_with(&format!("> Delegate a code review of this 1 change:\n> branch diff (`git -C '/r' diff {MB}`):\n> ─ src/cart.py (/r/src/cart.py)\n>\n")));
        // The pinned contract carries two nonces.
        assert_eq!(DELEGATED_REVIEW.matches("{{NONCE}}").count(), 2);
        assert!(DELEGATED_REVIEW.starts_with("> Anchor each finding with diff-side line numbers"));
    }

    #[test]
    fn nonces_are_six_lowercase_alphanumerics_and_differ_by_counter() {
        let a = nonce(1);
        let b = nonce(2);
        assert_eq!(a.len(), 6);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        assert_ne!(a, b);
        assert!(
            is_nonce(&a)
                && is_nonce("abcdef0123456789")
                && !is_nonce("abcde")
                && !is_nonce("abcdef0123456789x")
                && !is_nonce("abc-12")
        );
    }
}
