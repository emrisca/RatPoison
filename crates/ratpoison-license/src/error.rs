//! Error types for licensing, trial management, offline activation, and updater verification.

use thiserror::Error;

/// Unified error type for `ratpoison-license`.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LicenseError {
    #[error("invalid license key format: {0}")]
    InvalidLicenseKey(String),

    #[error("monotonic clock rollback detected (last observed: {last_observed}, current: {current}, rollback: {rollback_secs}s)")]
    ClockRollbackDetected {
        last_observed: u64,
        current: u64,
        rollback_secs: u64,
    },

    #[error("14-day evaluation trial expired at timestamp {expired_at}")]
    TrialExpired { expired_at: u64 },

    #[error("machine seat limit ({max_machines}) reached; deactivation of an existing seat is required")]
    SeatLimitReached { max_machines: usize },

    #[error("target machine fingerprint '{0}' was not found in the active seat registry")]
    MachineSeatNotFound(String),

    #[error("invalid seat identifier {0}; expected 1..=5")]
    InvalidSeatId(u8),

    #[error("malformed offline challenge code: {0}")]
    MalformedChallenge(String),

    #[error("malformed offline activation token: {0}")]
    MalformedActivationToken(String),

    #[error("offline activation token Ed25519 cryptographic signature verification failed")]
    SignatureVerificationFailed,

    #[error("offline activation token machine fingerprint mismatch (expected '{expected}', got '{actual}')")]
    FingerprintMismatch { expected: String, actual: String },

    #[error("offline challenge license key hash does not match issuer license key")]
    LicenseKeyMismatch,

    #[error("offline activation token expired at {expires_at} (current: {current})")]
    TokenExpired { expires_at: u64, current: u64 },

    #[error("failed to parse semantic version '{0}'")]
    InvalidSemVer(String),

    #[error("invalid update manifest: {0}")]
    InvalidUpdateManifest(String),

    #[error("platform target '{0}' not found in update manifest")]
    PlatformTargetNotFound(String),

    #[error("cryptographic signature metadata missing or invalid for update artifact: {0}")]
    InvalidUpdateSignature(String),

    #[error("license storage I/O error: {0}")]
    StorageIo(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}
