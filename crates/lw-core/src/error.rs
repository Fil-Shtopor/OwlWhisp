//! Error types shared across the core.

use std::path::PathBuf;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by `lw-core`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An I/O error with contextual path information.
    #[error("i/o error at {path}: {source}")]
    Io {
        /// The path involved, if known.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },

    /// Serialization/deserialization failure.
    #[error("serialization error: {0}")]
    Serde(String),

    /// A configuration value was invalid.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// Audio processing error (resampling, format).
    #[error("audio error: {0}")]
    Audio(String),

    /// A speech engine failed.
    #[error("engine error: {0}")]
    Engine(String),

    /// Model download or verification error.
    #[error("model error: {0}")]
    Model(String),

    /// A checksum did not match the pinned value.
    #[error("integrity check failed for {file}: expected {expected}, got {actual}")]
    Integrity {
        /// The file that failed verification.
        file: PathBuf,
        /// Expected SHA-256 (hex).
        expected: String,
        /// Actual SHA-256 (hex).
        actual: String,
    },

    /// A feature or backend is not available on this platform/hardware.
    #[error("unavailable: {0}")]
    Unavailable(String),

    /// A generic error carrying a message.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Construct an I/O error with a path for context.
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
    /// Construct a generic error from anything displayable.
    pub fn other(msg: impl std::fmt::Display) -> Self {
        Error::Other(msg.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}
