//! Privacy-preserving hardware fingerprinting and seat metadata.
//!
//! Raw hardware serial numbers or platform UUIDs are wrapped in `zeroize::Zeroizing`
//! immediately upon collection and hashed via `SHA-256(salt || ":" || platform_machine_id)`.
//! Raw identifiers never leave the local machine or persist to disk.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::LicenseError;

/// Domain-separated salt for RatPoison hardware fingerprints.
pub const DEFAULT_FINGERPRINT_SALT: &[u8] = b"ratpoison.mouseless.hardware-fingerprint.v1";

/// Domain-separated salt for hashing license keys before storage or offline challenge transport.
pub const LICENSE_KEY_HASH_SALT: &[u8] = b"ratpoison.mouseless.license-key-hash.v1";

/// Summary of a machine requesting or holding a license seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineSeatSummary {
    /// Hex-encoded `SHA-256(salt || platform_machine_id)` digest (64 hex chars).
    pub machine_fingerprint: String,
    /// Human-readable hostname shown in the 5-machine deactivation dialog.
    pub friendly_hostname: String,
    /// Normalized operating system identifier (`macos`, `windows`, `linux`).
    pub os: String,
}

/// Active seat entry in the 5-machine license pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineSeat {
    /// Assigned seat slot (`1..=5`).
    pub seat_id: u8,
    /// Privacy-preserving SHA-256 hardware fingerprint.
    pub machine_fingerprint: String,
    /// User-recognizable hostname for the deactivation prompt.
    pub friendly_hostname: String,
    /// Operating system label (`macos`, `windows`, `linux`).
    pub os: String,
    /// Unix timestamp (seconds) when this seat was first activated.
    pub activated_at: u64,
    /// Unix timestamp (seconds) when this seat was last verified/refreshed.
    pub last_seen: u64,
}

/// Encodes a byte slice as lowercase hexadecimal.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Computes a privacy-preserving hardware fingerprint:
/// `SHA-256(salt || b":" || raw_platform_machine_id)`.
///
/// The caller's raw identifier is absorbed into a `Zeroizing<String>` copy if normalized,
/// ensuring intermediate buffers are wiped from memory on drop.
pub fn compute_machine_fingerprint(raw_platform_machine_id: &str) -> Result<String, LicenseError> {
    compute_machine_fingerprint_with_salt(raw_platform_machine_id, DEFAULT_FINGERPRINT_SALT)
}

/// Computes a salted SHA-256 hardware fingerprint with a custom domain salt.
pub fn compute_machine_fingerprint_with_salt(
    raw_platform_machine_id: &str,
    salt: &[u8],
) -> Result<String, LicenseError> {
    let trimmed = Zeroizing::new(raw_platform_machine_id.trim().to_string());
    if trimmed.is_empty() {
        return Err(LicenseError::StorageIo(
            "platform machine identifier cannot be empty".to_string(),
        ));
    }

    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(b":");
    hasher.update(trimmed.as_bytes());
    let digest = hasher.finalize();
    Ok(hex_encode(&digest))
}

/// Normalizes and hashes a raw license key inside a `Zeroizing<String>` guard.
///
/// Returns the lowercase hex SHA-256 digest of the canonical uppercase key.
pub fn normalize_and_hash_license_key(raw_license_key: &str) -> Result<String, LicenseError> {
    let normalized = Zeroizing::new(raw_license_key.trim().to_ascii_uppercase());
    if normalized.len() < 8 {
        return Err(LicenseError::InvalidLicenseKey(
            "license key must be at least 8 characters".to_string(),
        ));
    }
    if !normalized
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(LicenseError::InvalidLicenseKey(
            "license key may only contain ASCII alphanumeric characters and hyphens".to_string(),
        ));
    }

    let mut hasher = Sha256::new();
    hasher.update(LICENSE_KEY_HASH_SALT);
    hasher.update(b":");
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    Ok(hex_encode(&digest))
}

/// Detects the current host's stable machine fingerprint without exposing raw hardware serials.
pub fn detect_current_machine_fingerprint() -> Result<String, LicenseError> {
    let raw_id: Zeroizing<String> = Zeroizing::new(read_raw_platform_machine_id());
    compute_machine_fingerprint(raw_id.as_str())
}

fn read_raw_platform_machine_id() -> String {
    // 1. Try Linux /etc/machine-id or /var/lib/dbus/machine-id
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let clean = contents.trim();
            if !clean.is_empty() {
                return clean.to_string();
            }
        }
    }

    // 2. Fallback to OS + hostname + user environment composite when running in restricted sandboxes
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "ratpoison-local-host".to_string());
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "ratpoison-user".to_string());
    format!("{}:{}:{}", std::env::consts::OS, host, user)
}
