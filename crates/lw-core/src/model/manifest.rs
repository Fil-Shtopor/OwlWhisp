//! Model manifest types: a pinned, hash-verified description of the files that make up a model,
//! plus per-SoC/architecture artifact keying so the right encoder is fetched for the hardware.

use serde::{Deserialize, Serialize};

/// A single downloadable file within a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative path within the model directory (no `..`, no absolute).
    pub path: String,
    /// HTTPS URL to fetch it from.
    pub url: String,
    /// Expected size in bytes.
    pub bytes: u64,
    /// Expected lowercase-hex SHA-256.
    pub sha256: String,
}

impl FileEntry {
    /// True if `path` is a safe relative path (no root, no `..`, no drive).
    pub fn path_is_safe(&self) -> bool {
        let p = std::path::Path::new(&self.path);
        !self.path.is_empty()
            && p.is_relative()
            && !self.path.contains("..")
            && !self.path.starts_with('/')
            && !self.path.starts_with('\\')
    }
}

/// The compute target an artifact set is compiled for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTarget {
    /// Runs anywhere (CPU / dynamic-shape ONNX).
    Any,
    /// CPU-only int8 ONNX.
    CpuInt8,
    /// Qualcomm HTP V73 context binary (Snapdragon X Elite).
    QnnHtpV73,
    /// Qualcomm HTP V81 context binary (Snapdragon X2 Elite).
    QnnHtpV81,
    /// Apple CoreML bundle.
    CoreMl,
}

/// A named set of files for one target (e.g. the CPU set vs the V81-NPU set).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactSet {
    /// The target this set is for.
    pub target: ArtifactTarget,
    /// The files.
    pub files: Vec<FileEntry>,
}

/// A complete model description.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelManifest {
    /// Manifest schema version.
    pub schema_version: u32,
    /// Stable model id, e.g. `"parakeet-tdt-0.6b-v3"`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Model version tag (e.g. HF revision).
    pub version: String,
    /// SPDX / licence string, e.g. `"CC-BY-4.0"`.
    pub license: String,
    /// Attribution line to show in About.
    pub attribution: String,
    /// Local directory name under the models cache.
    pub local_dir: String,
    /// Language codes the model supports.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Files common to every target (mel preprocessor, vocab, decoder).
    #[serde(default)]
    pub common: Vec<FileEntry>,
    /// Per-target artifact sets (the encoder, mainly).
    pub artifacts: Vec<ArtifactSet>,
}

impl ModelManifest {
    /// Validate the manifest structurally (safe paths, https URLs, hex hashes, non-zero sizes).
    pub fn validate(&self) -> crate::Result<()> {
        if self.schema_version != 1 {
            return Err(crate::Error::Model(format!(
                "unsupported schema_version {}",
                self.schema_version
            )));
        }
        let check = |f: &FileEntry| -> crate::Result<()> {
            if !f.path_is_safe() {
                return Err(crate::Error::Model(format!("unsafe path: {}", f.path)));
            }
            if !f.url.starts_with("https://") {
                return Err(crate::Error::Model(format!("non-https url: {}", f.url)));
            }
            if f.bytes == 0 {
                return Err(crate::Error::Model(format!("zero size: {}", f.path)));
            }
            if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(crate::Error::Model(format!("bad sha256: {}", f.path)));
            }
            Ok(())
        };
        for f in &self.common {
            check(f)?;
        }
        for a in &self.artifacts {
            for f in &a.files {
                check(f)?;
            }
        }
        Ok(())
    }

    /// The best artifact set for the given available targets, in preference order.
    /// Returns the common files plus the chosen set's files, and the target chosen.
    pub fn select_files(&self, preferred: &[ArtifactTarget]) -> Option<(ArtifactTarget, Vec<FileEntry>)> {
        for want in preferred {
            if let Some(set) = self.artifacts.iter().find(|a| a.target == *want) {
                let mut files = self.common.clone();
                files.extend(set.files.clone());
                return Some((set.target, files));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            url: "https://example.com/x".into(),
            bytes: 10,
            sha256: "a".repeat(64),
        }
    }

    fn manifest() -> ModelManifest {
        ModelManifest {
            schema_version: 1,
            id: "parakeet-tdt-0.6b-v3".into(),
            name: "Parakeet".into(),
            version: "v3".into(),
            license: "CC-BY-4.0".into(),
            attribution: "NVIDIA".into(),
            local_dir: "parakeet-v3".into(),
            languages: vec!["en".into()],
            common: vec![entry("vocab.txt")],
            artifacts: vec![
                ArtifactSet {
                    target: ArtifactTarget::CpuInt8,
                    files: vec![entry("encoder.int8.onnx")],
                },
                ArtifactSet {
                    target: ArtifactTarget::QnnHtpV81,
                    files: vec![entry("encoder.bin")],
                },
            ],
        }
    }

    #[test]
    fn validates_ok() {
        manifest().validate().unwrap();
    }

    #[test]
    fn rejects_unsafe_path() {
        let mut m = manifest();
        m.common[0].path = "../evil".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn rejects_http() {
        let mut m = manifest();
        m.common[0].url = "http://insecure".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn selects_preferred_target() {
        let m = manifest();
        let (target, files) = m
            .select_files(&[ArtifactTarget::QnnHtpV81, ArtifactTarget::CpuInt8])
            .unwrap();
        assert_eq!(target, ArtifactTarget::QnnHtpV81);
        // common + 1 artifact file
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn falls_back_to_cpu_when_npu_absent() {
        let mut m = manifest();
        m.artifacts.retain(|a| a.target == ArtifactTarget::CpuInt8);
        let (target, _) = m
            .select_files(&[ArtifactTarget::QnnHtpV81, ArtifactTarget::CpuInt8])
            .unwrap();
        assert_eq!(target, ArtifactTarget::CpuInt8);
    }
}
