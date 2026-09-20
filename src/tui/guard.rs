use std::io::Write;
use std::time::Duration;

type DisableRawMode = fn() -> std::io::Result<()>;

pub struct TerminalGuard<
    W: Write = std::io::Stdout,
    D: FnMut() -> std::io::Result<()> = DisableRawMode,
> {
    output: W,
    disable_raw_mode: D,
    /// Conservative "capture may be applied" (spec §2): set before an
    /// enable is attempted, cleared only after a successful disable.
    mouse: bool,
}

impl TerminalGuard {
    pub fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let mut output = std::io::stdout();
        if let Err(error) =
            crossterm::execute!(&mut output, crossterm::terminal::EnterAlternateScreen)
        {
            let _ = crossterm::terminal::disable_raw_mode();
            return Err(error);
        }
        Ok(Self {
            output,
            disable_raw_mode: crossterm::terminal::disable_raw_mode,
            mouse: false,
        })
    }
}

impl<W: Write, D: FnMut() -> std::io::Result<()>> TerminalGuard<W, D> {
    /// Every scheduled transition writes its bytes — no equality
    /// short-circuit on `self.mouse`, because after a failed disable the
    /// flag reads true while reporting may be partially off (spec §2).
    pub fn set_mouse(&mut self, on: bool) -> std::io::Result<()> {
        if on {
            self.mouse = true;
            crossterm::execute!(&mut self.output, crossterm::event::EnableMouseCapture)
        } else {
            crossterm::execute!(&mut self.output, crossterm::event::DisableMouseCapture)?;
            self.mouse = false;
            Ok(())
        }
    }
}

impl<W: Write, D: FnMut() -> std::io::Result<()>> Drop for TerminalGuard<W, D> {
    fn drop(&mut self) {
        // Capture off strictly first: leaking capture into the user's
        // shell is the worst failure this feature can add (spec §2).
        if self.mouse {
            let _ = crossterm::execute!(&mut self.output, crossterm::event::DisableMouseCapture);
        }
        let _ = crossterm::execute!(&mut self.output, crossterm::terminal::LeaveAlternateScreen);
        let _ = (self.disable_raw_mode)();
    }
}

fn terminal_is_gone(revents: i16) -> bool {
    revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

/// Wait on the tty itself before asking Crossterm to parse pending input.
/// Crossterm 0.29 loops internally when a pty reports EOF as readable, so its
/// timeout cannot protect us after the master disappears.
pub fn poll_terminal(timeout: Duration) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;

    let mut input = libc::pollfd {
        fd: std::io::stdin().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: `input` is one initialized pollfd, its stdin fd stays open for
    // the call, and the count matches the one-element buffer.
    let ready = unsafe { libc::poll(&mut input, 1, timeout_ms) };
    if ready < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    if terminal_is_gone(input.revents) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "terminal input is gone",
        ));
    }
    // Zero cannot trap us in Crossterm's EOF loop because HUP/ERR was handled
    // above; it also lets Crossterm deliver resize signals after our wait.
    crossterm::event::poll(Duration::ZERO)
}

/// One attempt per change of the requested value (spec §2 rule 1),
/// keyed on the request — never on the guard's conservative flag, which
/// diverges forever after a failed disable and would retry every tick.
pub fn mouse_transition(requested: bool, last_attempted: Option<bool>) -> Option<bool> {
    (last_attempted != Some(requested)).then_some(requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_terminal_guard_leaves_the_screen_and_raw_mode() {
        let mut output = Vec::new();
        let raw_disabled = std::cell::Cell::new(false);
        {
            let _guard = TerminalGuard {
                output: &mut output,
                disable_raw_mode: || {
                    raw_disabled.set(true);
                    Ok(())
                },
                mouse: false,
            };
        }

        assert_eq!(output, b"\x1b[?1049l", "alternate screen was not left");
        assert!(raw_disabled.get(), "raw mode was not disabled");
    }

    #[test]
    fn a_tty_hangup_is_not_readable_input() {
        assert!(terminal_is_gone(libc::POLLHUP));
        assert!(!terminal_is_gone(libc::POLLIN));
    }

    /// A writer the test can make fail on demand, with a shared view of
    /// everything successfully written.
    #[derive(Clone)]
    struct FlakyWriter {
        fail: std::rc::Rc<std::cell::Cell<bool>>,
        wrote: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    }

    impl FlakyWriter {
        fn new() -> Self {
            Self {
                fail: std::rc::Rc::new(std::cell::Cell::new(false)),
                wrote: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            }
        }

        fn written(&self) -> String {
            String::from_utf8_lossy(&self.wrote.borrow()).into_owned()
        }
    }

    impl Write for FlakyWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.fail.get() {
                return Err(std::io::Error::other("flaky"));
            }
            self.wrote.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail.get() {
                return Err(std::io::Error::other("flaky"));
            }
            Ok(())
        }
    }

    fn test_guard(writer: FlakyWriter) -> TerminalGuard<FlakyWriter, fn() -> std::io::Result<()>> {
        TerminalGuard {
            output: writer,
            disable_raw_mode: || Ok(()),
            mouse: false,
        }
    }

    #[test]
    fn drop_disables_capture_before_leaving_the_alternate_screen() {
        let writer = FlakyWriter::new();
        let mut guard = test_guard(writer.clone());
        guard.set_mouse(true).expect("enable");
        drop(guard);
        let bytes = writer.written();
        let disable = bytes.find("?1000l").expect("disable-capture bytes");
        let leave = bytes.find("?1049l").expect("leave-alternate-screen bytes");
        assert!(disable < leave, "capture off strictly before leave-alt");
    }

    #[test]
    fn drop_without_capture_never_emits_a_disable() {
        let writer = FlakyWriter::new();
        let guard = test_guard(writer.clone());
        drop(guard);
        assert!(!writer.written().contains("?1000l"));
        assert!(writer.written().contains("?1049l"));
    }

    #[test]
    fn the_conservative_flag_survives_failures_and_never_short_circuits() {
        let writer = FlakyWriter::new();
        let mut guard = test_guard(writer.clone());

        // A failed enable still marks "may be applied" (spec §2).
        writer.fail.set(true);
        assert!(guard.set_mouse(true).is_err());
        assert!(guard.mouse, "flag set before the attempt");

        // A failed disable does not clear it.
        assert!(guard.set_mouse(false).is_err());
        assert!(guard.mouse, "cleared only after a successful disable");

        // Recovery emits bytes: a new `on` request writes the enable
        // sequence even though the flag already reads true.
        writer.fail.set(false);
        writer.wrote.borrow_mut().clear();
        guard.set_mouse(true).expect("re-enable");
        assert!(
            writer.written().contains("?1000h"),
            "no equality short-circuit on the conservative flag"
        );

        // And a successful disable finally clears it.
        guard.set_mouse(false).expect("disable");
        assert!(!guard.mouse);
    }

    #[test]
    fn the_reconciler_attempts_once_per_requested_change() {
        // Startup None forces exactly one attempt, whatever the request.
        assert_eq!(mouse_transition(true, None), Some(true));
        assert_eq!(mouse_transition(false, None), Some(false));
        // A repeated tick with the same request does nothing…
        assert_eq!(mouse_transition(true, Some(true)), None);
        assert_eq!(mouse_transition(false, Some(false)), None);
        // …and each change attempts exactly once, success or not.
        assert_eq!(mouse_transition(false, Some(true)), Some(false));
        assert_eq!(mouse_transition(true, Some(false)), Some(true));
    }
}
