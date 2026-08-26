//! Resumable, verified model downloader (async, `reqwest` + `tokio`).
//!
//! For each file: check free disk space, download to a `.part` file using an HTTP `Range` request
//! to resume any partial content, stream to disk while hashing, verify size + SHA-256, then move
//! the verified file into a staging directory. When all files pass, the staging directory is
//! atomically promoted to the model directory (rename), so an interrupted install never yields a
//! half-populated model dir.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::manifest::FileEntry;
use crate::{Error, Result};

/// Per-file / overall progress.
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    /// Bytes downloaded for the current file.
    pub file_downloaded: u64,
    /// Total bytes of the current file.
    pub file_total: u64,
    /// Index of the current file (0-based).
    pub file_index: usize,
    /// Total number of files.
    pub file_count: usize,
}

/// Events emitted during a download.
#[derive(Clone, Debug)]
pub enum DownloadEvent {
    /// Starting a file.
    FileStarted {
        /// The relative path.
        path: String,
        /// Total bytes.
        total: u64,
    },
    /// Progress update.
    Progress(Progress),
    /// A file finished and verified.
    FileVerified {
        /// The relative path.
        path: String,
    },
    /// All files installed and promoted.
    Completed,
}

/// Downloads and verifies a model's files.
pub struct ModelDownloader {
    client: reqwest::Client,
    /// Extra free-space margin required beyond the sum of file sizes.
    margin_bytes: u64,
}

impl ModelDownloader {
    /// New downloader with a default client and a 128 MiB free-space margin.
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("LocalWisper/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Model(format!("http client: {e}")))?;
        Ok(Self { client, margin_bytes: 128 * 1024 * 1024 })
    }

    /// Download `files` into `staging_dir`, then atomically rename to `final_dir`.
    ///
    /// `on_event` is called with progress and lifecycle events. `cancel` aborts cooperatively.
    pub async fn install<F>(
        &self,
        files: &[FileEntry],
        staging_dir: &Path,
        final_dir: &Path,
        cancel: CancellationToken,
        mut on_event: F,
    ) -> Result<()>
    where
        F: FnMut(DownloadEvent),
    {
        // Disk-space pre-check against the staging parent.
        let need: u64 = files.iter().map(|f| f.bytes).sum::<u64>() + self.margin_bytes;
        if let Some(parent) = staging_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
            if let Some(free) = available_space(parent) {
                if free < need {
                    return Err(Error::Model(format!(
                        "insufficient disk space: need ~{} MiB, have {} MiB",
                        need / 1_048_576,
                        free / 1_048_576
                    )));
                }
            }
        }

        // Fresh staging dir.
        if staging_dir.exists() {
            std::fs::remove_dir_all(staging_dir).map_err(|e| Error::io(staging_dir.display().to_string(), e))?;
        }
        std::fs::create_dir_all(staging_dir).map_err(|e| Error::io(staging_dir.display().to_string(), e))?;

        let count = files.len();
        for (index, f) in files.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(Error::Model("download cancelled".into()));
            }
            if !f.path_is_safe() {
                return Err(Error::Model(format!("unsafe path in manifest: {}", f.path)));
            }
            let dest = staging_dir.join(&f.path);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p).map_err(|e| Error::io(p.display().to_string(), e))?;
            }
            on_event(DownloadEvent::FileStarted { path: f.path.clone(), total: f.bytes });
            self.download_one(f, &dest, index, count, &cancel, &mut on_event).await?;

            // Verify size + hash in the staging dir.
            super::verify_file(staging_dir, f)?;
            on_event(DownloadEvent::FileVerified { path: f.path.clone() });
        }

        // Atomic promote: replace final_dir with staging_dir.
        promote(staging_dir, final_dir)?;
        on_event(DownloadEvent::Completed);
        Ok(())
    }

    async fn download_one<F>(
        &self,
        f: &FileEntry,
        dest: &Path,
        index: usize,
        count: usize,
        cancel: &CancellationToken,
        on_event: &mut F,
    ) -> Result<()>
    where
        F: FnMut(DownloadEvent),
    {
        let part = dest.with_extension("part");
        let mut have: u64 = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        // If the partial is already too big, restart it.
        if have > f.bytes {
            let _ = std::fs::remove_file(&part);
            have = 0;
        }

        let mut req = self.client.get(&f.url).timeout(std::time::Duration::from_secs(3600));
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = req.send().await.map_err(|e| Error::Model(format!("GET {}: {e}", f.url)))?;
        let status = resp.status();
        let resuming = status == reqwest::StatusCode::PARTIAL_CONTENT;
        if !status.is_success() {
            return Err(Error::Model(format!("GET {} -> HTTP {}", f.url, status)));
        }
        if have > 0 && !resuming {
            // Server ignored Range; start over.
            let _ = std::fs::remove_file(&part);
            have = 0;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(resuming)
            .write(true)
            .truncate(!resuming)
            .open(&part)
            .await
            .map_err(|e| Error::io(part.display().to_string(), e))?;

        let mut downloaded = have;
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            if cancel.is_cancelled() {
                return Err(Error::Model("download cancelled".into()));
            }
            let chunk = chunk.map_err(|e| Error::Model(format!("stream {}: {e}", f.url)))?;
            file.write_all(&chunk).await.map_err(|e| Error::io(part.display().to_string(), e))?;
            downloaded += chunk.len() as u64;
            on_event(DownloadEvent::Progress(Progress {
                file_downloaded: downloaded,
                file_total: f.bytes,
                file_index: index,
                file_count: count,
            }));
        }
        file.flush().await.map_err(|e| Error::io(part.display().to_string(), e))?;
        file.sync_all().await.map_err(|e| Error::io(part.display().to_string(), e))?;
        drop(file);

        std::fs::rename(&part, dest).map_err(|e| Error::io(dest.display().to_string(), e))?;
        Ok(())
    }
}

impl Default for ModelDownloader {
    fn default() -> Self {
        Self::new().expect("http client")
    }
}

/// Atomically replace `final_dir` with `staging_dir`.
fn promote(staging_dir: &Path, final_dir: &Path) -> Result<()> {
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
    }
    if final_dir.exists() {
        let backup = final_dir.with_extension("replaced");
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(final_dir, &backup).map_err(|e| Error::io(final_dir.display().to_string(), e))?;
        match std::fs::rename(staging_dir, final_dir) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&backup);
                Ok(())
            }
            Err(e) => {
                // Roll back.
                let _ = std::fs::rename(&backup, final_dir);
                Err(Error::io(final_dir.display().to_string(), e))
            }
        }
    } else {
        std::fs::rename(staging_dir, final_dir).map_err(|e| Error::io(final_dir.display().to_string(), e))
    }
}

/// Best-effort free-space query for the filesystem holding `path`.
fn available_space(path: &Path) -> Option<u64> {
    // `sysinfo` gives per-disk data; we match the longest mount-point prefix.
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64)> = None;
    let path_str = path.to_string_lossy().to_lowercase();
    for d in disks.list() {
        let mp = d.mount_point().to_string_lossy().to_lowercase();
        if path_str.starts_with(&mp) {
            let len = mp.len();
            if best.map(|(l, _)| len > l).unwrap_or(true) {
                best = Some((len, d.available_space()));
            }
        }
    }
    best.map(|(_, s)| s)
}

/// Compute the SHA-256 of an in-memory buffer (used in tests and small verifications).
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// A staging directory path next to a model directory.
pub fn staging_dir_for(final_dir: &Path) -> PathBuf {
    let name = final_dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    final_dir.with_file_name(format!(".{name}.staging"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn promote_moves_staging_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join(".m.staging");
        let final_dir = dir.path().join("m");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::File::create(staging.join("f")).unwrap().write_all(b"x").unwrap();
        promote(&staging, &final_dir).unwrap();
        assert!(final_dir.join("f").exists());
        assert!(!staging.exists());
    }

    #[test]
    fn promote_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let final_dir = dir.path().join("m");
        std::fs::create_dir_all(&final_dir).unwrap();
        std::fs::File::create(final_dir.join("old")).unwrap();
        let staging = dir.path().join(".m.staging");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::File::create(staging.join("new")).unwrap();
        promote(&staging, &final_dir).unwrap();
        assert!(final_dir.join("new").exists());
        assert!(!final_dir.join("old").exists());
    }

    #[test]
    fn sha256_bytes_vector() {
        assert_eq!(sha256_bytes(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn staging_name() {
        let p = staging_dir_for(Path::new("/models/parakeet-v3"));
        assert!(p.to_string_lossy().ends_with(".parakeet-v3.staging"));
    }
}
