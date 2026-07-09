//! Single-writer daemon and Unix-socket RPC for alefsdb.

mod client;
mod handle;
mod protocol;

pub use client::{rpc_call, ClientError};
pub use handle::{
    default_socket_path, open_db, read_message, serve_listener, write_message, DbHandle, ServeError,
};
pub use protocol::{Request, Response};
