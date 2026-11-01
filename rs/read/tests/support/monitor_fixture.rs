//! Shared helpers for the local monitor tests: scratch directories and repository paths.
#![allow(dead_code)]
use std::path::{Path, PathBuf};

/// One private scratch directory, removed with its contents.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "verdaxis-monitor-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&path).expect("create a scratch directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Names of the entries in the directory, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .expect("list the scratch directory")
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The repository root (the directory that holds `rs/` and `deploy/`).
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Static inputs shared by the monitor tests.
pub fn contract_fixture(relative: &str) -> PathBuf {
    repository_root()
        .join("tests/rust_contract/phase10")
        .join(relative)
}
