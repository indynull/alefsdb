//! Value algebra, canonical encoding, and database paths for alefsdb.

mod codec;
mod path;
mod value;

pub use codec::{decode, encode, CodecError, CODEC_VERSION};
pub use path::{DbPath, PathError};
pub use value::{Scalar, Value};
