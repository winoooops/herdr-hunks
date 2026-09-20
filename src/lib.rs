#[cfg(not(unix))]
compile_error!("herdr-hunks targets Unix (macOS and Linux) only");

pub mod filesystem;
#[allow(dead_code)]
pub mod git;
pub mod runtime;
