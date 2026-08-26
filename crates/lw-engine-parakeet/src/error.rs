//! Error type for the Parakeet engine.

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Parakeet engine errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// I/O error with context.
    #[error("i/o: {0}")]
    Io(String),
    /// ONNX Runtime error.
    #[error("onnxruntime: {0}")]
    Ort(String),
    /// A model file was missing.
    #[error("missing model file: {0}")]
    MissingFile(String),
    /// Feature extraction failed.
    #[error("feature extraction: {0}")]
    Feature(String),
    /// Decoding failed.
    #[error("decode: {0}")]
    Decode(String),
    /// Generic error.
    #[error("{0}")]
    Other(String),
}

impl From<lw_ort::ort::Error> for Error {
    fn from(e: lw_ort::ort::Error) -> Self {
        Error::Ort(e.to_string())
    }
}

impl From<Error> for lw_core::Error {
    fn from(e: Error) -> Self {
        lw_core::Error::Engine(e.to_string())
    }
}
