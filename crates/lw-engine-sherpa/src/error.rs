//! Error type for the sherpa-onnx engine.

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by [`crate`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// I/O error with context.
    #[error("i/o: {0}")]
    Io(String),
    /// The model directory did not match any supported layout.
    #[error("unrecognized model layout in {dir}: {reason}")]
    UnknownLayout {
        /// The directory that was inspected.
        dir: String,
        /// Why detection failed, and what was expected.
        reason: String,
    },
    /// A file required by the detected layout was missing.
    #[error("missing model file: {0}")]
    MissingFile(String),
    /// A configuration value was invalid.
    #[error("invalid configuration: {0}")]
    Config(String),
    /// The native sherpa-onnx library refused to build a recognizer.
    #[error("sherpa-onnx: {0}")]
    Sherpa(String),
    /// The engine was used before `initialize` succeeded.
    #[error("engine not initialized")]
    NotInitialized,
    /// The crate was built without the `sherpa` cargo feature.
    #[error("lw-engine-sherpa was built without the `sherpa` cargo feature")]
    FeatureDisabled,
    /// Generic error.
    #[error("{0}")]
    Other(String),
}

impl From<Error> for lw_core::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::FeatureDisabled => lw_core::Error::Unavailable(e.to_string()),
            other => lw_core::Error::Engine(other.to_string()),
        }
    }
}
