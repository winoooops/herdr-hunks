use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// One request may take this long, connect to complete reply.
pub const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum HerdrClientError {
    /// Before anything was written: no socket file, a refused connection, a connect that did
    /// not complete by the deadline.
    #[error("herdr socket unavailable at {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The request line was not written in full.
    #[error("could not write to herdr: {0}")]
    Write(std::io::Error),
    /// The line was written in full and no complete reply arrived by the deadline.
    #[error("no complete reply from herdr: {0}")]
    Read(std::io::Error),
    #[error("herdr returned malformed json: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("herdr error response: {0}")]
    Api(String),
}

pub fn decode_response(line: &str) -> Result<Value, HerdrClientError> {
    let value: Value = serde_json::from_str(line)?;
    if let Some(error) = value.get("error") {
        return Err(HerdrClientError::Api(error.to_string()));
    }
    Ok(value)
}

pub struct HerdrClient {
    socket_path: PathBuf,
    next_id: AtomicU64,
}

fn timed_out() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::TimedOut, "the deadline passed")
}

/// Time left before `deadline`, or an error once it has passed.
fn left(deadline: Instant) -> std::io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(timed_out())
    } else {
        Ok(left)
    }
}

/// A connect that cannot outlive the deadline: `UnixStream::connect` blocks while the host's
/// listen backlog is full, so the socket is made non-blocking and retried or polled instead.
fn connect_within(path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "socket path too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let length = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    loop {
        left(deadline)?;
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Owned from here: dropped on every early return below.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        stream.set_nonblocking(true)?;
        let connected = unsafe {
            libc::connect(
                stream.as_raw_fd(),
                &address as *const libc::sockaddr_un as *const libc::sockaddr,
                length as libc::socklen_t,
            )
        };
        if connected != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EAGAIN) {
                drop(stream);
                std::thread::sleep(Duration::from_millis(10).min(left(deadline)?));
                continue;
            }
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            let mut poll = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let wait = left(deadline)?.as_millis().min(i32::MAX as u128) as libc::c_int;
            let ready = unsafe { libc::poll(&mut poll, 1, wait) };
            if ready < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if ready == 0 {
                return Err(timed_out());
            }
            // The pending connect has an outcome now; read it the way std does.
            if let Some(error) = stream.take_error()? {
                return Err(error);
            }
        }
        stream.set_nonblocking(false)?;
        return Ok(stream);
    }
}

impl HerdrClient {
    pub fn from_env() -> Self {
        let socket_path = std::env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
                    .unwrap_or_else(|| PathBuf::from(".config"))
                    .join("herdr/herdr.sock")
            });
        Self::new(socket_path)
    }

    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// One request per connection, one deadline for the whole exchange: every timeout below is
    /// the time the deadline has left, and the read loop stops at it however many bytes have
    /// trickled in. The deadline is one part of the incumbent daemon's worst-case shutdown,
    /// which the eight-second singleton takeover deadline must clear.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, HerdrClientError> {
        let deadline = Instant::now() + DEADLINE;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let request = json!({ "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        let mut stream = connect_within(&self.socket_path, deadline).map_err(|source| {
            HerdrClientError::Connect {
                path: self.socket_path.clone(),
                source,
            }
        })?;
        let mut written = 0;
        while written < line.len() {
            stream
                .set_write_timeout(Some(left(deadline).map_err(HerdrClientError::Write)?))
                .map_err(HerdrClientError::Write)?;
            match stream.write(&line.as_bytes()[written..]) {
                Ok(0) => {
                    return Err(HerdrClientError::Write(
                        std::io::ErrorKind::WriteZero.into(),
                    ))
                }
                Ok(n) => written += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(HerdrClientError::Write(e)),
            }
        }
        let mut reply = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream
                .set_read_timeout(Some(left(deadline).map_err(HerdrClientError::Read)?))
                .map_err(HerdrClientError::Read)?;
            match stream.read(&mut byte) {
                Ok(0) => {
                    return Err(HerdrClientError::Read(
                        std::io::ErrorKind::UnexpectedEof.into(),
                    ))
                }
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => reply.push(byte[0]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(HerdrClientError::Read(e)),
            }
            if reply.len() > 1 << 20 {
                return Err(HerdrClientError::Read(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "reply longer than a megabyte",
                )));
            }
        }
        decode_response(&String::from_utf8_lossy(&reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_captured_ping_success() {
        let line = include_str!("../../tests/fixtures/ping-response.json");
        let value = decode_response(line).expect("ping fixture decodes as success");
        assert_eq!(value["id"], "probe-1");
    }

    #[test]
    fn error_response_maps_to_api_error() {
        let line = r#"{"id":"x","error":{"code":"unknown_method","message":"nope"}}"#;
        match decode_response(line) {
            Err(HerdrClientError::Api(message)) => {
                assert!(message.contains("unknown_method"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    /// A server playing one script, on a fresh socket.
    fn server(
        script: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
    ) -> (tempfile::TempDir, HerdrClient) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                script(stream);
            }
        });
        (dir, HerdrClient::new(path))
    }

    #[test]
    fn a_missing_socket_and_a_refused_connection_fail_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = HerdrClient::new(dir.path().join("none.sock"));
        assert!(matches!(
            missing.request("ping", json!({})),
            Err(HerdrClientError::Connect { .. })
        ));
        // A path that exists but nobody listens on.
        let stale = dir.path().join("stale.sock");
        drop(UnixListener::bind(&stale).unwrap());
        let refused = HerdrClient::new(stale);
        let started = Instant::now();
        assert!(matches!(
            refused.request("ping", json!({})),
            Err(HerdrClientError::Connect { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_full_listen_backlog_retries_until_the_server_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.sock");
        let listener = UnixListener::bind(&path).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        // Linux queues backlog + 1 connections before returning EAGAIN.
        let mut queued = [
            UnixStream::connect(&path).unwrap(),
            UnixStream::connect(&path).unwrap(),
        ];
        for stream in &mut queued {
            stream.write_all(b"{\"id\":\"queued\"}\n").unwrap();
        }
        let started = Instant::now();
        let server = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(DEADLINE)).unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let reply = json!({ "id": request["id"], "result": { "type": "pong" } });
                stream.write_all(format!("{reply}\n").as_bytes()).unwrap();
            }
        });
        let result = HerdrClient::new(path).request("ping", json!({}));
        let took = started.elapsed();
        assert!(result.is_ok(), "{result:?} after {took:?}");
        assert_eq!(result.unwrap()["result"]["type"], "pong");
        assert!(
            took >= Duration::from_millis(200) && took < DEADLINE,
            "{took:?}"
        );
        server.join().unwrap();
    }

    #[test]
    fn a_server_that_reads_and_never_answers_fails_after_writing_at_the_deadline() {
        let (_dir, client) = server(|stream| {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            std::thread::sleep(Duration::from_secs(8));
        });
        let started = Instant::now();
        let error = client.request("ping", json!({})).unwrap_err();
        assert!(matches!(error, HerdrClientError::Read(_)), "{error:?}");
        let took = started.elapsed();
        assert!(
            took >= DEADLINE && took < DEADLINE + Duration::from_secs(1),
            "{took:?}"
        );
    }

    #[test]
    fn a_byte_every_two_seconds_still_fails_at_the_deadline() {
        let (_dir, client) = server(|mut stream| {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            // Never a newline: the reply is never complete.
            for b in br#"{"id":"1","result":{}}"# {
                if stream.write_all(&[*b]).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        let started = Instant::now();
        let result = client.request("ping", json!({}));
        let took = started.elapsed();
        // A per-call socket timeout alone would run on for 2 s × 22 bytes.
        assert!(took < DEADLINE + Duration::from_secs(1), "{took:?}");
        let error = result.unwrap_err();
        assert!(matches!(error, HerdrClientError::Read(_)), "{error:?}");
    }

    #[test]
    fn a_server_that_never_reads_fails_the_write_at_the_deadline() {
        // Four megabytes fill the socket buffer while the peer never reads.
        let (_dir, client) = server(|_stream| std::thread::sleep(Duration::from_secs(8)));
        let big = "x".repeat(4 << 20);
        let started = Instant::now();
        let error = client
            .request("agent.prompt", json!({ "target": "w1:p2", "text": big }))
            .unwrap_err();
        assert!(matches!(error, HerdrClientError::Write(_)), "{error:?}");
        let took = started.elapsed();
        assert!(
            took >= DEADLINE && took < DEADLINE + Duration::from_secs(1),
            "{took:?}"
        );
    }

    #[test]
    fn a_complete_reply_within_the_deadline_decodes() {
        let (_dir, client) = server(|mut stream| {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            let request: Value = serde_json::from_str(&line).unwrap();
            let reply = json!({ "id": request["id"], "result": { "type": "pong" } });
            std::thread::sleep(Duration::from_millis(300));
            let _ = stream.write_all(format!("{reply}\n").as_bytes());
        });
        let value = client.request("ping", json!({})).unwrap();
        assert_eq!(value["result"]["type"], "pong");
    }
}
