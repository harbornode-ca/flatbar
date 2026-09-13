//! Singleton enforcement using flock per config name.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SingletonError {
    #[error("Could not determine XDG_RUNTIME_DIR or runtime directory")]
    RuntimeDirUnavailable,
    #[error("Failed to create lock file directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("Another instance of flatbar named '{0}' is already running")]
    AlreadyRunning(String),
}

/// A file lock guard that unlocks when dropped.
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Try to acquire an exclusive non-blocking lock for the given bar instance name.
    pub fn acquire(name: &str) -> Result<Self, SingletonError> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        let lock_dir = runtime_dir.join("flatbar");
        fs::create_dir_all(&lock_dir)?;

        let lock_path = lock_dir.join(format!("{name}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?;

        let fd = file.as_raw_fd();
        // flock(fd, LOCK_EX | LOCK_NB)
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK)
                || err.raw_os_error() == Some(libc::EAGAIN)
            {
                return Err(SingletonError::AlreadyRunning(name.to_string()));
            }
            return Err(SingletonError::Io(err));
        }

        Ok(Self {
            _file: file,
            path: lock_path,
        })
    }

    pub fn lock_path(&self) -> &PathBuf {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_singleton_lock() {
        let name = "test-bar-singleton";
        let lock1 = InstanceLock::acquire(name).expect("First lock should succeed");
        let lock2 = InstanceLock::acquire(name);
        assert!(matches!(lock2, Err(SingletonError::AlreadyRunning(_))));
        drop(lock1);
        let lock3 = InstanceLock::acquire(name);
        assert!(lock3.is_ok());
    }
}
