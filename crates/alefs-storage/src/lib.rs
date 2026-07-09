//! Durable storage abstraction for alefsdb.

mod batch;
mod error;
mod memory;
mod wal;

pub use batch::{BatchOp, WriteBatch};
pub use error::StorageError;
pub use memory::MemoryStorage;
pub use wal::WalStorage;

/// Ordered key/value pairs from a prefix scan.
pub type ScanRows = Vec<(Vec<u8>, Vec<u8>)>;

/// Byte-oriented key-value storage used by the namespace layer.
pub trait Storage {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError>;

    /// Apply all ops durably (S1+) or atomically in-process (S0 memory).
    fn commit(&mut self, batch: WriteBatch) -> Result<(), StorageError>;

    /// Return key-value pairs whose keys start with `prefix`, ordered by key.
    fn scan_prefix(&self, prefix: &[u8]) -> Result<ScanRows, StorageError>;
}
