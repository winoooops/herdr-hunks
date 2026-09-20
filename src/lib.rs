#[cfg(not(unix))]
compile_error!("herdr-hunks targets Unix (macOS and Linux) only");

pub mod engine;
pub mod filesystem;
#[allow(dead_code)]
pub mod git;
pub mod runtime;
#[cfg(feature = "tui")]
pub mod tui;

#[cfg(test)]
mod git_diff_response_tests;
#[cfg(test)]
mod git_patches_tests;
