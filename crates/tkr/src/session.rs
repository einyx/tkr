use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

/// Live session events for `tkr watch` (Unix domain socket). No-op on Windows.
#[derive(Clone)]
pub struct Session {
    #[cfg(unix)]
    inner: Arc<Mutex<Option<UnixStream>>>,
}

impl Session {
    pub fn connect(path: &str) -> Self {
        #[cfg(unix)]
        {
            let stream = UnixStream::connect(path).ok();
            Self {
                inner: Arc::new(Mutex::new(stream)),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Self
        }
    }

    pub fn emit(&self, event: serde_json::Value) {
        #[cfg(unix)]
        {
            if let Ok(mut guard) = self.inner.lock() {
                if let Some(ref mut stream) = *guard {
                    let mut line = event.to_string();
                    line.push('\n');
                    if stream.write_all(line.as_bytes()).is_err() {
                        *guard = None;
                    }
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = event;
        }
    }

    pub fn command_start(&self, cmd: &str, args: &str) {
        self.emit(json!({"event":"command_start","cmd":cmd,"args":args,"ts":now_secs()}));
    }

    pub fn line_suppressed(&self, reason: &str, score: f32) {
        self.emit(json!({"event":"line_suppressed","reason":reason,"score":score}));
    }

    pub fn command_end(&self, tokens_in: u64, tokens_saved: u64) {
        self.emit(json!({"event":"command_end","tokens_in":tokens_in,"tokens_saved":tokens_saved,"ts":now_secs()}));
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_no_ops_when_socket_missing() {
        let session = Session::connect("/tmp/__tkr_no_such_socket_xyz");
        session.command_start("git", "status");
        session.command_end(100, 50);
    }
}
