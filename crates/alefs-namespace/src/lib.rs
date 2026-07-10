//! Hierarchical namespace: directories and typed values.

mod db;
mod error;
mod keys;
mod txn;

pub use db::{Database, Entry, EntryKind};
pub use error::NsError;
pub use txn::TxnOp;
