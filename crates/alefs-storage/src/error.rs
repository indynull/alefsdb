use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    Io(String),
    Corrupt(String),
    Internal(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Io(m) => write!(f, "storage i/o: {m}"),
            StorageError::Corrupt(m) => write!(f, "storage corrupt: {m}"),
            StorageError::Internal(m) => write!(f, "storage internal: {m}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        StorageError::Io(e.to_string())
    }
}
