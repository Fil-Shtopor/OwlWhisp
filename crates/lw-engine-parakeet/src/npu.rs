//! Native preparation of the downloaded FP32 encoder for Qualcomm's static-shape HTP.

use crate::{Error, Result};
use lw_ort::OrtRuntime;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Whether an encoder the NPU path can prepare or load is present.
pub fn model_ready(model_dir: &Path, frames: usize) -> bool {
    static_model(model_dir, frames).is_some()
        || (model_dir.join("encoder-model.onnx").is_file()
            && model_dir.join("encoder-model.onnx.data").is_file())
}

fn static_model(model_dir: &Path, frames: usize) -> Option<PathBuf> {
    [
        format!("encoder-static-t{frames}.onnx"),
        "encoder-static.onnx".into(),
    ]
    .into_iter()
    .map(|name| model_dir.join(name))
    .find(|path| path.is_file())
}

/// Resolve an existing static encoder or prepare the FP32 graph once, without Python.
/// The cache includes the source graph, weights metadata, ORT version and window size.
/// A temporary directory keeps interrupted preparation from becoming a cache hit.
pub fn prepare_model(
    runtime: &OrtRuntime,
    model_dir: &Path,
    cache: &Path,
    frames: usize,
    threads: usize,
) -> Result<PathBuf> {
    if let Some(path) = static_model(model_dir, frames) {
        return Ok(path);
    }
    if frames == 0 || !model_ready(model_dir, frames) {
        return Err(Error::MissingFile(
            "NPU needs the FP32 encoder and its .onnx.data weights; prepare the NPU model in Settings".into(),
        ));
    }
    let source = model_dir.join("encoder-model.onnx");
    let weights = std::fs::metadata(model_dir.join("encoder-model.onnx.data"))?;
    let mut hash = Sha256::new();
    hash.update(std::fs::read(&source)?);
    hash.update(format!(
        "v1:{frames}:{}:{:?}:{}",
        weights.len(),
        weights.modified()?,
        lw_ort::ort::info()
    ));
    let directory = cache.join(format!("npu-graph-{}", hex::encode(hash.finalize())));
    let prepared = directory.join("encoder.onnx");
    if prepared.is_file() && directory.join("ready").is_file() {
        return Ok(prepared);
    }
    std::fs::create_dir_all(cache)?;
    let staging = tempfile::Builder::new()
        .prefix("npu-prepare-")
        .tempdir_in(cache)?;
    tracing::info!("preparing static NPU encoder for {frames} mel frames");
    lw_ort::prepare_static_model(
        runtime,
        &source,
        &staging.path().join("encoder.onnx"),
        &[
            ("audio_signal_dynamic_axes_1", 1),
            ("audio_signal_dynamic_axes_2", frames as i64),
            ("length_dynamic_axes_1", 1),
        ],
        threads,
    )
    .map_err(|e| Error::Ort(e.to_string()))?;
    std::fs::write(staging.path().join("ready"), b"1")?;
    if let Err(error) = std::fs::rename(staging.path(), &directory)
        && !(prepared.is_file() && directory.join("ready").is_file())
    {
        return Err(error.into());
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fp32_graph_without_external_weights_is_not_npu_ready() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("encoder-model.onnx"), b"graph").unwrap();
        assert!(!model_ready(dir.path(), 2000));
        std::fs::write(dir.path().join("encoder-model.onnx.data"), b"weights").unwrap();
        assert!(model_ready(dir.path(), 2000));
    }

    #[test]
    fn prebuilt_static_encoders_are_supported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("encoder-static-t2000.onnx"), b"graph").unwrap();
        assert!(model_ready(dir.path(), 2000));
        assert!(!model_ready(dir.path(), 1000));
    }
}
