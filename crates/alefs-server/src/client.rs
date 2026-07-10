use crate::handle::{read_message, write_message, ServeError};
use crate::protocol::{Request, Response};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum ClientError {
    Io(String),
    Protocol(String),
    Remote(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Io(m) => write!(f, "client i/o: {m}"),
            ClientError::Protocol(m) => write!(f, "protocol: {m}"),
            ClientError::Remote(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<ServeError> for ClientError {
    fn from(e: ServeError) -> Self {
        ClientError::Io(e.to_string())
    }
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Io(e.to_string())
    }
}

/// One-shot RPC: connect, send request, read response, disconnect.
pub fn rpc_call(socket: impl AsRef<Path>, request: Request) -> Result<Response, ClientError> {
    let mut c = Client::connect(socket)?;
    c.call(request)
}

/// Persistent connection for multi-request sessions (bench, tooling).
pub struct Client {
    stream: UnixStream,
    path: PathBuf,
}

impl Client {
    pub fn connect(socket: impl AsRef<Path>) -> Result<Self, ClientError> {
        let path = socket.as_ref().to_path_buf();
        let stream = UnixStream::connect(&path)?;
        Ok(Self { stream, path })
    }

    pub fn call(&mut self, request: Request) -> Result<Response, ClientError> {
        let payload =
            serde_json::to_vec(&request).map_err(|e| ClientError::Protocol(e.to_string()))?;
        write_message(&mut self.stream, &payload)?;
        let resp_bytes = read_message(&mut self.stream)?
            .ok_or_else(|| ClientError::Protocol("server closed without response".into()))?;
        let response: Response = serde_json::from_slice(&resp_bytes)
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        Ok(response)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handle::{default_socket_path, open_daemon, serve_daemon};
    use crate::protocol::Request;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn persistent_client_multi_call() {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "alefs-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = default_socket_path(&dir);
        let daemon = open_daemon(&dir).unwrap();
        let shutdown = std::sync::Arc::clone(&daemon.shutdown);
        let sock_c = sock.clone();
        let handle = thread::spawn(move || {
            let _ = serve_daemon(daemon, sock_c);
        });
        for _ in 0..50 {
            if sock.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let mut c = Client::connect(&sock).unwrap();
        let r = c
            .call(Request::Set {
                path: "/x".into(),
                type_name: "int".into(),
                value: "7".into(),
            })
            .unwrap();
        assert!(matches!(r, Response::Ok { .. }), "{r:?}");
        let r = c.call(Request::Get { path: "/x".into() }).unwrap();
        match r {
            Response::Value { display, .. } => assert_eq!(display, "7"),
            other => panic!("{other:?}"),
        }
        let r = c.call(Request::Stats).unwrap();
        assert!(matches!(r, Response::Stats { .. }), "{r:?}");
        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        thread::sleep(Duration::from_millis(100));
        let _ = handle.join();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
