//! Sysfs and procfs reading helpers with test fixture path support.

use std::fs;
use std::path::{Path, PathBuf};

/// Environment-aware or fixture-aware path provider for procfs and sysfs.
#[derive(Debug, Clone)]
pub struct FsPathProvider {
    pub proc_root: PathBuf,
    pub sys_root: PathBuf,
}

impl Default for FsPathProvider {
    fn default() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            sys_root: PathBuf::from("/sys"),
        }
    }
}

impl FsPathProvider {
    pub fn new(proc_root: impl Into<PathBuf>, sys_root: impl Into<PathBuf>) -> Self {
        Self {
            proc_root: proc_root.into(),
            sys_root: sys_root.into(),
        }
    }

    pub fn proc_path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.proc_root.join(rel)
    }

    pub fn sys_path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.sys_root.join(rel)
    }

    /// Read file content as trimmed string. Returns None if missing or unreadable.
    pub fn read_string(&self, path: &Path) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    /// Read file and parse as integer. Returns None if missing or unparseable.
    pub fn read_int<T: std::str::FromStr>(&self, path: &Path) -> Option<T> {
        let content = self.read_string(path)?;
        content.parse::<T>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_read_missing_returns_none() {
        let provider = FsPathProvider::default();
        let val = provider.read_string(&PathBuf::from("/nonexistent/file"));
        assert!(val.is_none());
    }

    #[test]
    fn test_read_fixture() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("capacity");
        fs::write(&file_path, " 88 \n").unwrap();

        let provider = FsPathProvider::new(dir.path(), dir.path());
        let val: Option<u32> = provider.read_int(&file_path);
        assert_eq!(val, Some(88));
    }
}
