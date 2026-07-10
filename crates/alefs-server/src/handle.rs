use crate::lock::DataLock;
use crate::protocol::{ListEntry, QueryHitDto, Request, Response};
use alefs_namespace::{Database, EntryKind, NsError, TxnOp};
use alefs_query::execute;
use alefs_storage::WalStorage;
use alefs_types::{DbPath, PathError, Scalar, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Shared single-writer database handle.
pub type DbHandle = Arc<Mutex<Database<WalStorage>>>;

#[derive(Debug)]
pub enum ServeError {
    Io(String),
    Internal(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Io(m) => write!(f, "i/o: {m}"),
            ServeError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ServeError {}

impl From<std::io::Error> for ServeError {
    fn from(e: std::io::Error) -> Self {
        ServeError::Io(e.to_string())
    }
}

#[derive(Default)]
pub struct ServerStats {
    pub requests: AtomicU64,
    pub mutations: AtomicU64,
    pub queries: AtomicU64,
    pub errors: AtomicU64,
    pub started: Mutex<Option<Instant>>,
}

impl ServerStats {
    pub fn new() -> Arc<Self> {
        let s = Arc::new(Self::default());
        *s.started.lock().unwrap() = Some(Instant::now());
        s
    }
}

pub struct Daemon {
    pub db: DbHandle,
    pub stats: Arc<ServerStats>,
    pub shutdown: Arc<AtomicBool>,
    _lock: DataLock,
    pub data_dir: PathBuf,
}

pub fn default_socket_path(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("alefs.sock")
}

pub fn open_db(data_dir: impl AsRef<Path>) -> Result<DbHandle, ServeError> {
    let store =
        WalStorage::open(data_dir.as_ref()).map_err(|e| ServeError::Internal(e.to_string()))?;
    let db = Database::open(store).map_err(|e| ServeError::Internal(e.to_string()))?;
    Ok(Arc::new(Mutex::new(db)))
}

pub fn open_daemon(data_dir: impl AsRef<Path>) -> Result<Daemon, ServeError> {
    let data_dir = data_dir.as_ref().to_path_buf();
    let lock = DataLock::acquire(&data_dir)?;
    let db = open_db(&data_dir)?;
    Ok(Daemon {
        db,
        stats: ServerStats::new(),
        shutdown: Arc::new(AtomicBool::new(false)),
        _lock: lock,
        data_dir,
    })
}

/// Bind Unix socket and serve until shutdown (locks parent of socket path).
/// Prefer [`open_daemon`] + [`serve_daemon`] for production so the lock is on the data dir.
pub fn serve_listener(db: DbHandle, socket_path: impl AsRef<Path>) -> Result<(), ServeError> {
    let parent = socket_path
        .as_ref()
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let daemon = Daemon {
        db,
        stats: ServerStats::new(),
        shutdown: Arc::new(AtomicBool::new(false)),
        _lock: DataLock::acquire(parent)?,
        data_dir: parent.to_path_buf(),
    };
    serve_daemon(daemon, socket_path)
}

pub fn serve_daemon(daemon: Daemon, socket_path: impl AsRef<Path>) -> Result<(), ServeError> {
    let socket_path = socket_path.as_ref().to_path_buf();
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    tracing::info!(path = %socket_path.display(), "listening");

    let shutdown = Arc::clone(&daemon.shutdown);
    install_signal_handlers(Arc::clone(&shutdown));

    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let db = Arc::clone(&daemon.db);
                let stats = Arc::clone(&daemon.stats);
                thread::spawn(move || {
                    if let Err(e) = handle_connection(db, stats, stream) {
                        tracing::warn!(error = %e, "connection error");
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::error!(error = %e, "accept error");
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    tracing::info!("shutting down");
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

fn install_signal_handlers(shutdown: Arc<AtomicBool>) {
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, shutdown);
}

pub fn request_shutdown(daemon: &Daemon) {
    daemon.shutdown.store(true, Ordering::SeqCst);
}

fn handle_connection(
    db: DbHandle,
    stats: Arc<ServerStats>,
    mut stream: UnixStream,
) -> Result<(), ServeError> {
    stream.set_nonblocking(false)?;
    while let Some(req) = read_message(&mut stream)? {
        let request: Request = match serde_json::from_slice(&req) {
            Ok(r) => r,
            Err(e) => {
                stats.errors.fetch_add(1, Ordering::Relaxed);
                let resp = Response::Err {
                    message: format!("bad request: {e}"),
                };
                write_message(&mut stream, &serde_json::to_vec(&resp).unwrap())?;
                continue;
            }
        };
        stats.requests.fetch_add(1, Ordering::Relaxed);
        let response = dispatch_with_stats(&db, &stats, request);
        if matches!(response, Response::Err { .. }) {
            stats.errors.fetch_add(1, Ordering::Relaxed);
        }
        let bytes =
            serde_json::to_vec(&response).map_err(|e| ServeError::Internal(e.to_string()))?;
        write_message(&mut stream, &bytes)?;
    }
    Ok(())
}

pub fn dispatch(db: &DbHandle, request: Request) -> Response {
    let stats = ServerStats::new();
    dispatch_with_stats(db, &stats, request)
}

pub fn dispatch_with_stats(db: &DbHandle, stats: &ServerStats, request: Request) -> Response {
    match &request {
        Request::Query { .. } => {
            stats.queries.fetch_add(1, Ordering::Relaxed);
        }
        Request::Ping
        | Request::Stats
        | Request::Get { .. }
        | Request::Ls { .. }
        | Request::Export => {}
        _ => {
            stats.mutations.fetch_add(1, Ordering::Relaxed);
        }
    }

    match request {
        Request::Ping => Response::Ok {
            message: "pong".into(),
        },
        Request::Stats => stats_response(db, stats),
        Request::Batch { ops } => apply_batch(db, ops),
        Request::Mkdir { path } => with_db_mut(db, |d| {
            let p = parse_path(&path)?;
            d.mkdir(&p).map_err(ns)?;
            Ok(Response::Ok {
                message: format!("ok {}", p.as_str()),
            })
        }),
        Request::Set {
            path,
            type_name,
            value,
        } => with_db_mut(db, |d| {
            let p = parse_path(&path)?;
            let v = parse_value(&type_name, &value)?;
            d.set(&p, v).map_err(ns)?;
            Ok(Response::Ok {
                message: format!("ok {}", p.as_str()),
            })
        }),
        Request::Get { path } => with_db(db, |d| {
            let p = parse_path(&path)?;
            let e = d.get(&p).map_err(ns)?;
            match e.kind {
                EntryKind::Directory => Ok(Response::Value {
                    type_name: "directory".into(),
                    display: p.as_str(),
                }),
                EntryKind::Value => {
                    let v = e.value.unwrap();
                    Ok(Response::Value {
                        type_name: v.typename().into(),
                        display: format_value(&v),
                    })
                }
            }
        }),
        Request::Ls { path } => with_db(db, |d| {
            let p = parse_path(&path)?;
            let mut entries = Vec::new();
            for (name, kind) in d.list(&p).map_err(ns)? {
                entries.push(ListEntry {
                    name,
                    kind: match kind {
                        EntryKind::Directory => "dir".into(),
                        EntryKind::Value => "val".into(),
                    },
                });
            }
            Ok(Response::List { entries })
        }),
        Request::Rm { path } => with_db_mut(db, |d| {
            let p = parse_path(&path)?;
            d.delete(&p).map_err(ns)?;
            Ok(Response::Ok {
                message: format!("ok {}", p.as_str()),
            })
        }),
        Request::Hset {
            path,
            key,
            type_name,
            value,
        } => with_db_mut(db, |d| {
            let p = parse_path(&path)?;
            let v = parse_value(&type_name, &value)?;
            d.hash_set(&p, &key, v).map_err(ns)?;
            Ok(Response::Ok {
                message: "ok".into(),
            })
        }),
        Request::Lpush {
            path,
            type_name,
            value,
        } => with_db_mut(db, |d| {
            let p = parse_path(&path)?;
            let v = parse_value(&type_name, &value)?;
            d.list_push(&p, v).map_err(ns)?;
            Ok(Response::Ok {
                message: "ok".into(),
            })
        }),
        Request::Sadd {
            path,
            type_name,
            value,
        } => with_db_mut(db, |d| {
            let p = parse_path(&path)?;
            let v = parse_value(&type_name, &value)?;
            d.set_add(&p, v).map_err(ns)?;
            Ok(Response::Ok {
                message: "ok".into(),
            })
        }),
        Request::Tset {
            path,
            key,
            type_name,
            value,
        } => with_db_mut(db, |d| {
            let p = parse_path(&path)?;
            let sk = if let Ok(n) = key.parse::<i64>() {
                Scalar::Int(n)
            } else {
                Scalar::String(key)
            };
            let v = parse_value(&type_name, &value)?;
            d.tree_set(&p, sk, v).map_err(ns)?;
            Ok(Response::Ok {
                message: "ok".into(),
            })
        }),
        Request::Query { query } => with_db(db, |d| {
            let hits = execute(d, &query).map_err(|e| e.to_string())?;
            Ok(Response::Query {
                hits: hits
                    .into_iter()
                    .map(|h| QueryHitDto {
                        path: h.path.as_str(),
                        type_name: h.type_name,
                    })
                    .collect(),
            })
        }),
        Request::Compact => with_db_mut(db, |d| {
            d.store_mut().compact().map_err(|e| e.to_string())?;
            Ok(Response::Ok {
                message: "ok compacted".into(),
            })
        }),
        Request::Export => with_db(db, |d| {
            let json = d.export_json().map_err(ns)?;
            Ok(Response::Export { json })
        }),
        Request::Import { json } => with_db_mut(db, |d| {
            d.import_json(&json).map_err(ns)?;
            Ok(Response::Ok {
                message: "ok imported".into(),
            })
        }),
    }
}

fn stats_response(db: &DbHandle, stats: &ServerStats) -> Response {
    let uptime_sec = stats
        .started
        .lock()
        .unwrap()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    let keys_approx = db.lock().map(|g| g.store().len() as u64).unwrap_or(0);
    Response::Stats {
        uptime_sec,
        requests: stats.requests.load(Ordering::Relaxed),
        mutations: stats.mutations.load(Ordering::Relaxed),
        queries: stats.queries.load(Ordering::Relaxed),
        errors: stats.errors.load(Ordering::Relaxed),
        keys_approx,
    }
}

fn apply_batch(db: &DbHandle, ops: Vec<Request>) -> Response {
    with_db_mut(db, |d| {
        let mut txn = Vec::new();
        for op in ops {
            txn.push(request_to_txn(op)?);
        }
        d.apply_txn(&txn).map_err(ns)?;
        Ok(Response::Ok {
            message: format!("ok batch {}", txn.len()),
        })
    })
}

fn request_to_txn(op: Request) -> Result<TxnOp, String> {
    match op {
        Request::Mkdir { path } => Ok(TxnOp::Mkdir {
            path: parse_path(&path)?,
        }),
        Request::Set {
            path,
            type_name,
            value,
        } => Ok(TxnOp::Set {
            path: parse_path(&path)?,
            value: parse_value(&type_name, &value)?,
        }),
        Request::Rm { path } => Ok(TxnOp::Delete {
            path: parse_path(&path)?,
        }),
        Request::Hset {
            path,
            key,
            type_name,
            value,
        } => Ok(TxnOp::HashSet {
            path: parse_path(&path)?,
            key,
            value: parse_value(&type_name, &value)?,
        }),
        Request::Lpush {
            path,
            type_name,
            value,
        } => Ok(TxnOp::ListPush {
            path: parse_path(&path)?,
            value: parse_value(&type_name, &value)?,
        }),
        Request::Sadd {
            path,
            type_name,
            value,
        } => Ok(TxnOp::SetAdd {
            path: parse_path(&path)?,
            value: parse_value(&type_name, &value)?,
        }),
        Request::Tset {
            path,
            key,
            type_name,
            value,
        } => {
            let sk = if let Ok(n) = key.parse::<i64>() {
                Scalar::Int(n)
            } else {
                Scalar::String(key)
            };
            Ok(TxnOp::TreeSet {
                path: parse_path(&path)?,
                key: sk,
                value: parse_value(&type_name, &value)?,
            })
        }
        other => Err(format!("op not allowed in batch: {other:?}")),
    }
}

fn ns(e: NsError) -> String {
    e.to_string()
}

fn parse_path(path: &str) -> Result<DbPath, String> {
    DbPath::parse(path).map_err(|e: PathError| e.to_string())
}

fn with_db<F>(db: &DbHandle, f: F) -> Response
where
    F: FnOnce(&Database<WalStorage>) -> Result<Response, String>,
{
    let guard = match db.lock() {
        Ok(g) => g,
        Err(_) => {
            return Response::Err {
                message: "lock poisoned".into(),
            }
        }
    };
    match f(&guard) {
        Ok(r) => r,
        Err(e) => Response::Err { message: e },
    }
}

fn with_db_mut<F>(db: &DbHandle, f: F) -> Response
where
    F: FnOnce(&mut Database<WalStorage>) -> Result<Response, String>,
{
    let mut guard = match db.lock() {
        Ok(g) => g,
        Err(_) => {
            return Response::Err {
                message: "lock poisoned".into(),
            }
        }
    };
    match f(&mut guard) {
        Ok(r) => r,
        Err(e) => Response::Err { message: e },
    }
}

fn parse_value(ty: &str, raw: &str) -> Result<Value, String> {
    match ty {
        "null" => Ok(Value::null()),
        "bool" => Ok(Value::bool(raw.parse().map_err(|e| format!("{e}"))?)),
        "int" => Ok(Value::int(raw.parse().map_err(|e| format!("{e}"))?)),
        "float" => Ok(Value::float(raw.parse().map_err(|e| format!("{e}"))?)),
        "string" => Ok(Value::string(raw)),
        "bytes" => Ok(Value::bytes(raw.as_bytes().to_vec())),
        "hash" => Ok(Value::Hash(BTreeMap::new())),
        "list" => Ok(Value::List(Vec::new())),
        "set" => Ok(Value::Set(Vec::new())),
        "tree" => Ok(Value::Tree(BTreeMap::new())),
        other => Err(format!("unknown type {other}")),
    }
}

fn format_value(v: &Value) -> String {
    match v {
        Value::Scalar(Scalar::Null) => "null".into(),
        Value::Scalar(Scalar::Bool(b)) => b.to_string(),
        Value::Scalar(Scalar::Int(n)) => n.to_string(),
        Value::Scalar(Scalar::Float(bits)) => f64::from_bits(*bits).to_string(),
        Value::Scalar(Scalar::String(s)) => s.clone(),
        Value::Scalar(Scalar::Bytes(b)) => format!("bytes[{}]", b.len()),
        Value::Hash(m) => {
            let keys: Vec<_> = m.keys().cloned().collect();
            format!("hash{{{}}}", keys.join(","))
        }
        Value::Set(m) => format!("set({})", m.len()),
        Value::List(m) => format!("list[{}]", m.len()),
        Value::Tree(m) => format!("tree({{{}}})", m.len()),
    }
}

pub fn read_message(stream: &mut impl Read) -> Result<Option<Vec<u8>>, ServeError> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 64 * 1024 * 1024 {
        return Err(ServeError::Internal("message too large".into()));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(Some(buf))
}

pub fn write_message(stream: &mut impl Write, payload: &[u8]) -> Result<(), ServeError> {
    let len = (payload.len() as u32).to_le_bytes();
    stream.write_all(&len)?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Request;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "alefs-srv-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn dispatch_mkdir_set_get() {
        let dir = tmp();
        let db = open_db(&dir).unwrap();
        let r = dispatch(&db, Request::Mkdir { path: "/a".into() });
        assert!(matches!(r, Response::Ok { .. }), "{r:?}");
        let r = dispatch(
            &db,
            Request::Set {
                path: "/a/x".into(),
                type_name: "string".into(),
                value: "hi".into(),
            },
        );
        assert!(matches!(r, Response::Ok { .. }), "{r:?}");
        let r = dispatch(
            &db,
            Request::Get {
                path: "/a/x".into(),
            },
        );
        match r {
            Response::Value { type_name, display } => {
                assert_eq!(type_name, "string");
                assert_eq!(display, "hi");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_atomic() {
        let dir = tmp();
        let db = open_db(&dir).unwrap();
        let r = dispatch(
            &db,
            Request::Batch {
                ops: vec![
                    Request::Mkdir { path: "/b".into() },
                    Request::Set {
                        path: "/b/x".into(),
                        type_name: "int".into(),
                        value: "9".into(),
                    },
                ],
            },
        );
        assert!(matches!(r, Response::Ok { .. }), "{r:?}");
        match dispatch(
            &db,
            Request::Get {
                path: "/b/x".into(),
            },
        ) {
            Response::Value { display, .. } => assert_eq!(display, "9"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn framing_roundtrip() {
        let mut buf = Vec::new();
        write_message(&mut buf, b"hello").unwrap();
        let mut cur = std::io::Cursor::new(buf);
        let msg = read_message(&mut cur).unwrap().unwrap();
        assert_eq!(msg, b"hello");
    }

    #[test]
    fn flock_exclusive() {
        let dir = tmp();
        let _a = DataLock::acquire(&dir).unwrap();
        let b = DataLock::acquire(&dir);
        assert!(b.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
