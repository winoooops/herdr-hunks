//! The terminal loop; repository work stays on the engine's runtime.
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::Event;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::Paragraph;

use crate::engine::{self, Command, EngineHandle, SessionConfig, Snapshot};
use crate::tui::config::{self, Config, FilesSetting, ModeSetting};
use crate::tui::guard::{self, TerminalGuard};
use crate::tui::input::{self, Outcome};
use crate::tui::state::{FilesPanel, ViewState};
use crate::tui::style::{Role, Semantic, Style};
use crate::tui::view::{self, Rendered};

// Ratatui's drop prints cursor-restore errors; stderr may be the same disconnected tty.
struct TerminalOutput(io::Stdout);

impl Write for TerminalOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.0.write(bytes) {
            Err(_) if !self.0.is_terminal() => Ok(bytes.len()),
            result => result,
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0.flush() {
            Err(_) if !self.0.is_terminal() => Ok(()),
            result => result,
        }
    }
}

fn to_ratatui(style: &Style) -> ratatui::style::Style {
    use ratatui::style::{Color, Modifier, Style as RStyle};
    let mut result = RStyle::default();
    if let Some(semantic) = style.semantic {
        result = result.fg(match semantic {
            Semantic::Good => Color::Green,
            Semantic::Bad => Color::Red,
            Semantic::Accent => Color::Cyan,
            Semantic::Warn => Color::Yellow,
        });
    }
    result = match style.role {
        Role::Label | Role::Rule => result.add_modifier(Modifier::DIM),
        Role::Emphasis => result.add_modifier(Modifier::BOLD),
        Role::Body => result,
    };
    if style.reverse {
        result = result.add_modifier(Modifier::REVERSED);
    }
    result
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
        );
        let _ = crossterm::terminal::disable_raw_mode();
        previous(info);
    }));
}

fn config_notice(problems: &[String], dir: Option<&Path>) -> Option<String> {
    if problems.is_empty() {
        return None;
    }
    let Some(dir) = dir else {
        return Some(format!(
            "config: {} problem(s); no state directory for the log",
            problems.len()
        ));
    };
    let result = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(dir.join("config-problems.log"), problems.join("\n") + "\n"));
    Some(match result {
        Ok(()) => format!(
            "config: {} problem(s), see config-problems.log",
            problems.len()
        ),
        Err(error) => format!(
            "config: {} problem(s); cannot write config-problems.log: {error}",
            problems.len()
        ),
    })
}

fn initial_state(config: &Config, width: u16) -> ViewState {
    use crate::engine::nav::ViewMode;
    let mode = match config.mode {
        ModeSetting::Auto if width >= 120 => ViewMode::Split,
        ModeSetting::Split => ViewMode::Split,
        _ => ViewMode::Unified,
    };
    let files = match config.files {
        FilesSetting::Pinned => FilesPanel::Pinned,
        FilesSetting::Auto if width >= 100 => FilesPanel::Pinned,
        _ => FilesPanel::Hidden,
    };
    let mut state = ViewState::new(mode, files, config.mouse);
    state.resize(width, 0);
    if config.mode == ModeSetting::Split && width < view::MIN_SPLIT_WIDTH {
        state.notice = Some("split view needs 100 columns".into());
    }
    state
}

fn run_terminal(
    handle: &EngineHandle,
    config: &Config,
    notice: Option<String>,
    path: &std::path::Path,
) -> io::Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut terminal = ratatui::Terminal::new(CrosstermBackend::new(TerminalOutput(io::stdout())))?;
    let mut width = terminal.size()?.width;
    let mut state = initial_state(config, width);
    state.popup = std::env::var("HERDR_HUNKS_PLACEMENT").as_deref() == Ok("popup");
    state.notice = notice.or(state.notice);
    let mut snapshot = Arc::new(Snapshot::empty(&path.to_string_lossy()));
    let mut rendered = Rendered {
        lines: Vec::new(),
        hits: Vec::new(),
    };
    let mut dirty = true;
    let mut last_attempted = None;
    loop {
        if let Some(requested) = guard::mouse_transition(state.mouse_requested, last_attempted) {
            last_attempted = Some(requested);
            if let Err(error) = guard.set_mouse(requested) {
                state.notice = Some(format!("mouse capture failed: {error}"));
                dirty = true;
            }
        }
        while let Ok(next) = handle.snapshots.try_recv() {
            snapshot = next;
            dirty = true;
        }
        if dirty {
            terminal.draw(|frame| {
                let area = frame.area();
                width = area.width;
                state.resize(width, view::body_height(&state, &snapshot, area.height));
                state.reconcile(&snapshot);
                rendered = view::render(&snapshot, &state, width, area.height);
                let lines: Vec<_> = rendered
                    .lines
                    .iter()
                    .map(|line| {
                        ratatui::text::Line::from(
                            line.iter()
                                .map(|span| {
                                    ratatui::text::Span::styled(
                                        span.text.as_str(),
                                        to_ratatui(&span.style),
                                    )
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect();
                frame.render_widget(Paragraph::new(lines), area);
            })?;
            dirty = false;
        }
        match guard::poll_terminal(Duration::from_millis(100)) {
            Ok(false) => continue,
            Err(_) => break,
            Ok(true) => {}
        }
        let outcome = match crossterm::event::read() {
            Ok(Event::Key(key)) => input::handle_key(&mut state, &snapshot, key, width),
            Ok(Event::Mouse(mouse)) => input::handle_mouse(&mut state, &snapshot, &rendered, mouse),
            Ok(Event::Resize(_, _)) => Outcome::Redraw,
            Ok(_) => Outcome::Inert,
            Err(_) => break,
        };
        match outcome {
            Outcome::Quit => break,
            Outcome::Redraw => dirty = true,
            Outcome::Engine(command) => {
                let _ = handle.commands.send(command);
                dirty = true;
            }
            Outcome::Inert => {}
        }
    }
    Ok(())
}

/// The caller must initialize the process environment before starting any threads.
pub fn run(path: PathBuf) -> i32 {
    if !io::stdout().is_terminal() {
        eprintln!("herdr-hunks: stdout is not a terminal");
        return 2;
    }
    if !io::stdin().is_terminal() {
        eprintln!("herdr-hunks: stdin is not a terminal");
        return 2;
    }
    let lookup = |key: &str| std::env::var_os(key);
    let (config, problems) = config::config_dir(lookup)
        .as_deref()
        .map(config::load)
        .unwrap_or_default();
    let notice = config_notice(&problems, config::state_dir(lookup).as_deref());
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("herdr-hunks: cannot start runtime: {error}");
            return 1;
        }
    };
    let handle = engine::spawn(runtime.handle(), SessionConfig::production(path.clone()));
    install_panic_hook();
    let result = run_terminal(&handle, &config, notice, &path);
    let _ = handle.commands.send(Command::Shutdown);
    // A blocking git call must not keep a closed viewer alive for its full timeout.
    runtime.shutdown_timeout(Duration::from_secs(1));
    match result {
        Ok(()) => 0,
        Err(_) if !io::stdout().is_terminal() => 0,
        Err(error) => {
            eprintln!("herdr-hunks: terminal error: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::nav::ViewMode;

    #[test]
    fn missing_directories_do_not_read_config_or_write_logs_in_the_current_directory() {
        if std::env::var_os("HUNKS_MISSING_DIR_TEST_CHILD").is_none() {
            let dir = tempfile::tempdir().unwrap();
            let trap = dir.path().join(".config/herdr-hunks");
            std::fs::create_dir_all(&trap).unwrap();
            std::fs::write(trap.join("config.toml"), "[input]\nmouse = false\n").unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tui::shell::tests::missing_directories_do_not_read_config_or_write_logs_in_the_current_directory", "--nocapture"])
                .env("HUNKS_MISSING_DIR_TEST_CHILD", "1")
                .current_dir(dir.path()).output().unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!dir.path().join(".local").exists());
            return;
        }
        let lookup = |_: &str| None;
        let config_dir = config::config_dir(lookup);
        let state_dir = config::state_dir(lookup);
        assert!(config_dir.is_none() && state_dir.is_none());
        let (config, problems) = config_dir.as_deref().map(config::load).unwrap_or_default();
        assert_eq!(config, Config::default());
        assert!(problems.is_empty());
        assert_eq!(
            config_notice(&["bad value".into()], state_dir.as_deref()).as_deref(),
            Some("config: 1 problem(s); no state directory for the log")
        );
        assert_eq!(config_notice(&[], None), None);
        assert!(!Path::new(".local").exists());
    }

    #[test]
    fn settings_resolve_at_the_width_thresholds() {
        for (width, mode, files) in [
            (99, ViewMode::Unified, FilesPanel::Hidden),
            (100, ViewMode::Unified, FilesPanel::Pinned),
            (119, ViewMode::Unified, FilesPanel::Pinned),
            (120, ViewMode::Split, FilesPanel::Pinned),
        ] {
            let state = initial_state(&Config::default(), width);
            assert_eq!(
                (state.mode, state.files_panel, state.mouse_requested),
                (mode, files, true)
            );
        }
        let config = Config {
            mode: ModeSetting::Split,
            files: FilesSetting::Hidden,
            mouse: false,
        };
        let mut state = initial_state(&config, 99);
        assert_eq!(
            (state.mode, state.files_panel, state.mouse_requested),
            (ViewMode::Unified, FilesPanel::Hidden, false)
        );
        assert!(state.notice.as_deref().unwrap().contains("100 columns"));
        assert_eq!(state.requested_mode, ViewMode::Split);
        state.resize(120, 10);
        assert_eq!(state.mode, ViewMode::Split);
        assert_eq!(initial_state(&config, 100).mode, ViewMode::Split);
        let config = Config {
            mode: ModeSetting::Unified,
            files: FilesSetting::Pinned,
            mouse: true,
        };
        assert_eq!(initial_state(&config, 120).mode, ViewMode::Unified);
        assert_eq!(initial_state(&config, 40).files_panel, FilesPanel::Pinned);
    }

    #[test]
    fn styles_use_named_colours_and_preserve_dim_bold_and_reverse() {
        use ratatui::style::{Color, Modifier};
        for (semantic, color) in [
            (Semantic::Good, Color::Green),
            (Semantic::Bad, Color::Red),
            (Semantic::Accent, Color::Cyan),
            (Semantic::Warn, Color::Yellow),
        ] {
            let mapped = to_ratatui(&Style {
                role: Role::Emphasis,
                semantic: Some(semantic),
                reverse: true,
                rgb: Some((1, 2, 3)),
                ansi: Some(5),
            });
            assert_eq!(mapped.fg, Some(color));
            assert_eq!(mapped.bg, None);
            assert!(mapped
                .add_modifier
                .contains(Modifier::BOLD | Modifier::REVERSED));
        }
        for role in [Role::Label, Role::Rule] {
            assert!(to_ratatui(&Style::role(role))
                .add_modifier
                .contains(Modifier::DIM));
        }
        assert_eq!(
            to_ratatui(&Style::default()),
            ratatui::style::Style::default()
        );
    }

    #[test]
    fn panic_restores_the_terminal_before_printing() {
        use std::fs::File;
        use std::io::Read;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::process::{Command, Stdio};
        if std::env::var_os("HUNKS_PANIC_TEST_CHILD").is_some() {
            install_panic_hook();
            let mut guard = TerminalGuard::enter().unwrap();
            guard.set_mouse(true).unwrap();
            crossterm::execute!(io::stdout(), crossterm::cursor::Hide).unwrap();
            panic!("intentional terminal panic");
        }
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: openpty initializes both owned descriptors; optional settings are null.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        // SAFETY: these descriptors were just opened and ownership is transferred once.
        let (mut master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        let attrs = || {
            let mut value = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: tcgetattr initializes value on success; slave remains open.
            assert_eq!(
                unsafe { libc::tcgetattr(slave.as_raw_fd(), value.as_mut_ptr()) },
                0
            );
            // SAFETY: the successful call initialized value.
            unsafe { value.assume_init() }
        };
        let before = attrs();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tui::shell::tests::panic_restores_the_terminal_before_printing",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("HUNKS_PANIC_TEST_CHILD", "1")
            .env("RUST_BACKTRACE", "0")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()))
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("panic child did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(!status.success());
        assert_eq!(attrs().c_lflag, before.c_lflag);
        // SAFETY: master is an open descriptor; nonblocking reads allow draining with slave open.
        assert_ne!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
            -1
        );
        let mut bytes = Vec::new();
        let _ = master.read_to_end(&mut bytes);
        let output = String::from_utf8_lossy(&bytes);
        let disable = output.find("\x1b[?1000l").expect("mouse disabled");
        let leave = output.find("\x1b[?1049l").expect("alternate screen left");
        let show = output.find("\x1b[?25h").expect("cursor shown");
        let message = output
            .find("intentional terminal panic")
            .expect("panic printed");
        assert!(
            disable < leave && leave < show && show < message,
            "{output}"
        );
    }
}
