//! Core Licensing State Machine (`LicenseManager`):
//! - 14-day evaluation trial with monotonic clock rollback detection
//! - 5-machine seat registry (`MAX_MACHINES = 5`)
//! - Deactivation dialog payload (`DeactivationPrompt`) when all 5 seats are occupied
//! - Seat deactivation and immediate current-machine swap
//! - Offline Ed25519 challenge-response activation + atomic license file storage

use std::path::{Path, PathBuf};

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::LicenseError;
use crate::fingerprint::{
    normalize_and_hash_license_key, MachineSeat, MachineSeatSummary,
};
use crate::offline::{
    generate_offline_challenge_with_metadata, verify_offline_activation_token,
    OfflineActivationClaims,
};

/// Maximum number of active machines allowed per RatPoison license key.
pub const MAX_MACHINES: usize = 5;

/// Standard free evaluation trial duration in days.
pub const TRIAL_DURATION_DAYS: u64 = 14;

/// Standard free evaluation trial duration in seconds (`14 * 86_400 = 1_209_600`).
pub const TRIAL_DURATION_SECS: u64 = TRIAL_DURATION_DAYS * 24 * 60 * 60;

/// Maximum backward clock drift tolerated (5 minutes) for minor NTP corrections
/// before triggering tamper-evident `ClockTampered` lockdown.
pub const CLOCK_ROLLBACK_TOLERANCE_SECS: u64 = 300;

/// Default embedded Ed25519 public key bytes (32 bytes) used when no custom issuer key is injected.
pub const DEFAULT_EMBEDDED_PUBLIC_KEY_BYTES: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
    0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
    0x51, 0x1a,
];

/// How the active license seat was authorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMode {
    /// Activated against the 5-machine seat pool.
    OnlineSeat,
    /// Activated via air-gapped Ed25519 challenge-response token.
    OfflineChallengeResponse,
}

/// Structured deactivation prompt returned when all 5 seats are occupied and the user
/// attempts to activate a 6th machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeactivationPrompt {
    /// Salted SHA-256 hash of the license key being activated.
    pub license_key_hash: String,
    /// Maximum allowed seats (`5`).
    pub max_machines: usize,
    /// Metadata of the current machine attempting to claim a seat.
    pub attempting_machine: MachineSeatSummary,
    /// The 5 currently occupied seats (sorted by `seat_id`), including friendly hostname, OS, and `last_seen`.
    pub occupied_seats: Vec<MachineSeat>,
    /// Human-readable explanation for the Tauri UI dialog.
    pub message: String,
}

/// Complete state machine states for RatPoison licensing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LicenseState {
    /// Application launched for the first time before trial initialization.
    UnstartedTrial,
    /// Active 14-day evaluation period.
    TrialActive {
        started_at: u64,
        expires_at: u64,
        remaining_days: u32,
        remaining_seconds: u64,
    },
    /// 14-day trial window has elapsed without activation.
    TrialExpired {
        started_at: u64,
        expired_at: u64,
    },
    /// System clock was rolled backwards beyond [`CLOCK_ROLLBACK_TOLERANCE_SECS`].
    ClockTampered {
        last_observed_at: u64,
        current_timestamp: u64,
        rollback_seconds: u64,
    },
    /// Activation paused awaiting user selection in the 5-machine deactivation dialog.
    SeatLimitReached {
        prompt: DeactivationPrompt,
    },
    /// Validated paid license bound to the current machine fingerprint.
    Licensed {
        mode: ActivationMode,
        license_key_hash: String,
        machine_fingerprint: String,
        seat_id: u8,
        activated_at: u64,
        expires_at: Option<u64>,
    },
}

impl LicenseState {
    /// Returns `true` if RatPoison overlay features are unlocked (`TrialActive` or `Licensed`).
    pub fn is_feature_unlocked(&self) -> bool {
        matches!(
            self,
            LicenseState::TrialActive { .. } | LicenseState::Licensed { .. }
        )
    }
}

/// Result of an online activation attempt against the 5-machine seat limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ActivationOutcome {
    /// Seat was granted (or refreshed) and the machine is now `Licensed`.
    Activated {
        state: LicenseState,
        seat: MachineSeat,
    },
    /// All 5 seats are currently occupied by other machines; UI must display the deactivation modal.
    RequiresDeactivation {
        prompt: DeactivationPrompt,
    },
}

/// Serializable state persisted atomically to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PersistedLicenseStore {
    /// Timestamp when the 14-day trial was first started.
    pub trial_started_at: Option<u64>,
    /// Highest monotonic Unix timestamp ever observed (clock-skew tamper guard).
    pub last_observed_timestamp: Option<u64>,
    /// Salted SHA-256 hash of the active license key, if any.
    pub license_key_hash: Option<String>,
    /// Active mode if licensed.
    pub activation_mode: Option<ActivationMode>,
    /// Optional license expiration timestamp (for time-bounded offline leases).
    pub license_expires_at: Option<u64>,
    /// Up to `MAX_MACHINES` (5) active machine seats registered to this license.
    pub active_seats: Vec<MachineSeat>,
    /// Last evaluated state snapshot.
    pub cached_state: Option<LicenseState>,
}

/// Primary state machine manager for RatPoison trial, 5-machine seat management,
/// deactivation swaps, and air-gapped Ed25519 challenge-response activation.
#[derive(Debug, Clone)]
pub struct LicenseManager {
    current_machine: MachineSeatSummary,
    verifying_key: VerifyingKey,
    store: PersistedLicenseStore,
    storage_path: Option<PathBuf>,
}

impl LicenseManager {
    /// Creates a new `LicenseManager` for the given machine summary and Ed25519 `VerifyingKey`.
    pub fn new(current_machine: MachineSeatSummary, verifying_key: VerifyingKey) -> Self {
        Self {
            current_machine,
            verifying_key,
            store: PersistedLicenseStore::default(),
            storage_path: None,
        }
    }

    /// Creates a `LicenseManager` using the default embedded Ed25519 verifying key.
    pub fn with_default_key(current_machine: MachineSeatSummary) -> Result<Self, LicenseError> {
        let vk = VerifyingKey::from_bytes(&DEFAULT_EMBEDDED_PUBLIC_KEY_BYTES)
            .map_err(|e| LicenseError::Serialization(e.to_string()))?;
        Ok(Self::new(current_machine, vk))
    }

    /// Attaches an atomic file storage path to this `LicenseManager`.
    pub fn with_storage_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.storage_path = Some(path.into());
        self
    }

    /// Loads persisted license state from disk if the file exists, or initializes a fresh store.
    pub fn load_or_init<P: AsRef<Path>>(
        path: P,
        current_machine: MachineSeatSummary,
        verifying_key: VerifyingKey,
    ) -> Result<Self, LicenseError> {
        let path_buf = path.as_ref().to_path_buf();
        let store = if path_buf.exists() {
            let bytes = std::fs::read(&path_buf)
                .map_err(|e| LicenseError::StorageIo(e.to_string()))?;
            serde_json::from_slice::<PersistedLicenseStore>(&bytes)
                .map_err(|e| LicenseError::Serialization(e.to_string()))?
        } else {
            PersistedLicenseStore::default()
        };

        Ok(Self {
            current_machine,
            verifying_key,
            store,
            storage_path: Some(path_buf),
        })
    }

    /// Returns a reference to the current machine metadata.
    pub fn current_machine(&self) -> &MachineSeatSummary {
        &self.current_machine
    }

    /// Updates the current machine identity (useful when testing multi-machine seat pools).
    pub fn set_current_machine(&mut self, machine: MachineSeatSummary) {
        self.current_machine = machine;
    }

    /// Returns the currently registered machine seats (`0..=5`).
    pub fn active_seats(&self) -> &[MachineSeat] {
        &self.store.active_seats
    }

    /// Returns the underlying persisted store snapshot.
    pub fn store(&self) -> &PersistedLicenseStore {
        &self.store
    }

    /// Checks and updates the monotonic clock watermark.
    ///
    /// If `current_timestamp + CLOCK_ROLLBACK_TOLERANCE_SECS < last_observed`, transitions
    /// the state machine to `LicenseState::ClockTampered` and returns `Err(LicenseError::ClockRollbackDetected)`.
    fn check_and_advance_clock(&mut self, current_timestamp: u64) -> Result<(), LicenseError> {
        if let Some(last_observed) = self.store.last_observed_timestamp {
            if current_timestamp + CLOCK_ROLLBACK_TOLERANCE_SECS < last_observed {
                let rollback_seconds = last_observed - current_timestamp;
                self.store.cached_state = Some(LicenseState::ClockTampered {
                    last_observed_at: last_observed,
                    current_timestamp,
                    rollback_seconds,
                });
                let _ = self.persist_if_configured();
                return Err(LicenseError::ClockRollbackDetected {
                    last_observed,
                    current: current_timestamp,
                    rollback_secs: rollback_seconds,
                });
            }
            if current_timestamp > last_observed {
                self.store.last_observed_timestamp = Some(current_timestamp);
            }
        } else {
            self.store.last_observed_timestamp = Some(current_timestamp);
        }
        Ok(())
    }

    /// Starts the 14-day evaluation trial if not already started, and evaluates the current state.
    pub fn start_trial(&mut self, current_timestamp: u64) -> Result<LicenseState, LicenseError> {
        self.check_and_advance_clock(current_timestamp)?;
        if self.store.trial_started_at.is_none() {
            self.store.trial_started_at = Some(current_timestamp);
        }
        self.evaluate_state(current_timestamp)
    }

    /// Evaluates the current licensing state at `current_timestamp`, enforcing clock-rollback
    /// checks, offline token expiry, and 14-day trial window calculation.
    pub fn evaluate_state(&mut self, current_timestamp: u64) -> Result<LicenseState, LicenseError> {
        self.check_and_advance_clock(current_timestamp)?;

        // 1. Check if the current machine holds an active licensed seat
        if let (Some(key_hash), Some(mode)) = (
            self.store.license_key_hash.clone(),
            self.store.activation_mode,
        ) {
            if let Some(seat) = self
                .store
                .active_seats
                .iter_mut()
                .find(|s| s.machine_fingerprint == self.current_machine.machine_fingerprint)
            {
                if let Some(exp) = self.store.license_expires_at {
                    if current_timestamp > exp {
                        return Err(LicenseError::TokenExpired {
                            expires_at: exp,
                            current: current_timestamp,
                        });
                    }
                }
                seat.last_seen = seat.last_seen.max(current_timestamp);
                let state = LicenseState::Licensed {
                    mode,
                    license_key_hash: key_hash,
                    machine_fingerprint: seat.machine_fingerprint.clone(),
                    seat_id: seat.seat_id,
                    activated_at: seat.activated_at,
                    expires_at: self.store.license_expires_at,
                };
                self.store.cached_state = Some(state.clone());
                self.persist_if_configured()?;
                return Ok(state);
            }
        }

        // 2. Otherwise evaluate 14-day trial state
        let state = match self.store.trial_started_at {
            None => LicenseState::UnstartedTrial,
            Some(started_at) => {
                let expires_at = started_at.saturating_add(TRIAL_DURATION_SECS);
                if current_timestamp >= expires_at {
                    LicenseState::TrialExpired {
                        started_at,
                        expired_at: expires_at,
                    }
                } else {
                    let remaining_seconds = expires_at - current_timestamp;
                    // Ceiling division so day 1 shows 14 days remaining and last partial day shows 1 day remaining
                    let remaining_days = remaining_seconds.div_ceil(86_400) as u32;
                    LicenseState::TrialActive {
                        started_at,
                        expires_at,
                        remaining_days,
                        remaining_seconds,
                    }
                }
            }
        };

        self.store.cached_state = Some(state.clone());
        self.persist_if_configured()?;
        Ok(state)
    }

    /// Attempts to activate the current machine (or a specified `machine`) with `license_key`.
    ///
    /// - Raw `license_key` is wrapped in `Zeroizing<String>` and hashed before inspection.
    /// - If this machine's fingerprint is already present in `active_seats`, its `last_seen`
    ///   is updated and `ActivationOutcome::Activated` is returned.
    /// - If fewer than `MAX_MACHINES` (5) seats are occupied, allocates the lowest available
    ///   `seat_id` (`1..=5`) and transitions to `LicenseState::Licensed`.
    /// - If all 5 seats are occupied by other machines, transitions to `LicenseState::SeatLimitReached`
    ///   and returns `ActivationOutcome::RequiresDeactivation(DeactivationPrompt)` listing all 5 machines.
    pub fn activate_machine(
        &mut self,
        license_key: &str,
        machine: MachineSeatSummary,
        current_timestamp: u64,
    ) -> Result<ActivationOutcome, LicenseError> {
        self.check_and_advance_clock(current_timestamp)?;

        let key_guard = Zeroizing::new(license_key.to_string());
        let key_hash = normalize_and_hash_license_key(key_guard.as_str())?;

        // Reset seat pool if switching to a brand new license key hash
        if self.store.license_key_hash.as_deref() != Some(key_hash.as_str()) {
            self.store.license_key_hash = Some(key_hash.clone());
            self.store.active_seats.clear();
        }

        self.current_machine = machine.clone();

        // Case 1: Machine already holds one of the 5 seats -> idempotent refresh
        if let Some(existing_idx) = self
            .store
            .active_seats
            .iter()
            .position(|s| s.machine_fingerprint == machine.machine_fingerprint)
        {
            self.store.active_seats[existing_idx].last_seen = current_timestamp;
            self.store.active_seats[existing_idx].friendly_hostname =
                machine.friendly_hostname.clone();
            self.store.active_seats[existing_idx].os = machine.os.clone();
            let seat = self.store.active_seats[existing_idx].clone();

            self.store.activation_mode = Some(ActivationMode::OnlineSeat);
            self.store.license_expires_at = None;
            let state = LicenseState::Licensed {
                mode: ActivationMode::OnlineSeat,
                license_key_hash: key_hash,
                machine_fingerprint: seat.machine_fingerprint.clone(),
                seat_id: seat.seat_id,
                activated_at: seat.activated_at,
                expires_at: None,
            };
            self.store.cached_state = Some(state.clone());
            self.persist_if_configured()?;
            return Ok(ActivationOutcome::Activated { state, seat });
        }

        // Case 2: Free seat available (< 5 seats occupied)
        if self.store.active_seats.len() < MAX_MACHINES {
            let next_seat_id = self.allocate_lowest_free_seat_id()?;
            let new_seat = MachineSeat {
                seat_id: next_seat_id,
                machine_fingerprint: machine.machine_fingerprint.clone(),
                friendly_hostname: machine.friendly_hostname.clone(),
                os: machine.os.clone(),
                activated_at: current_timestamp,
                last_seen: current_timestamp,
            };
            self.store.active_seats.push(new_seat.clone());
            self.store.active_seats.sort_by_key(|s| s.seat_id);

            self.store.activation_mode = Some(ActivationMode::OnlineSeat);
            self.store.license_expires_at = None;
            let state = LicenseState::Licensed {
                mode: ActivationMode::OnlineSeat,
                license_key_hash: key_hash,
                machine_fingerprint: new_seat.machine_fingerprint.clone(),
                seat_id: new_seat.seat_id,
                activated_at: new_seat.activated_at,
                expires_at: None,
            };
            self.store.cached_state = Some(state.clone());
            self.persist_if_configured()?;
            return Ok(ActivationOutcome::Activated {
                state,
                seat: new_seat,
            });
        }

        // Case 3: All 5 seats occupied -> build DeactivationPrompt for UI modal
        let mut occupied_seats = self.store.active_seats.clone();
        occupied_seats.sort_by_key(|s| s.seat_id);

        let prompt = DeactivationPrompt {
            license_key_hash: key_hash,
            max_machines: MAX_MACHINES,
            attempting_machine: machine.clone(),
            occupied_seats,
            message: format!(
                "All {MAX_MACHINES} machine seats are currently active. Select an existing machine to deactivate and transfer its seat to '{}'.",
                machine.friendly_hostname
            ),
        };

        self.store.cached_state = Some(LicenseState::SeatLimitReached {
            prompt: prompt.clone(),
        });
        self.persist_if_configured()?;

        Ok(ActivationOutcome::RequiresDeactivation { prompt })
    }

    /// Deactivates an occupied seat identified by `target_fingerprint_to_deactivate` and
    /// immediately activates `self.current_machine` in its place.
    pub fn deactivate_and_replace_seat(
        &mut self,
        license_key: &str,
        target_fingerprint_to_deactivate: &str,
        current_timestamp: u64,
    ) -> Result<ActivationOutcome, LicenseError> {
        self.check_and_advance_clock(current_timestamp)?;

        let key_guard = Zeroizing::new(license_key.to_string());
        let key_hash = normalize_and_hash_license_key(key_guard.as_str())?;

        if self.store.license_key_hash.as_deref() != Some(key_hash.as_str()) {
            return Err(LicenseError::LicenseKeyMismatch);
        }

        let remove_idx = self
            .store
            .active_seats
            .iter()
            .position(|s| s.machine_fingerprint == target_fingerprint_to_deactivate.trim())
            .ok_or_else(|| {
                LicenseError::MachineSeatNotFound(
                    target_fingerprint_to_deactivate.trim().to_string(),
                )
            })?;

        let reclaimed_seat_id = self.store.active_seats[remove_idx].seat_id;
        self.store.active_seats.remove(remove_idx);

        let replacement_seat = MachineSeat {
            seat_id: reclaimed_seat_id,
            machine_fingerprint: self.current_machine.machine_fingerprint.clone(),
            friendly_hostname: self.current_machine.friendly_hostname.clone(),
            os: self.current_machine.os.clone(),
            activated_at: current_timestamp,
            last_seen: current_timestamp,
        };

        self.store.active_seats.push(replacement_seat.clone());
        self.store.active_seats.sort_by_key(|s| s.seat_id);
        self.store.activation_mode = Some(ActivationMode::OnlineSeat);
        self.store.license_expires_at = None;

        let state = LicenseState::Licensed {
            mode: ActivationMode::OnlineSeat,
            license_key_hash: key_hash,
            machine_fingerprint: replacement_seat.machine_fingerprint.clone(),
            seat_id: replacement_seat.seat_id,
            activated_at: replacement_seat.activated_at,
            expires_at: None,
        };
        self.store.cached_state = Some(state.clone());
        self.persist_if_configured()?;

        Ok(ActivationOutcome::Activated {
            state,
            seat: replacement_seat,
        })
    }

    /// Deactivates a specific machine fingerprint from the active seat list without replacing it.
    pub fn deactivate_seat(&mut self, target_fingerprint: &str) -> Result<MachineSeat, LicenseError> {
        let idx = self
            .store
            .active_seats
            .iter()
            .position(|s| s.machine_fingerprint == target_fingerprint.trim())
            .ok_or_else(|| {
                LicenseError::MachineSeatNotFound(target_fingerprint.trim().to_string())
            })?;

        let removed = self.store.active_seats.remove(idx);
        self.persist_if_configured()?;
        Ok(removed)
    }

    /// Generates a Base64URL offline challenge code for air-gapped activation of `self.current_machine`.
    pub fn generate_offline_challenge(
        &self,
        license_key: &str,
        current_timestamp: u64,
    ) -> Result<String, LicenseError> {
        generate_offline_challenge_with_metadata(
            license_key,
            &self.current_machine.machine_fingerprint,
            current_timestamp,
            &self.current_machine.friendly_hostname,
            &self.current_machine.os,
        )
    }

    /// Verifies and applies an Ed25519-signed offline activation token (`RP-ACTIVATE-V1:...`),
    /// checks hardware fingerprint binding and clock integrity, and atomically writes the
    /// activated license store to disk.
    pub fn apply_offline_response(
        &mut self,
        token: &str,
        current_timestamp: u64,
    ) -> Result<LicenseState, LicenseError> {
        self.check_and_advance_clock(current_timestamp)?;

        let claims: OfflineActivationClaims = verify_offline_activation_token(
            token,
            &self.verifying_key,
            &self.current_machine.machine_fingerprint,
            current_timestamp,
        )?;

        self.store.license_key_hash = Some(claims.license_key_hash.clone());
        self.store.activation_mode = Some(ActivationMode::OfflineChallengeResponse);
        self.store.license_expires_at = claims.expires_at;

        // Replace any existing entry for this machine or seat_id
        self.store.active_seats.retain(|s| {
            s.machine_fingerprint != claims.machine_fingerprint && s.seat_id != claims.seat_id
        });

        let offline_seat = MachineSeat {
            seat_id: claims.seat_id,
            machine_fingerprint: claims.machine_fingerprint.clone(),
            friendly_hostname: self.current_machine.friendly_hostname.clone(),
            os: self.current_machine.os.clone(),
            activated_at: claims.issued_at,
            last_seen: current_timestamp,
        };
        self.store.active_seats.push(offline_seat);
        self.store.active_seats.sort_by_key(|s| s.seat_id);

        let state = LicenseState::Licensed {
            mode: ActivationMode::OfflineChallengeResponse,
            license_key_hash: claims.license_key_hash,
            machine_fingerprint: claims.machine_fingerprint,
            seat_id: claims.seat_id,
            activated_at: claims.issued_at,
            expires_at: claims.expires_at,
        };

        self.store.cached_state = Some(state.clone());
        self.persist_if_configured()?;

        Ok(state)
    }

    /// Atomically writes the current license store to `path` via a sibling `.tmp` file + `rename`.
    pub fn save_atomic_to<P: AsRef<Path>>(&self, path: P) -> Result<(), LicenseError> {
        let target = path.as_ref();
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| LicenseError::StorageIo(e.to_string()))?;
            }
        }

        let tmp_path = target.with_extension("license.tmp");
        let serialized = serde_json::to_vec_pretty(&self.store)
            .map_err(|e| LicenseError::Serialization(e.to_string()))?;

        std::fs::write(&tmp_path, &serialized)
            .map_err(|e| LicenseError::StorageIo(e.to_string()))?;
        std::fs::rename(&tmp_path, target)
            .map_err(|e| LicenseError::StorageIo(e.to_string()))?;
        Ok(())
    }

    fn persist_if_configured(&self) -> Result<(), LicenseError> {
        if let Some(ref path) = self.storage_path {
            self.save_atomic_to(path)?;
        }
        Ok(())
    }

    fn allocate_lowest_free_seat_id(&self) -> Result<u8, LicenseError> {
        for candidate in 1..=(MAX_MACHINES as u8) {
            if !self
                .store
                .active_seats
                .iter()
                .any(|s| s.seat_id == candidate)
            {
                return Ok(candidate);
            }
        }
        Err(LicenseError::SeatLimitReached {
            max_machines: MAX_MACHINES,
        })
    }
}
