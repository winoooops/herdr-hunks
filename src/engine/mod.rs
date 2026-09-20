//! UI-agnostic engine. No ratatui or crossterm types appear here.
pub mod gitver;
pub mod nav;
pub mod session;
pub mod types;
pub use session::{spawn, EngineHandle, SessionConfig};
pub use types::*;

/// D3. The frozen tree spawns git itself, so policy is process-wide.
pub const GIT_CHILD_ENV: [(&str, &str); 3] = [
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_LITERAL_PATHSPECS", "1"),
    ("GIT_NO_LAZY_FETCH", "1"),
];

/// Call once, before any thread starts. Embedders must call it too.
pub fn init_process_env() {
    for (key, value) in GIT_CHILD_ENV {
        std::env::set_var(key, value);
    }
    std::env::remove_var("GIT_EXTERNAL_DIFF");
}

#[cfg(test)]
mod tests {
    // Environment mutation is tested in isolated processes, never in libtest threads.
    #[test]
    fn the_policy_names_the_three_variables() {
        let keys: Vec<&str> = super::GIT_CHILD_ENV.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "GIT_OPTIONAL_LOCKS",
                "GIT_LITERAL_PATHSPECS",
                "GIT_NO_LAZY_FETCH"
            ]
        );
    }
}
