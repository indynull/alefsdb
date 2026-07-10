//! Exclusive flock on the data directory.

use crate::handle::ServeError;
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub struct DataLock {
    _file: File,
    path: PathBuf,
}

impl DataLock {
    pub fn acquire(data_dir: impl AsRef<Path>) -> Result<Self, ServeError> {
        let data_dir = data_dir.as_ref();
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("LOCK");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(ServeError::Internal(format!(
                "data directory already locked by another process ({})",
                path.display()
            )));
        }
        Ok(Self { _file: file, path })
    }
}

impl Drop for DataLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self._file.as_raw_fd(), libc::LOCK_UN) };
        let _ = std::fs::remove_file(&self.path);
    }
}
