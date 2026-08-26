//! Model management: manifest types, SHA-256 verification, resumable HTTPS download, disk-space
//! checks, atomic promotion into the cache, and cache-state inspection.
//!
//! No downloaded file is ever executed; only data files (ONNX, vocab, context binaries) are
//! fetched, and each is verified against a pinned SHA-256 before use.

mod download;
mod manifest;

pub use download::{DownloadEvent, ModelDownloader, Progress};
pub use manifest::{ArtifactSet, ArtifactTarget, FileEntry, ModelManifest};

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// Compute the lowercase-hex SHA-256 of a file, reading in 1 MiB chunks.
pub fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| Error::io(path.display().to_string(), e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| Error::io(path.display().to_string(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verify a file's size and SHA-256 against a manifest entry.
pub fn verify_file(dir: &Path, entry: &FileEntry) -> Result<()> {
    let path = dir.join(&entry.path);
    let meta = std::fs::metadata(&path).map_err(|e| Error::io(path.display().to_string(), e))?;
    if meta.len() != entry.bytes {
        return Err(Error::Model(format!(
            "size mismatch for {}: expected {}, got {}",
            entry.path,
            entry.bytes,
            meta.len()
        )));
    }
    let actual = sha256_file(&path)?;
    if !actual.eq_ignore_ascii_case(&entry.sha256) {
        return Err(Error::Integrity {
            file: path,
            expected: entry.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

/// The state of a model in the local cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheState {
    /// Not present.
    Missing,
    /// Present and all files match size (fast check, no hashing).
    Installed,
    /// Present but some files are missing or the wrong size.
    Incomplete,
}

/// Resolves cache paths and inspects/verifies installed models.
pub struct ModelRegistry {
    /// Root directory holding one subdir per model.
    root: PathBuf,
}

impl ModelRegistry {
    /// New registry rooted at `root` (e.g. `%LOCALAPPDATA%/LocalWisper/models`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Directory for a given model.
    pub fn model_dir(&self, manifest: &ModelManifest) -> PathBuf {
        self.root.join(&manifest.local_dir)
    }

    /// Fast cache-state check (size only) for the given files.
    pub fn state(&self, manifest: &ModelManifest, files: &[FileEntry]) -> CacheState {
        let dir = self.model_dir(manifest);
        if !dir.exists() {
            return CacheState::Missing;
        }
        let mut any = false;
        for f in files {
            any = true;
            match std::fs::metadata(dir.join(&f.path)) {
                Ok(m) if m.len() == f.bytes => {}
                _ => return CacheState::Incomplete,
            }
        }
        if any {
            CacheState::Installed
        } else {
            CacheState::Missing
        }
    }

    /// Full verification (size + SHA-256) of the given files.
    pub fn verify(&self, manifest: &ModelManifest, files: &[FileEntry]) -> Result<()> {
        let dir = self.model_dir(manifest);
        for f in files {
            verify_file(&dir, f)?;
        }
        Ok(())
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, data: &[u8]) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::File::create(path).unwrap().write_all(data).unwrap();
    }

    #[test]
    fn sha256_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write(&p, b"abc");
        // SHA-256("abc")
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn verify_detects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("f"), b"hello");
        let entry = FileEntry {
            path: "f".into(),
            url: "https://x/f".into(),
            bytes: 5,
            sha256: "0".repeat(64),
        };
        let err = verify_file(dir.path(), &entry).unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }));
    }

    #[test]
    fn cache_state_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let reg = ModelRegistry::new(dir.path());
        let manifest = ModelManifest {
            schema_version: 1,
            id: "m".into(),
            name: "M".into(),
            version: "1".into(),
            license: "CC-BY-4.0".into(),
            attribution: "x".into(),
            local_dir: "m".into(),
            languages: vec![],
            common: vec![],
            artifacts: vec![],
        };
        let files = vec![FileEntry {
            path: "a.bin".into(),
            url: "https://x/a".into(),
            bytes: 3,
            sha256: "a".repeat(64),
        }];
        assert_eq!(reg.state(&manifest, &files), CacheState::Missing);
        write(&reg.model_dir(&manifest).join("a.bin"), b"abc");
        assert_eq!(reg.state(&manifest, &files), CacheState::Installed);
        write(&reg.model_dir(&manifest).join("a.bin"), b"ab");
        assert_eq!(reg.state(&manifest, &files), CacheState::Incomplete);
    }
}
