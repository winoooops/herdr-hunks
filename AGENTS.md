# Working on herdr-hunks

- Phase 1 is a read-only viewer. Keep git mutations, comments, and agent dispatch out of this phase.
- `src/git/` is frozen at vimeflow `91e45b1c8f381385093813d0b4eb1d9daeb2d563` plus registered patches. Never edit it by hand: change the pin or add a numbered patch in `port/patches/`, registered in `PORT-SURFACE.md`.
- Verify the frozen tree with `scripts/port-check.sh /path/to/vimeflow` and `sh scripts/port-check-selftest.sh /path/to/vimeflow`. Treat the reference checkout as read-only.
- Before committing, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo check --no-default-features`, plus both port checks. Test HOME must be writable and outside a git repository; preserve CARGO_HOME and RUSTUP_HOME when overriding it.
- Keep inherited clippy allowances on the `git` module declaration; do not alter frozen sources to satisfy lints.
- Reserved keys remain unbound: `s d D i I u U x v y Y @ c /`.
- Read the plugin ID from `HERDR_PLUGIN_ID`. Reach the host through `HERDR_BIN_PATH` or `HERDR_SOCKET_PATH`; never hardcode its executable name in Rust.
- Use the shared absolute config/state directory helpers. Never write state relative to the current directory. Sanitize git text before drawing; use named ANSI colours only.
- Keep Cargo.toml, Cargo.lock, and herdr-plugin.toml versions together, and mirror README changes in English, Simplified Chinese, and Japanese.
- Use conventional commits with lowercase subjects. Inline comments should be one short line and never refer to a task or PR.
