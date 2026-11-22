import { RefreshCw, Sparkles } from 'lucide-react';
import { useSettingsStore } from '../../store/useSettingsStore';
import { SettingControlByPath } from './SettingControls';

/**
 * Updates tab view combining release channel controls with signed updater status.
 */
export function UpdatesPanel() {
  const updateResult = useSettingsStore((s) => s.updateResult);
  const busyAction = useSettingsStore((s) => s.busyAction);
  const checkForUpdates = useSettingsStore((s) => s.checkForUpdates);

  return (
    <div className="rp-panel-stack">
      <div className="rp-status-card rp-status-card--updates">
        <div className="rp-status-icon">
          <Sparkles size={22} />
        </div>
        <div className="rp-status-body">
          <div className="rp-status-headline">
            RatPoison v{updateResult?.currentVersion ?? '0.1.0'}
          </div>
          <p className="rp-status-sub">
            {updateResult
              ? updateResult.updateAvailable
                ? `Version v${updateResult.latestVersion} is ready to install.`
                : 'You are on the latest verified build.'
              : 'All releases are Ed25519-signed and verified before installation.'}
          </p>
        </div>
        <button
          type="button"
          className="rp-btn rp-btn--primary"
          disabled={busyAction !== null}
          onClick={() => void checkForUpdates()}
        >
          <RefreshCw size={14} className={busyAction === 'updates' ? 'rp-spin' : undefined} />
          {busyAction === 'updates' ? 'Checking…' : 'Check for Updates'}
        </button>
      </div>

      {updateResult?.releaseNotes && (
        <section className="rp-section-card">
          <h3 className="rp-section-title">
            Release Notes — v{updateResult.latestVersion}
          </h3>
          <pre className="rp-code-block">{updateResult.releaseNotes}</pre>
        </section>
      )}

      <section className="rp-section-card">
        <h3 className="rp-section-title">Update Preferences</h3>
        <SettingControlByPath path="updates.channel" />
        <SettingControlByPath path="updates.autoCheck" />
        <SettingControlByPath path="updates.autoDownload" />
      </section>
    </div>
  );
}
