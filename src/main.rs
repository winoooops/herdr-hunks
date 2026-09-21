use std::path::PathBuf;

use herdr_hunks::actions::Placement;
#[cfg(feature = "tui")]
use herdr_hunks::tui::shell::run as run_tui;

#[cfg(not(feature = "tui"))]
fn run_tui(_path: PathBuf) -> i32 {
    eprintln!("herdr-hunks: the viewer requires the tui feature");
    1
}

fn main() {
    herdr_hunks::engine::init_process_env();
    let mut args = std::env::args().skip(1);
    let first = args.next();
    let code = match first.as_deref() {
        None => run_tui(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        Some("tui") => {
            let path = args
                .next()
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            run_tui(path)
        }
        Some("open") => herdr_hunks::actions::run_open(Placement::Popup),
        Some("open-split") => herdr_hunks::actions::run_open(Placement::Split),
        Some("update") => herdr_hunks::actions::update::run(),
        Some("--version") | Some("-V") => {
            println!("herdr-hunks {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some(flag) if flag.starts_with('-') => {
            eprintln!("herdr-hunks: unknown option `{flag}` (expected: tui [PATH], open, open-split, update)");
            2
        }
        // Any other word is a path. The engine validates it, so a missing path shows the error state.
        Some(path) => run_tui(PathBuf::from(path)),
    };
    std::process::exit(code);
}
