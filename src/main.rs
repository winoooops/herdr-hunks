use std::path::PathBuf;

fn main() {
    herdr_hunks::engine::init_process_env();
    let mut args = std::env::args().skip(1);
    let first = args.next();
    let code = match first.as_deref() {
        None => herdr_hunks::tui::shell::run(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        ),
        Some("tui") => {
            let path = args
                .next()
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            herdr_hunks::tui::shell::run(path)
        }
        Some("--version") | Some("-V") => {
            println!("herdr-hunks {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some(flag) if flag.starts_with('-') => {
            eprintln!("herdr-hunks: unknown option `{flag}` (expected: tui [PATH], open, open-split, update)");
            2
        }
        // Any other word is a path. The engine validates it, so a missing path shows the error state.
        Some(path) => herdr_hunks::tui::shell::run(PathBuf::from(path)),
    };
    std::process::exit(code);
}
