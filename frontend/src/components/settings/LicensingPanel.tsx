import { useState } from 'react';
import { CheckCircle2, KeyRound, Laptop, ShieldAlert, ShieldCheck, WifiOff } from 'lucide-react';
import { useSettingsStore } from '../../store/useSettingsStore';

/**
 * Licensing tab view supporting 14-day trial status, 5-machine seat management,
 * online license activation, and Ed25519 air-gapped offline challenge-response activation.
 */
export function LicensingPanel() {
  const licenseStatus = useSettingsStore((s) => s.licenseStatus);
  const offlineChallenge = useSettingsStore((s) => s.offlineChallenge);
  const busyAction = useSettingsStore((s) => s.busyAction);
  const activateLicense = useSettingsStore((s) => s.activateLicense);
  const deactivateMachine = useSettingsStore((s) => s.deactivateMachine);
  const generateOfflineChallenge = useSettingsStore((s) => s.generateOfflineChallenge);
  const applyOfflineResponse = useSettingsStore((s) => s.applyOfflineResponse);

  const [licenseKey, setLicenseKey] = useState('');
  const [offlineBlob, setOfflineBlob] = useState('');
  const [showOffline, setShowOffline] = useState(false);

  const isLicensed =
    licenseStatus?.state === 'licensed' || licenseStatus?.state === 'offline-licensed';
  const isExpired = licenseStatus?.state === 'expired';

  return (
    <div className="rp-panel-stack">
      {/* Status Banner */}
      <div className={`rp-status-card rp-status-card--${licenseStatus?.state ?? 'trial'}`}>
        <div className="rp-status-icon">
          {isLicensed ? (
            <ShieldCheck size={24} />
          ) : isExpired ? (
            <ShieldAlert size={24} />
          ) : (
            <KeyRound size={24} />
          )}
        </div>
        <div className="rp-status-body">
          <div className="rp-status-headline">
            {isLicensed
              ? licenseStatus?.state === 'offline-licensed'
                ? 'Licensed (Offline Air-Gapped)'
                : 'Licensed — Full Access'
              : isExpired
                ? 'Trial Expired'
                : `14-Day Trial — ${licenseStatus?.trialDaysRemaining ?? 14} days remaining`}
          </div>
          <p className="rp-status-sub">
            {licenseStatus?.licenseKeyMasked
              ? `Active key: ${licenseStatus.licenseKeyMasked}`
              : 'All Grid, Subgrid, Free Mode, and Label Mode features are unlocked during evaluation.'}
          </p>
        </div>
        <div className="rp-seat-pill">
          <Laptop size={14} />
          <span>
            {licenseStatus?.machinesUsed ?? 1} / {licenseStatus?.machinesMax ?? 5} machines
          </span>
        </div>
      </div>

      {/* Activation Form */}
      <section className="rp-section-card">
        <h3 className="rp-section-title">License Key Activation</h3>
        <p className="rp-section-desc">
          One RatPoison license covers up to 5 personal workstations (macOS, Windows, and Linux).
          Machine ID: <code>{licenseStatus?.machineId ?? '—'}</code>
        </p>

        <div className="rp-inline-form">
          <input
            type="text"
            className="rp-input rp-input--mono rp-flex-1"
            placeholder="RP-XXXX-XXXX-XXXX-XXXX"
            value={licenseKey}
            onChange={(e) => setLicenseKey(e.target.value)}
            disabled={busyAction !== null}
          />
          <button
            type="button"
            className="rp-btn rp-btn--primary"
            disabled={!licenseKey.trim() || busyAction !== null}
            onClick={() => void activateLicense(licenseKey)}
          >
            <CheckCircle2 size={14} />
            {busyAction === 'activate' ? 'Activating…' : 'Activate License'}
          </button>
          {isLicensed && (
            <button
              type="button"
              className="rp-btn rp-btn--ghost"
              disabled={busyAction !== null}
              onClick={() => void deactivateMachine()}
            >
              {busyAction === 'deactivate' ? 'Releasing…' : 'Deactivate This Machine'}
            </button>
          )}
        </div>
      </section>

      {/* Air-Gapped Offline Challenge-Response */}
      <section className="rp-section-card">
        <div className="rp-section-row-between">
          <div>
            <h3 className="rp-section-title">
              <WifiOff size={15} style={{ marginRight: 6, verticalAlign: '-2px' }} />
              Offline Air-Gapped Activation
            </h3>
            <p className="rp-section-desc">
              Generate a signed machine challenge to activate RatPoison on air-gapped workstations without internet access.
            </p>
          </div>
          <button
            type="button"
            className="rp-btn rp-btn--secondary"
            onClick={() => setShowOffline((v) => !v)}
          >
            {showOffline ? 'Hide Offline Flow' : 'Configure Offline'}
          </button>
        </div>

        {showOffline && (
          <div className="rp-offline-box">
            <div className="rp-offline-step">
              <div className="rp-offline-step-head">
                <span>1. Generate Hardware Challenge</span>
                <button
                  type="button"
                  className="rp-btn rp-btn--secondary"
                  disabled={busyAction !== null}
                  onClick={() => void generateOfflineChallenge()}
                >
                  {busyAction === 'challenge' ? 'Generating…' : 'Generate Challenge'}
                </button>
              </div>
              {offlineChallenge && (
                <pre className="rp-code-block">{offlineChallenge.challengeCode}</pre>
              )}
            </div>

            <div className="rp-offline-step">
              <label className="rp-offline-step-head" htmlFor="rp-offline-response">
                <span>2. Paste Signed Ed25519 Response Token</span>
              </label>
              <textarea
                id="rp-offline-response"
                className="rp-textarea rp-input--mono"
                rows={3}
                placeholder="Paste signed base64 response blob from licensing portal…"
                value={offlineBlob}
                onChange={(e) => setOfflineBlob(e.target.value)}
              />
              <div className="rp-offline-actions">
                <button
                  type="button"
                  className="rp-btn rp-btn--primary"
                  disabled={!offlineBlob.trim() || busyAction !== null}
                  onClick={() => void applyOfflineResponse(offlineBlob)}
                >
                  {busyAction === 'offline-apply' ? 'Verifying…' : 'Verify & Apply Offline License'}
                </button>
              </div>
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
