//! Error types for `ratpoison-core`.

use thiserror::Error;

/// Errors produced by pure core geometry, grid, preset, config, or state-machine operations.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum CoreError {
    /// A monitor definition had invalid geometry or scale factor.
    #[error("invalid monitor '{id}': {reason}")]
    InvalidMonitor { id: String, reason: String },

    /// Duplicate monitor identifier in a `MonitorLayout`.
    #[error("duplicate monitor id '{0}'")]
    DuplicateMonitorId(String),

    /// Grid dimensions or subgrid dimensions were invalid (e.g. zero rows/cols).
    #[error("invalid grid dimensions {cols}x{rows}: {reason}")]
    InvalidGridDimensions {
        cols: u16,
        rows: u16,
        reason: String,
    },

    /// Label alphabet could not generate enough unique labels for the requested grid size.
    #[error("insufficient label capacity: grid requires {required} unique labels, scheme provides {available}")]
    InsufficientLabelCapacity { required: usize, available: usize },

    /// Label scheme was empty or contained duplicate characters.
    #[error("invalid label scheme: {0}")]
    InvalidLabelScheme(String),

    /// Requested setting key for `restore_default` is not recognized.
    #[error("unknown setting key '{0}'")]
    UnknownSettingKey(String),

    /// JSON serialization or deserialization failed.
    #[error("config serialization error: {0}")]
    Serialization(String),
}
