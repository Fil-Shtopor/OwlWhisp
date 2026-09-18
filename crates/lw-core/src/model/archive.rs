//! Unpacking model archives, for publishers who ship one file instead of many.
//!
//! sherpa-onnx publishes its models as `.tar.bz2` on a GitHub release — 498 of them — and does not
//! mirror most of them as loose files anywhere. Until now that put every one of them out of reach:
//! the installer could pin and verify a URL, but not open what came back. `parakeet-tdt-ctc-110m-en`
//! is already in the catalog at full precision for exactly this reason, its int8 weights existing
//! only inside an archive.
//!
//! # What makes this safe
//!
//! Extraction is the part of an installer that gets it wrong, so the rules are explicit:
//!
//! 1. **The hash is checked before a byte is unpacked.** The caller verifies the archive against
//!    the manifest's SHA-256 first; this module is only ever handed bytes that already matched.
//!    Nothing here decides whether to trust the source.
//! 2. **Every path is resolved and confined.** An entry naming `..`, an absolute path or a Windows
//!    drive prefix fails the whole install rather than being skipped, because an archive that
//!    contains one is not an archive we should be unpacking at all.
//! 3. **Only regular files and directories.** Symlinks and hard links are refused: a link is how
//!    an archive escapes its directory without any path looking suspicious.
//! 4. **Bounded.** Entry count and total unpacked size are capped, so a small download cannot fill
//!    the disk.
//!
//! Nothing unpacked is ever executed — these are ONNX graphs and vocabularies, loaded by the
//! engine, and the app has no path that runs a downloaded binary.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How many entries one archive may contain.
const MAX_ENTRIES: usize = 4_096;

/// How much one archive may unpack to. Generous next to the largest model here (~1 GB) and far
/// below anything that would fill a disk.
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Archive formats the installer understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveFormat {
    /// `.tar.bz2`, which is what sherpa-onnx publishes.
    #[serde(rename = "tar.bz2")]
    TarBz2,
}

/// Instructions for unpacking a downloaded file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extract {
    /// The archive format.
    pub format: ArchiveFormat,
    /// Leading path components to drop.
    ///
    /// Publishers wrap everything in a single directory named after the release, and the model
    /// directory should hold `encoder.onnx`, not `sherpa-onnx-whatever-2026-01-01/encoder.onnx`.
    #[serde(default)]
    pub strip_components: usize,
    /// When non-empty, only unpack entries whose (stripped) path starts with one of these.
    ///
    /// Archives carry test audio, shell scripts and READMEs beside the model. Fetching them is
    /// unavoidable -- the publisher ships one file -- but writing them into the model directory is
    /// not, and a directory holding only what the engine loads is one a reader can check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only_prefixes: Vec<String>,
}

/// Unpack `archive` into `dest`, returning the relative paths written.
///
/// `dest` must already exist. The archive file is left alone; the caller removes it.
pub fn extract(archive: &Path, dest: &Path, spec: &Extract) -> Result<Vec<String>> {
    match spec.format {
        ArchiveFormat::TarBz2 => {
            let file =
                std::fs::File::open(archive).map_err(|e| Error::io(archive.display().to_string(), e))?;
            let decoder = bzip2::read::BzDecoder::new(std::io::BufReader::new(file));
            unpack_tar(decoder, dest, spec)
        }
    }
}

fn unpack_tar<R: Read>(reader: R, dest: &Path, spec: &Extract) -> Result<Vec<String>> {
    let mut tar = tar::Archive::new(reader);
    let mut written = Vec::new();
    let mut total: u64 = 0;
    let mut seen = 0usize;

    let entries = tar
        .entries()
        .map_err(|e| Error::Model(format!("archive could not be read: {e}")))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| Error::Model(format!("archive entry unreadable: {e}")))?;

        seen += 1;
        if seen > MAX_ENTRIES {
            return Err(Error::Model(format!(
                "archive has more than {MAX_ENTRIES} entries; refusing to unpack it"
            )));
        }

        // Links are the escape hatch that no path check catches, so they end the install.
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            return Err(Error::Model(
                "archive contains a link; refusing to unpack it".into(),
            ));
        }
        if !kind.is_file() && !kind.is_dir() {
            continue; // pax headers and similar metadata entries
        }

        let raw = entry
            .path()
            .map_err(|e| Error::Model(format!("archive entry has no usable path: {e}")))?
            .into_owned();
        let Some(rel) = confine(&raw, spec.strip_components)? else {
            continue; // stripped away entirely
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !spec.only_prefixes.is_empty() && !spec.only_prefixes.iter().any(|p| rel_str.starts_with(p)) {
            continue;
        }

        let out = dest.join(&rel);
        if kind.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| Error::io(out.display().to_string(), e))?;
            continue;
        }

        let size = entry.header().size().unwrap_or(0);
        total = total.saturating_add(size);
        if total > MAX_TOTAL_BYTES {
            return Err(Error::Model(format!(
                "archive unpacks to more than {} GiB; refusing to continue",
                MAX_TOTAL_BYTES / (1024 * 1024 * 1024)
            )));
        }

        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
        }
        let mut sink = std::fs::File::create(&out).map_err(|e| Error::io(out.display().to_string(), e))?;
        std::io::copy(&mut entry, &mut sink).map_err(|e| Error::io(out.display().to_string(), e))?;
        written.push(rel.to_string_lossy().replace('\\', "/"));
    }

    if written.is_empty() {
        return Err(Error::Model(
            "archive unpacked to no files; the manifest's strip_components or only_prefixes is wrong".into(),
        ));
    }
    Ok(written)
}

/// Strip leading components and confine the result to a relative path under the destination.
///
/// Returns `None` when the entry is entirely consumed by `strip_components`, and an error for
/// anything that tries to leave the directory.
fn confine(path: &Path, strip: usize) -> Result<Option<PathBuf>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(p) => parts.push(p.to_owned()),
            // A `.` is harmless; everything else is an attempt to name something outside.
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::Model(format!(
                    "archive entry escapes the model directory: {}",
                    path.display()
                )));
            }
        }
    }
    if parts.len() <= strip {
        return Ok(None);
    }
    Ok(Some(parts[strip..].iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a `.tar.bz2` in memory from (path, contents) pairs.
    fn tar_bz2(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (name, body) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, *body).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    /// A tar built byte by byte, because `tar::Builder` refuses to write a hostile path — which
    /// is exactly the archive the confinement rules exist to survive. A real attacker is not
    /// using the safe API either.
    fn raw_tar_bz2(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_bytes: Vec<u8> = Vec::new();
        for (name, body) in entries {
            let mut header = [0u8; 512];
            let name_bytes = name.as_bytes();
            header[..name_bytes.len()].copy_from_slice(name_bytes);
            header[100..107].copy_from_slice(b"0000644"); // mode
            header[108..115].copy_from_slice(b"0000000"); // uid
            header[116..123].copy_from_slice(b"0000000"); // gid
            let size = format!("{:011o}", body.len());
            header[124..135].copy_from_slice(size.as_bytes());
            header[136..147].copy_from_slice(b"00000000000"); // mtime
            header[156] = b'0'; // regular file
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");
            // Checksum is computed with the checksum field read as spaces.
            header[148..156].copy_from_slice(b"        ");
            let sum: u32 = header.iter().map(|b| u32::from(*b)).sum();
            let chk = format!("{sum:06o}\0 ");
            header[148..156].copy_from_slice(chk.as_bytes());

            tar_bytes.extend_from_slice(&header);
            tar_bytes.extend_from_slice(body);
            let pad = (512 - body.len() % 512) % 512;
            tar_bytes.extend(std::iter::repeat_n(0u8, pad));
        }
        tar_bytes.extend(std::iter::repeat_n(0u8, 1024)); // end-of-archive
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lw-archive-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_archive(dir: &Path, bytes: &[u8]) -> PathBuf {
        let p = dir.join("a.tar.bz2");
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn spec(strip: usize) -> Extract {
        Extract {
            format: ArchiveFormat::TarBz2,
            strip_components: strip,
            only_prefixes: Vec::new(),
        }
    }

    #[test]
    fn it_unpacks_and_strips_the_wrapper_directory() {
        let dir = scratch("basic");
        let archive = write_archive(
            &dir,
            &tar_bz2(&[
                ("sherpa-onnx-model-2026/encoder.onnx", b"enc"),
                ("sherpa-onnx-model-2026/tokens.txt", b"tok"),
            ]),
        );
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();

        let written = extract(&archive, &out, &spec(1)).unwrap();

        assert_eq!(written, ["encoder.onnx", "tokens.txt"]);
        assert_eq!(std::fs::read(out.join("encoder.onnx")).unwrap(), b"enc");
        assert!(!out.join("sherpa-onnx-model-2026").exists());
    }

    #[test]
    fn only_prefix_leaves_the_test_audio_behind() {
        let dir = scratch("prefix");
        let archive = write_archive(
            &dir,
            &tar_bz2(&[
                ("m/encoder.onnx", b"enc"),
                ("m/test_wavs/one.wav", b"wav"),
                ("m/run.sh", b"#!/bin/sh"),
            ]),
        );
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();

        let mut s = spec(1);
        s.only_prefixes = vec!["encoder".into()];
        let written = extract(&archive, &out, &s).unwrap();

        assert_eq!(written, ["encoder.onnx"]);
        assert!(!out.join("test_wavs").exists(), "test audio must not be written");
        assert!(!out.join("run.sh").exists(), "a shell script must not be written");
    }

    /// The whole reason this module documents its rules: an archive must not be able to write
    /// outside the directory it is unpacked into.
    #[test]
    fn an_entry_that_climbs_out_fails_the_install() {
        let dir = scratch("escape");
        let archive = write_archive(&dir, &raw_tar_bz2(&[("../evil.onnx", b"nope")]));
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();

        let err = extract(&archive, &out, &spec(0)).unwrap_err().to_string();
        assert!(err.contains("escapes the model directory"), "{err}");
        assert!(!dir.join("evil.onnx").exists(), "nothing may be written outside");
    }

    #[test]
    fn an_absolute_entry_fails_the_install() {
        let dir = scratch("absolute");
        let archive = write_archive(&dir, &raw_tar_bz2(&[("/etc/evil", b"nope")]));
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();

        let err = extract(&archive, &out, &spec(0)).unwrap_err().to_string();
        assert!(err.contains("escapes the model directory"), "{err}");
    }

    #[test]
    fn an_archive_that_unpacks_to_nothing_is_an_error_not_a_silent_success() {
        let dir = scratch("empty");
        let archive = write_archive(&dir, &tar_bz2(&[("wrapper/only.onnx", b"x")]));
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();

        // Two components stripped from a two-component path leaves nothing.
        let err = extract(&archive, &out, &spec(2)).unwrap_err().to_string();
        assert!(err.contains("unpacked to no files"), "{err}");
    }

    #[test]
    fn confine_strips_and_rejects() {
        assert_eq!(
            confine(Path::new("a/b/c.onnx"), 1).unwrap(),
            Some(PathBuf::from("b").join("c.onnx"))
        );
        assert_eq!(confine(Path::new("a/b"), 2).unwrap(), None);
        assert_eq!(confine(Path::new("./a"), 0).unwrap(), Some(PathBuf::from("a")));
        assert!(confine(Path::new("a/../../b"), 0).is_err());
    }
}
