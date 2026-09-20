# herdr-hunks

**English** · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

A read-only git hunk viewer for [Herdr](https://herdr.dev), with changed files,
unified/split diffs, a clickable navigation toolbar, and live refresh.
Phase 1 does not stage, unstage, discard, add comments, or dispatch to agents.

## Install

**The GitHub repository and releases do not exist yet.** Use the local development
instructions below with this checkout. After the repository and first release
are published, the planned installation command is:

```sh
herdr plugin install winoooops/herdr-hunks
```

Fork users must run the same command with their `vimeflow` binary; the plugin
registries are separate. Installation will fetch a SHA256-verified binary for
macOS or Linux, on x86_64 or arm64. A missing asset, failed download, or checksum
mismatch falls back to `cargo build --release`, requiring Rust 1.88 or newer.

## Commands

Run an action through the host, which supplies the plugin ID and pane context:

```sh
herdr plugin action invoke open --plugin winoooops.hunks
herdr plugin action invoke open-split --plugin winoooops.hunks
herdr plugin action invoke update --plugin winoooops.hunks
```

| Action | Behaviour |
| --- | --- |
| `open` | Opens a focused overlay for the opener's working directory; closing restores the previous view. |
| `open-split` | Opens a split on the right, or focuses its existing viewer when the session, opener, repository, title, and cwd still match. |
| `update` | For a GitHub install, installs the newest stable version tag. Refuses linked/unknown sources. The GitHub repository and releases do not exist yet, so this is for after publication. |

Standalone: `herdr-hunks [PATH]` or `herdr-hunks tui [PATH]`; PATH defaults to the
current directory. Both stdin and stdout must be terminals. `--version` prints
the version. Action diagnostics are available through
`herdr plugin log list --plugin winoooops.hunks --limit 1`.

## Keybinding

Add this to your host configuration:

```toml
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "winoooops.hunks.open"
description = "Open the hunk viewer"
```

To bind the split action instead, change `command` to
`"winoooops.hunks.open-split"`. No keybinding installer runs automatically.

## Keys

| Key | Action |
| --- | --- |
| `j` / `k`, Down / Up | Next / previous row |
| Ctrl+D / Ctrl+U, PageDown / PageUp | Half page down / up |
| `[` / `]` | Previous / next hunk |
| `n` / `p` | Next / previous file |
| `h` / `l`, Left / Right | Deletions / additions side |
| `t` | Toggle unified / split; split needs at least 100 columns |
| `e` / `E` | Toggle / pin the files panel |
| `r` | Refresh |
| `g` / `G`, Home / End | First / last row |
| `H` / `L` | Scroll left / right by eight display cells |
| `m` | Toggle mouse capture |
| `?` | Open the key sheet; `j` / `k` or Down / Up scroll it |
| `q` | Quit; closes the key sheet first if open |
| Esc | Close the key sheet |
| Ctrl+C | Quit immediately, including from the key sheet |

Arrow, page, Home, and End aliases require no modifiers. Phase 1 leaves
`s d D i I u U x v y Y @ c /` unbound.

## Mouse

Click toolbar controls, file rows, or diff rows. The wheel scrolls three rows.
Press `m` to disable capture and restore native terminal text selection; press
it again to enable capture. Help captures wheel scrolling while open.

## Configuration

`config.toml` lives in `$HERDR_PLUGIN_CONFIG_DIR`, otherwise
`${XDG_CONFIG_HOME:-$HOME/.config}/herdr-hunks`:

```toml
[view]
mode = "auto"     # auto, unified, split
files = "auto"    # auto, pinned, hidden

[input]
mouse = true
```

At the initial width, auto mode selects split at 120 columns and auto files pins
the panel at 100 columns. A requested split temporarily becomes unified below
100 columns and returns when widened. Below 40×10, only a size notice is shown.
Invalid settings fall back individually and show a notice; diagnostics go to
`config-problems.log` in `$HERDR_PLUGIN_STATE_DIR`, otherwise
`${XDG_STATE_HOME:-$HOME/.local/state}/herdr-hunks`.

Directory settings must be absolute; empty/relative values fall through to the
next absolute setting. Without a usable state directory, split still opens but
skips reuse and reports why. No state directory is inferred from the repository.

## Requirements

- macOS or Linux, with terminal stdin and stdout.
- Git 2.31 or newer on PATH.
- Herdr 0.8.0 or newer for plugin actions; standalone use needs no host.
- Rust 1.88 or newer for local builds or installation fallback.

## Known limitations

This phase is read-only; staging and comments are not implemented.
[PORT-SURFACE.md lists K1–K6](PORT-SURFACE.md#known-defects): K1–K4 are frozen
mutation-path defects unreachable in Phase 1; K5 can hide the staged half of a
modified/added-then-deleted file; K6 allows superseded slow diffs to accumulate
git processes. The diff view keeps at most 200,000 lines, with a truncation row.
`GIT_NO_LAZY_FETCH` takes effect from Git 2.45; older Git may fetch missing objects
when reading a partial clone. No syntax highlighting or context expansion yet.

## Roadmap

P1: read-only viewing. P2: stage/unstage/discard actions and defect fixes.
P3: comments and agent dispatch. P4: replies and threads. P5: delegated review.
See the [design spec](docs/superpowers/specs/2026-09-18-hunks-roadmap-p1-viewer-design.md).

## Local development

From this checkout:

```sh
cargo build --release
herdr plugin link "$PWD"
herdr plugin action invoke open --plugin winoooops.hunks
```

`plugin link` skips `[[build]]`, so build first. Fork users substitute their
`vimeflow` binary. Build standalone actions without the viewer using
`cargo build --no-default-features`.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=1
cargo check --no-default-features
scripts/port-check.sh /path/to/vimeflow
sh scripts/port-check-selftest.sh /path/to/vimeflow
```

Tests create fixtures under HOME; it must be writable and outside a git checkout.
The reference checkout must contain commit `91e45b1c` and is read-only to these
checks. CI tries that pin from `winoooops/vimeflow`; if private, configure the
`VIMEFLOW_READ_TOKEN` Actions secret with read access. If checkout is unavailable,
CI prints a notice and skips both port checks; the commands above remain usable.

The future tag-triggered release workflow builds four targets and requires the
tag to match Cargo.toml. Keep Cargo.toml, Cargo.lock, and herdr-plugin.toml versions
in sync. `HERDR_HUNKS_RELEASE_BASE=file:///absolute/fixture` lets distribution
checks use local assets without contacting GitHub.

## Licence

[Apache-2.0](LICENSE). Includes code ported from vimeflow and herdr-agent-watcher;
see [PORT-SURFACE.md](PORT-SURFACE.md) for attribution and adaptations.
