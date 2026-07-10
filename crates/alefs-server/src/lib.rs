//! Single-writer daemon and Unix-socket RPC for alefsdb.

mod client;
mod handle;
mod lock;
mod protocol;

pub use client::{rpc_call, Client, ClientError};
pub use handle::{
    default_socket_path, dispatch, dispatch_with_stats, open_daemon, open_db, read_message,
    request_shutdown, serve_daemon, serve_listener, write_message, Daemon, DbHandle, ServeError,
    ServerStats,
};
pub use protocol::{Request, Response};
