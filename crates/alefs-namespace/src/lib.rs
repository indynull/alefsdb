//! Hierarchical namespace: directories and typed values.

mod db;
mod error;
mod keys;

pub use db::{Database, Entry, EntryKind};
pub use error::NsError;
