//! Cross-Platform In-Place Update Architecture (`tauri-plugin-updater`, Sparkle, WinSparkle, Flatpak).
//!
//! Provides:
//! - Semantic Versioning 2.0.0 parsing and pre-release ordering (`SemVer`)
//! - Release channel selection (`UpdateChannel::Stable` vs `UpdateChannel::Beta`)
//! - Tauri v2 Updater JSON manifest parsing (`TauriUpdateManifest`, `PlatformArtifact`)
//! - Cryptographic signature metadata validation (Ed25519 / Minisign `.sig` verification)
//! - Native OS update backend coordination (`TauriUpdater`, `SparkleAppcast`, `WinSparkleOmaha`,
//!   and Linux `FlatpakOstree` sandbox detection where self-updating inside the sandbox is disabled
//!   in favor of `flatpak update`).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use base64::{engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::LicenseError;

/// Release channel used for update checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Production releases (`vX.Y.Z` without pre-release suffixes).
    #[default]
    Stable,
    /// Preview / release-candidate builds (`vX.Y.Z-beta.N`, `vX.Y.Z-rc.N`), plus any newer stable build.
    Beta,
}

/// Platform-native or unified updater delivery backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateBackend {
    /// Unified `tauri-plugin-updater` static/dynamic JSON endpoint with Ed25519/Minisign signatures.
    TauriUpdater,
    /// macOS native Sparkle 2 framework using Ed25519-signed `appcast.xml` enclosures.
    SparkleAppcast,
    /// Windows native WinSparkle / Omaha feed with Authenticode + Ed25519 signature verification.
    WinSparkleOmaha,
    /// Linux Flatpak OSTree repository / Flathub (`flatpak update io.github.emrisca.RatPoison`).
    /// In-place binary self-update is disabled inside the immutable `/app` Flatpak sandbox.
    FlatpakOstree,
}

/// Parsed Semantic Version (`MAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]`).
#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre_release: Option<String>,
    pub build_metadata: Option<String>,
}

impl SemVer {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre_release: None,
            build_metadata: None,
        }
    }

    /// Returns `true` if this version has a pre-release tag (e.g. `-beta.1` or `-rc.2`).
    pub fn is_prerelease(&self) -> bool {
        self.pre_release.is_some()
    }

    /// Checks whether this version is eligible for the requested [`UpdateChannel`].
    pub fn matches_channel(&self, channel: UpdateChannel) -> bool {
        match channel {
            UpdateChannel::Stable => !self.is_prerelease(),
            UpdateChannel::Beta => true,
        }
    }
}

impl PartialEq for SemVer {
    fn eq(&self, other: &Self) -> bool {
        self.major == other.major
            && self.minor == other.minor
            && self.patch == other.patch
            && self.pre_release == other.pre_release
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        match (
            self.major.cmp(&other.major),
            self.minor.cmp(&other.minor),
            self.patch.cmp(&other.patch),
        ) {
            (Ordering::Equal, Ordering::Equal, Ordering::Equal) => {}
            (Ordering::Equal, Ordering::Equal, ord) => return ord,
            (Ordering::Equal, ord, _) => return ord,
            (ord, _, _) => return ord,
        }

        // SemVer 2.0.0 rule: normal version has higher precedence than a pre-release version
        match (&self.pre_release, &other.pre_release) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => compare_prerelease_identifiers(a, b),
        }
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_prerelease_identifiers(a: &str, b: &str) -> Ordering {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ap), Some(bp)) => {
                let ord = match (ap.parse::<u64>(), bp.parse::<u64>()) {
                    (Ok(an), Ok(bn)) => an.cmp(&bn),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => ap.cmp(bp),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(ref pre) = self.pre_release {
            write!(f, "-{pre}")?;
        }
        if let Some(ref build) = self.build_metadata {
            write!(f, "+{build}")?;
        }
        Ok(())
    }
}

impl FromStr for SemVer {
    type Err = LicenseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim().strip_prefix('v').unwrap_or_else(|| s.trim());
        if trimmed.is_empty() {
            return Err(LicenseError::InvalidSemVer(s.to_string()));
        }

        let (without_build, build_metadata) = match trimmed.split_once('+') {
            Some((left, build)) => {
                if build.is_empty() {
                    return Err(LicenseError::InvalidSemVer(s.to_string()));
                }
                (left, Some(build.to_string()))
            }
            None => (trimmed, None),
        };

        let (core, pre_release) = match without_build.split_once('-') {
            Some((left, pre)) => {
                if pre.is_empty() {
                    return Err(LicenseError::InvalidSemVer(s.to_string()));
                }
                (left, Some(pre.to_string()))
            }
            None => (without_build, None),
        };

        let mut parts = core.split('.');
        let major_str = parts
            .next()
            .ok_or_else(|| LicenseError::InvalidSemVer(s.to_string()))?;
        let minor_str = parts
            .next()
            .ok_or_else(|| LicenseError::InvalidSemVer(s.to_string()))?;
        let patch_str = parts
            .next()
            .ok_or_else(|| LicenseError::InvalidSemVer(s.to_string()))?;
        if parts.next().is_some() {
            return Err(LicenseError::InvalidSemVer(s.to_string()));
        }

        let major = major_str
            .parse::<u64>()
            .map_err(|_| LicenseError::InvalidSemVer(s.to_string()))?;
        let minor = minor_str
            .parse::<u64>()
            .map_err(|_| LicenseError::InvalidSemVer(s.to_string()))?;
        let patch = patch_str
            .parse::<u64>()
            .map_err(|_| LicenseError::InvalidSemVer(s.to_string()))?;

        Ok(SemVer {
            major,
            minor,
            patch,
            pre_release,
            build_metadata,
        })
    }
}

/// Platform-specific update bundle entry inside a Tauri v2 update manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformArtifact {
    /// Detached cryptographic signature content (`.sig` Base64 / Minisign / Ed25519 signature).
    pub signature: String,
    /// HTTPS download URL for the signed update bundle (`.app.tar.gz`, `.msi.zip`, `.nsis.zip`, `.AppImage.tar.gz`).
    pub url: String,
    /// Optional hex-encoded SHA-256 digest of the update archive for defense-in-depth verification.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Optional payload size in bytes.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Optional preferred native update backend override (`tauri_updater`, `sparkle_appcast`, `win_sparkle_omaha`).
    #[serde(default)]
    pub backend: Option<UpdateBackend>,
}

/// Tauri v2 Updater JSON manifest (`latest.json` / `beta.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TauriUpdateManifest {
    /// Release version string (e.g. `"0.2.0"` or `"0.2.0-beta.1"`).
    pub version: String,
    /// Markdown release notes displayed in the update prompt.
    #[serde(default)]
    pub notes: Option<String>,
    /// RFC 3339 publication timestamp (e.g. `"2026-11-22T15:00:00Z"`).
    #[serde(default)]
    pub pub_date: Option<String>,
    /// Optional explicit release channel (`stable` or `beta`).
    #[serde(default)]
    pub channel: Option<UpdateChannel>,
    /// Map of Tauri target keys (e.g. `"darwin-aarch64"`, `"darwin-x86_64"`, `"windows-x86_64"`, `"linux-x86_64"`)
    /// to signed platform artifacts.
    pub platforms: BTreeMap<String, PlatformArtifact>,
}

/// Outcome of evaluating an update manifest for a specific host platform and channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum UpdateCheckDecision {
    /// Current installed version is greater than or equal to the manifest version (or manifest is for a different channel).
    UpToDate {
        current_version: String,
        channel: UpdateChannel,
    },
    /// Running inside a managed sandbox (such as Linux Flatpak) where in-place self-updates
    /// are disabled in favor of the OS package manager (`flatpak update`).
    ManagedByPackageManager {
        backend: UpdateBackend,
        current_version: String,
        latest_version: String,
        command_hint: String,
    },
    /// A verified newer version is available for download and in-place installation.
    UpdateAvailable {
        current_version: String,
        new_version: String,
        channel: UpdateChannel,
        backend: UpdateBackend,
        artifact: PlatformArtifact,
        notes: Option<String>,
        pub_date: Option<String>,
    },
}

/// Parses and validates the structure of a Tauri v2 `latest.json` update manifest.
pub fn parse_update_manifest(json_str: &str) -> Result<TauriUpdateManifest, LicenseError> {
    let manifest: TauriUpdateManifest = serde_json::from_str(json_str)
        .map_err(|e| LicenseError::InvalidUpdateManifest(e.to_string()))?;

    // Validate SemVer syntax on manifest.version
    let _ = SemVer::from_str(&manifest.version)?;

    if manifest.platforms.is_empty() {
        return Err(LicenseError::InvalidUpdateManifest(
            "manifest 'platforms' map cannot be empty".to_string(),
        ));
    }

    Ok(manifest)
}

/// Validates a platform artifact's URL and cryptographic signature metadata.
///
/// If an Ed25519 `verifying_key` and `signed_payload` are provided, performs full Ed25519
/// signature verification in addition to structural metadata validation.
pub fn verify_artifact_signature_metadata(
    artifact: &PlatformArtifact,
    ed25519_verifying_key: Option<(&VerifyingKey, &[u8])>,
) -> Result<(), LicenseError> {
    let url = artifact.url.trim();
    if !url.starts_with("https://") {
        return Err(LicenseError::InvalidUpdateManifest(format!(
            "update artifact URL must use HTTPS: '{url}'"
        )));
    }

    let sig_trimmed = artifact.signature.trim();
    if sig_trimmed.is_empty() {
        return Err(LicenseError::InvalidUpdateSignature(
            "artifact signature field is empty".to_string(),
        ));
    }

    if let Some(ref sha256) = artifact.sha256 {
        if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(LicenseError::InvalidUpdateSignature(
                "artifact sha256 digest must be 64 hexadecimal characters".to_string(),
            ));
        }
    }

    if let Some((vk, payload)) = ed25519_verifying_key {
        let raw_sig = URL_SAFE_NO_PAD
            .decode(sig_trimmed)
            .or_else(|_| STANDARD.decode(sig_trimmed))
            .map_err(|e| {
                LicenseError::InvalidUpdateSignature(format!("invalid base64 signature: {e}"))
            })?;
        let signature = Signature::from_slice(&raw_sig).map_err(|_| {
            LicenseError::InvalidUpdateSignature(
                "Ed25519 signature must be 64 bytes".to_string(),
            )
        })?;
        vk.verify(payload, &signature).map_err(|_| {
            LicenseError::InvalidUpdateSignature(
                "Ed25519 signature verification failed over artifact digest".to_string(),
            )
        })?;
    }

    Ok(())
}

/// Evaluates whether an update should be offered for `current_version_str` on `platform_key`
/// under `requested_channel`.
///
/// When `is_flatpak_sandbox` is `true`, self-update is intercepted and delegated to
/// `flatpak update io.github.emrisca.RatPoison` (OSTree atomic delta update).
pub fn evaluate_update(
    manifest: &TauriUpdateManifest,
    current_version_str: &str,
    requested_channel: UpdateChannel,
    platform_key: &str,
    is_flatpak_sandbox: bool,
) -> Result<UpdateCheckDecision, LicenseError> {
    let current_ver = SemVer::from_str(current_version_str)?;
    let candidate_ver = SemVer::from_str(&manifest.version)?;

    let effective_channel = manifest.channel.unwrap_or_else(|| {
        if candidate_ver.is_prerelease() {
            UpdateChannel::Beta
        } else {
            UpdateChannel::Stable
        }
    });

    // A beta release must not be offered to a user subscribed to the Stable channel
    if requested_channel == UpdateChannel::Stable
        && (candidate_ver.is_prerelease() || effective_channel == UpdateChannel::Beta)
    {
        return Ok(UpdateCheckDecision::UpToDate {
            current_version: current_ver.to_string(),
            channel: requested_channel,
        });
    }

    if candidate_ver <= current_ver {
        return Ok(UpdateCheckDecision::UpToDate {
            current_version: current_ver.to_string(),
            channel: requested_channel,
        });
    }

    // Inside a Linux Flatpak sandbox, /app is read-only OSTree storage; disable in-app binary replacement.
    if is_flatpak_sandbox {
        return Ok(UpdateCheckDecision::ManagedByPackageManager {
            backend: UpdateBackend::FlatpakOstree,
            current_version: current_ver.to_string(),
            latest_version: candidate_ver.to_string(),
            command_hint: "flatpak update io.github.emrisca.RatPoison".to_string(),
        });
    }

    let artifact = manifest
        .platforms
        .get(platform_key)
        .ok_or_else(|| LicenseError::PlatformTargetNotFound(platform_key.to_string()))?
        .clone();

    verify_artifact_signature_metadata(&artifact, None)?;

    let backend = artifact.backend.unwrap_or(UpdateBackend::TauriUpdater);

    Ok(UpdateCheckDecision::UpdateAvailable {
        current_version: current_ver.to_string(),
        new_version: candidate_ver.to_string(),
        channel: effective_channel,
        backend,
        artifact,
        notes: manifest.notes.clone(),
        pub_date: manifest.pub_date.clone(),
    })
}

/// Detects whether the current Linux process is running inside a Flatpak sandbox.
pub fn detect_flatpak_sandbox() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || std::path::Path::new("/.flatpak-info").exists()
}
