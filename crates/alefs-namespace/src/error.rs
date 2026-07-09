use alefs_storage::StorageError;
use alefs_types::{CodecError, PathError};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NsError {
    Path(PathError),
    Codec(String),
    Storage(String),
    NotFound(String),
    AlreadyExists(String),
    NotDirectory(String),
    IsDirectory(String),
    ParentMissing(String),
    TypeMismatch(String),
    Invalid(String),
}

impl fmt::Display for NsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NsError::Path(e) => write!(f, "{e}"),
            NsError::Codec(e) => write!(f, "codec: {e}"),
            NsError::Storage(e) => write!(f, "storage: {e}"),
            NsError::NotFound(p) => write!(f, "not found: {p}"),
            NsError::AlreadyExists(p) => write!(f, "already exists: {p}"),
            NsError::NotDirectory(p) => write!(f, "not a directory: {p}"),
            NsError::IsDirectory(p) => write!(f, "is a directory: {p}"),
            NsError::ParentMissing(p) => write!(f, "parent missing: {p}"),
            NsError::TypeMismatch(m) => write!(f, "type mismatch: {m}"),
            NsError::Invalid(m) => write!(f, "invalid: {m}"),
        }
    }
}

impl std::error::Error for NsError {}

impl From<PathError> for NsError {
    fn from(e: PathError) -> Self {
        NsError::Path(e)
    }
}

impl From<CodecError> for NsError {
    fn from(e: CodecError) -> Self {
        NsError::Codec(e.to_string())
    }
}

impl From<StorageError> for NsError {
    fn from(e: StorageError) -> Self {
        NsError::Storage(e.to_string())
    }
}
