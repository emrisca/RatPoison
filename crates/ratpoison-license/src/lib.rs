//! `ratpoison-license` — 14-day evaluation trial, 5-machine seat licensing state machine,
//! deactivation dialog workflow, air-gapped Ed25519 challenge-response activation,
//! and cross-platform update manifest & signature verification.

pub mod error;
pub mod fingerprint;
pub mod manager;
pub mod offline;
pub mod updater;

pub use error::LicenseError;
pub use fingerprint::{
    compute_machine_fingerprint, compute_machine_fingerprint_with_salt,
    detect_current_machine_fingerprint, normalize_and_hash_license_key, MachineSeat,
    MachineSeatSummary, DEFAULT_FINGERPRINT_SALT, LICENSE_KEY_HASH_SALT,
};
pub use manager::{
    ActivationMode, ActivationOutcome, DeactivationPrompt, LicenseManager, LicenseState,
    PersistedLicenseStore, CLOCK_ROLLBACK_TOLERANCE_SECS, DEFAULT_EMBEDDED_PUBLIC_KEY_BYTES,
    MAX_MACHINES, TRIAL_DURATION_DAYS, TRIAL_DURATION_SECS,
};
pub use offline::{
    decode_offline_challenge, generate_offline_challenge, generate_offline_challenge_with_metadata,
    issue_offline_activation_token, sign_offline_claims, verify_offline_activation_token,
    OfflineActivationClaims, OfflineChallengePayload, OFFLINE_CHALLENGE_PREFIX,
    OFFLINE_TOKEN_PREFIX,
};
pub use updater::{
    detect_flatpak_sandbox, evaluate_update, parse_update_manifest,
    verify_artifact_signature_metadata, PlatformArtifact, SemVer, TauriUpdateManifest,
    UpdateBackend, UpdateChannel, UpdateCheckDecision,
};

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use ed25519_dalek::{Signer, SigningKey};
    use std::str::FromStr;

    fn test_keypair() -> (SigningKey, ed25519_dalek::VerifyingKey) {
        let secret_bytes: [u8; 32] = [
            0x42, 0x11, 0x99, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0x10, 0x20, 0x30,
            0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, 0x12, 0x34,
            0x56, 0x78, 0x9a, 0xbc,
        ];
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let verifying_key = signing_key.verifying_key();
        (signing_key, verifying_key)
    }

    fn sample_machine(index: usize) -> MachineSeatSummary {
        let raw_hw_id = format!("IOPlatformUUID-0000-0000-000{index}");
        let fp = compute_machine_fingerprint(&raw_hw_id).expect("fingerprint succeeds");
        MachineSeatSummary {
            machine_fingerprint: fp,
            friendly_hostname: format!("workstation-{index}.local"),
            os: match index % 3 {
                0 => "macos".to_string(),
                1 => "windows".to_string(),
                _ => "linux".to_string(),
            },
        }
    }

    #[test]
    fn test_14_day_trial_lifecycle_and_expiry() {
        let (_, vk) = test_keypair();
        let machine = sample_machine(1);
        let mut mgr = LicenseManager::new(machine, vk);

        let t0: u64 = 1_700_000_000;
        assert_eq!(
            mgr.evaluate_state(t0).unwrap(),
            LicenseState::UnstartedTrial
        );

        // Start trial at t0 -> 14 days remaining
        let started = mgr.start_trial(t0).unwrap();
        assert!(started.is_feature_unlocked());
        match started {
            LicenseState::TrialActive {
                started_at,
                expires_at,
                remaining_days,
                remaining_seconds,
            } => {
                assert_eq!(started_at, t0);
                assert_eq!(expires_at, t0 + TRIAL_DURATION_SECS);
                assert_eq!(remaining_days, 14);
                assert_eq!(remaining_seconds, TRIAL_DURATION_SECS);
            }
            other => panic!("expected TrialActive, got {other:?}"),
        }

        // Advance 7 days -> 7 days remaining
        let t_mid = t0 + 7 * 86_400;
        match mgr.evaluate_state(t_mid).unwrap() {
            LicenseState::TrialActive { remaining_days, .. } => assert_eq!(remaining_days, 7),
            other => panic!("expected TrialActive with 7 days, got {other:?}"),
        }

        // Advance to 1 second before expiry -> 1 day remaining
        let t_last_sec = t0 + TRIAL_DURATION_SECS - 1;
        match mgr.evaluate_state(t_last_sec).unwrap() {
            LicenseState::TrialActive {
                remaining_days,
                remaining_seconds,
                ..
            } => {
                assert_eq!(remaining_days, 1);
                assert_eq!(remaining_seconds, 1);
            }
            other => panic!("expected TrialActive on final second, got {other:?}"),
        }

        // Advance to exact 14-day boundary -> TrialExpired
        let t_expired = t0 + TRIAL_DURATION_SECS;
        let expired_state = mgr.evaluate_state(t_expired).unwrap();
        assert!(!expired_state.is_feature_unlocked());
        assert_eq!(
            expired_state,
            LicenseState::TrialExpired {
                started_at: t0,
                expired_at: t0 + TRIAL_DURATION_SECS,
            }
        );
    }

    #[test]
    fn test_monotonic_clock_rollback_detection() {
        let (_, vk) = test_keypair();
        let mut mgr = LicenseManager::new(sample_machine(1), vk);

        let t0: u64 = 1_700_100_000;
        mgr.start_trial(t0).unwrap();

        // Minor NTP backward drift within 300s tolerance is permitted
        let minor_drift = t0 - 120;
        assert!(mgr.evaluate_state(minor_drift).is_ok());

        // Rollback exceeding 300s triggers ClockRollbackDetected error and records ClockTampered
        let tampered_ts = t0 - (CLOCK_ROLLBACK_TOLERANCE_SECS + 3600);
        let err = mgr.evaluate_state(tampered_ts).unwrap_err();
        assert_eq!(
            err,
            LicenseError::ClockRollbackDetected {
                last_observed: t0,
                current: tampered_ts,
                rollback_secs: CLOCK_ROLLBACK_TOLERANCE_SECS + 3600,
            }
        );
        assert!(matches!(
            mgr.store().cached_state,
            Some(LicenseState::ClockTampered { .. })
        ));
    }

    #[test]
    fn test_5_machine_seat_limit_and_deactivation_swap() {
        let (_, vk) = test_keypair();
        let license_key = "RP-PRO-2026-ABCD-EFGH-9999";
        let mut mgr = LicenseManager::new(sample_machine(1), vk);
        let mut now: u64 = 1_700_200_000;

        // Activate 5 distinct machines (seats 1..=5)
        for i in 1..=MAX_MACHINES {
            now += 60;
            let machine = sample_machine(i);
            let outcome = mgr.activate_machine(license_key, machine.clone(), now).unwrap();
            match outcome {
                ActivationOutcome::Activated { state, seat } => {
                    assert!(state.is_feature_unlocked());
                    assert_eq!(seat.seat_id, i as u8);
                    assert_eq!(seat.machine_fingerprint, machine.machine_fingerprint);
                }
                ActivationOutcome::RequiresDeactivation { .. } => {
                    panic!("seat {i} should not require deactivation")
                }
            }
        }
        assert_eq!(mgr.active_seats().len(), 5);

        // Re-activating machine #3 is idempotent and updates last_seen without consuming a 6th seat
        now += 120;
        let machine_3 = sample_machine(3);
        match mgr
            .activate_machine(license_key, machine_3.clone(), now)
            .unwrap()
        {
            ActivationOutcome::Activated { seat, .. } => {
                assert_eq!(seat.seat_id, 3);
                assert_eq!(seat.last_seen, now);
            }
            _ => panic!("expected idempotent refresh for machine 3"),
        }

        // Attempting to activate a 6th machine returns RequiresDeactivation with all 5 seats
        now += 60;
        let machine_6 = sample_machine(6);
        let sixth_outcome = mgr
            .activate_machine(license_key, machine_6.clone(), now)
            .unwrap();
        let prompt = match sixth_outcome {
            ActivationOutcome::RequiresDeactivation { prompt } => prompt,
            other => panic!("expected RequiresDeactivation for 6th machine, got {other:?}"),
        };

        assert_eq!(prompt.max_machines, 5);
        assert_eq!(prompt.occupied_seats.len(), 5);
        assert_eq!(
            prompt.attempting_machine.machine_fingerprint,
            machine_6.machine_fingerprint
        );

        // User chooses to deactivate machine #2 from the dialog to free seat #2 for machine #6
        let machine_2_fp = sample_machine(2).machine_fingerprint;
        now += 30;
        let swap_outcome = mgr
            .deactivate_and_replace_seat(license_key, &machine_2_fp, now)
            .unwrap();

        match swap_outcome {
            ActivationOutcome::Activated { state, seat } => {
                assert!(state.is_feature_unlocked());
                assert_eq!(seat.seat_id, 2);
                assert_eq!(seat.machine_fingerprint, machine_6.machine_fingerprint);
                assert_eq!(seat.friendly_hostname, "workstation-6.local");
            }
            _ => panic!("expected seat replacement to succeed"),
        }

        assert_eq!(mgr.active_seats().len(), 5);
        assert!(!mgr
            .active_seats()
            .iter()
            .any(|s| s.machine_fingerprint == machine_2_fp));
        assert!(mgr
            .active_seats()
            .iter()
            .any(|s| s.machine_fingerprint == machine_6.machine_fingerprint && s.seat_id == 2));
    }

    #[test]
    fn test_offline_airgapped_challenge_response_and_tamper_rejection() {
        let (sk, vk) = test_keypair();
        let license_key = "RP-AIRGAP-ENT-2026-8888";
        let airgapped_machine = sample_machine(10);

        let tmp_dir = std::env::temp_dir().join(format!(
            "ratpoison_license_test_{}",
            std::process::id()
        ));
        let license_file = tmp_dir.join("license_state.json");
        let _ = std::fs::remove_file(&license_file);

        let mut mgr = LicenseManager::new(airgapped_machine.clone(), vk)
            .with_storage_path(&license_file);

        let t_challenge: u64 = 1_700_500_000;
        let challenge_code = mgr
            .generate_offline_challenge(license_key, t_challenge)
            .expect("challenge generation succeeds");
        assert!(challenge_code.starts_with(OFFLINE_CHALLENGE_PREFIX));
        // Ensure raw license key is never present in the challenge code
        assert!(!challenge_code.contains(license_key));

        // Server issues signed Ed25519 activation token for seat 1
        let t_issue = t_challenge + 300;
        let expires_at_ts = t_issue + 365 * 86_400;
        let expires_at = Some(expires_at_ts);
        let token = issue_offline_activation_token(
            &sk,
            &challenge_code,
            license_key,
            1,
            t_issue,
            expires_at,
        )
        .expect("token issuance succeeds");
        assert!(token.starts_with(OFFLINE_TOKEN_PREFIX));

        // 1. Tamper test: wrong license key during issuance is rejected
        assert_eq!(
            issue_offline_activation_token(
                &sk,
                &challenge_code,
                "RP-WRONG-KEY-0000",
                1,
                t_issue,
                expires_at,
            )
            .unwrap_err(),
            LicenseError::LicenseKeyMismatch
        );

        // 2. Tamper test: flipped signature byte is rejected offline
        let mut corrupted_token = token.clone();
        let last_char = corrupted_token.pop().unwrap();
        corrupted_token.push(if last_char == 'A' { 'B' } else { 'A' });
        assert_eq!(
            mgr.apply_offline_response(&corrupted_token, t_issue + 10)
                .unwrap_err(),
            LicenseError::SignatureVerificationFailed
        );

        // 3. Tamper test: token applied on a different machine fingerprint is rejected
        let mut wrong_machine_mgr = LicenseManager::new(sample_machine(99), vk);
        assert!(matches!(
            wrong_machine_mgr
                .apply_offline_response(&token, t_issue + 10)
                .unwrap_err(),
            LicenseError::FingerprintMismatch { .. }
        ));

        // 4. Valid offline activation succeeds and atomically persists to disk
        let state = mgr
            .apply_offline_response(&token, t_issue + 10)
            .expect("valid offline token activates");
        assert!(state.is_feature_unlocked());
        assert_eq!(
            state,
            LicenseState::Licensed {
                mode: ActivationMode::OfflineChallengeResponse,
                license_key_hash: normalize_and_hash_license_key(license_key).unwrap(),
                machine_fingerprint: airgapped_machine.machine_fingerprint.clone(),
                seat_id: 1,
                activated_at: t_issue,
                expires_at,
            }
        );

        // Verify atomic file storage round-trip
        assert!(license_file.exists());
        let mut reloaded =
            LicenseManager::load_or_init(&license_file, airgapped_machine, vk).unwrap();
        let evaluated = reloaded.evaluate_state(t_issue + 60).unwrap();
        assert!(evaluated.is_feature_unlocked());

        // 5. Expired offline token is rejected
        let after_expiry = expires_at_ts + 1;
        assert_eq!(
            reloaded.evaluate_state(after_expiry).unwrap_err(),
            LicenseError::TokenExpired {
                expires_at: expires_at_ts,
                current: after_expiry,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_updater_manifest_semver_channels_and_flatpak_guard() {
        let (sk, vk) = test_keypair();
        let artifact_digest = b"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let sig = sk.sign(artifact_digest);
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());

        // Verify SemVer ordering including pre-release tags
        let v_0_1_0 = SemVer::from_str("0.1.0").unwrap();
        let v_0_2_0_beta_1 = SemVer::from_str("0.2.0-beta.1").unwrap();
        let v_0_2_0_beta_2 = SemVer::from_str("0.2.0-beta.2").unwrap();
        let v_0_2_0 = SemVer::from_str("v0.2.0").unwrap();
        assert!(v_0_1_0 < v_0_2_0_beta_1);
        assert!(v_0_2_0_beta_1 < v_0_2_0_beta_2);
        assert!(v_0_2_0_beta_2 < v_0_2_0);

        let manifest_json = format!(
            r#"{{
                "version": "0.2.0-beta.2",
                "notes": "Sub-grid label latency improvements",
                "pub_date": "2026-11-22T15:00:00Z",
                "channel": "beta",
                "platforms": {{
                    "darwin-aarch64": {{
                        "signature": "{sig_b64}",
                        "url": "https://github.com/emrisca/RatPoison/releases/download/v0.2.0-beta.2/RatPoison_aarch64.app.tar.gz",
                        "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                        "backend": "sparkle_appcast"
                    }},
                    "linux-x86_64": {{
                        "signature": "{sig_b64}",
                        "url": "https://github.com/emrisca/RatPoison/releases/download/v0.2.0-beta.2/RatPoison_amd64.AppImage.tar.gz"
                    }}
                }}
            }}"#
        );

        let manifest = parse_update_manifest(&manifest_json).expect("manifest parses");

        // Cryptographic signature metadata check with Ed25519 public key
        let darwin_artifact = manifest.platforms.get("darwin-aarch64").unwrap();
        verify_artifact_signature_metadata(darwin_artifact, Some((&vk, artifact_digest)))
            .expect("Ed25519 signature over artifact digest verifies");

        // Stable channel subscriber ignores Beta manifest
        let stable_decision = evaluate_update(
            &manifest,
            "0.1.0",
            UpdateChannel::Stable,
            "darwin-aarch64",
            false,
        )
        .unwrap();
        assert_eq!(
            stable_decision,
            UpdateCheckDecision::UpToDate {
                current_version: "0.1.0".to_string(),
                channel: UpdateChannel::Stable,
            }
        );

        // Beta channel subscriber receives UpdateAvailable
        let beta_decision = evaluate_update(
            &manifest,
            "0.2.0-beta.1",
            UpdateChannel::Beta,
            "darwin-aarch64",
            false,
        )
        .unwrap();
        match beta_decision {
            UpdateCheckDecision::UpdateAvailable {
                new_version,
                backend,
                ..
            } => {
                assert_eq!(new_version, "0.2.0-beta.2");
                assert_eq!(backend, UpdateBackend::SparkleAppcast);
            }
            other => panic!("expected UpdateAvailable, got {other:?}"),
        }

        // Linux Flatpak sandbox delegates update to `flatpak update` instead of in-place binary overwrite
        let flatpak_decision = evaluate_update(
            &manifest,
            "0.1.0",
            UpdateChannel::Beta,
            "linux-x86_64",
            true,
        )
        .unwrap();
        assert_eq!(
            flatpak_decision,
            UpdateCheckDecision::ManagedByPackageManager {
                backend: UpdateBackend::FlatpakOstree,
                current_version: "0.1.0".to_string(),
                latest_version: "0.2.0-beta.2".to_string(),
                command_hint: "flatpak update io.github.emrisca.RatPoison".to_string(),
            }
        );
    }
}
