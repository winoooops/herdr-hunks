use herdr_hunks::engine::host::from_env;

// This binary isolates environment mutation from other tests.
#[test]
fn from_env_follows_the_socket_path_variable() {
    std::env::remove_var("HERDR_SOCKET_PATH");
    assert!(
        from_env().is_none(),
        "no path, no host: every pane target is NoHost"
    );
    std::env::set_var("HERDR_SOCKET_PATH", "/run/nowhere/herdr.sock");
    assert!(
        from_env().is_some(),
        "a path is enough here; the socket is opened per call"
    );
}
