//! Runtime manifest: a pinned description of the ORT + QNN DLLs the app ships, with per-file
//! SHA-256, verified before `ort::init_from` so a tampered or mismatched runtime fails loudly.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One runtime file (DLL/so) with its expected identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeFile {
    /// File name relative to the runtime directory.
    pub name: String,
    /// Expected size in bytes (0 = don't check size).
    #[serde(default)]
    pub bytes: u64,
    /// Expected lowercase-hex SHA-256 (empty = don't check hash).
    #[serde(default)]
    pub sha256: String,
    /// Whether the file must be present.
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

/// The pinned runtime contract.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeManifest {
    /// Schema version.
    pub schema_version: u32,
    /// Expected `ort` crate version, e.g. `"2.0.0-rc.13"`.
    pub ort_crate: String,
    /// Expected native ONNX Runtime version, e.g. `"1.29.0"`.
    pub onnxruntime: String,
    /// Expected QAIRT / QNN version, e.g. `"2.49.40"` (only meaningful when the QNN EP ships).
    #[serde(default)]
    pub qairt: Option<String>,
    /// Files common to every architecture.
    #[serde(default)]
    pub common: Vec<RuntimeFile>,
    /// Files for the QNN EP (Windows ARM64 only).
    #[serde(default)]
    pub qnn: Vec<RuntimeFile>,
}

impl RuntimeManifest {
    /// Parse from JSON.
    pub fn from_json(s: &str) -> Result<Self, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    /// Verify that the required files exist (and match size/hash if pinned) in `dir`.
    ///
    /// `include_qnn` selects whether the QNN files are also required (true on Snapdragon builds).
    pub fn verify(&self, dir: &Path, include_qnn: bool) -> Result<(), String> {
        let files = self
            .common
            .iter()
            .chain(if include_qnn { self.qnn.iter() } else { [].iter() });
        for f in files {
            let path = dir.join(&f.name);
            match std::fs::metadata(&path) {
                Ok(m) => {
                    if f.bytes != 0 && m.len() != f.bytes {
                        return Err(format!("{}: size {} != expected {}", f.name, m.len(), f.bytes));
                    }
                    if !f.sha256.is_empty() {
                        let actual = crate::runtime::sha256_file(&path).map_err(|e| e.to_string())?;
                        if !actual.eq_ignore_ascii_case(&f.sha256) {
                            return Err(format!("{}: sha256 mismatch", f.name));
                        }
                    }
                }
                Err(_) if f.required => return Err(format!("missing required runtime file: {}", f.name)),
                Err(_) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_and_verifies() {
        let json = r#"{
            "schema_version": 1,
            "ort_crate": "2.0.0-rc.13",
            "onnxruntime": "1.29.0",
            "qairt": "2.49.40",
            "common": [{"name": "onnxruntime.dll"}],
            "qnn": [{"name": "onnxruntime_providers_qnn.dll"}]
        }"#;
        let m = RuntimeManifest::from_json(json).unwrap();
        assert_eq!(m.onnxruntime, "1.29.0");
        assert_eq!(m.qairt.as_deref(), Some("2.49.40"));

        let dir = tempfile::tempdir().unwrap();
        std::fs::File::create(dir.path().join("onnxruntime.dll"))
            .unwrap()
            .write_all(b"x")
            .unwrap();
        // qnn not required -> ok
        m.verify(dir.path(), false).unwrap();
        // qnn required -> missing
        assert!(m.verify(dir.path(), true).is_err());
    }

    #[test]
    fn size_mismatch_detected() {
        let json =
            r#"{"schema_version":1,"ort_crate":"x","onnxruntime":"1","common":[{"name":"a","bytes":5}]}"#;
        let m = RuntimeManifest::from_json(json).unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::File::create(dir.path().join("a"))
            .unwrap()
            .write_all(b"xxx")
            .unwrap();
        assert!(m.verify(dir.path(), false).is_err());
    }
}
