import { Download, ExternalLink, RefreshCw, Sparkles } from 'lucide-react';
import { GITHUB_RELEASES_URL } from '../../ipc/tauriBridge';
import { useSettingsStore } from '../../store/useSettingsStore';
import { SettingControlByPath } from './SettingControls';

function formatPublishedDate(iso?: string): string | null {
  if (!iso) return null;
  const parsed = Date.parse(iso);
  if (Number.isNaN(parsed)) return null;
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  }).format(new Date(parsed));
}

/**
 * Updates tab view combining release channel controls with live GitHub Releases checks.
 */
export function UpdatesPanel() {
  const config = useSettingsStore((s) => s.config);
  const updateResult = useSettingsStore((s) => s.updateResult);
  const busyAction = useSettingsStore((s) => s.busyAction);
  const checkForUpdates = useSettingsStore((s) => s.checkForUpdates);
  const openExternalUrl = useSettingsStore((s) => s.openExternalUrl);

  const releaseUrl = updateResult?.releaseUrl ?? GITHUB_RELEASES_URL;
  const downloadUrl = updateResult?.downloadUrl ?? releaseUrl;
  const publishedLabel = formatPublishedDate(updateResult?.publishedAt);

  return (
    <div className="rp-panel-stack">
      <div className="rp-status-card rp-status-card--updates">
        <div className="rp-status-icon">
          <Sparkles size={22} />
        </div>
        <div className="rp-status-body">
          <div className="rp-status-headline">
            <span>RatPoison v{updateResult?.currentVersion ?? '0.1.0'}</span>
            {updateResult?.updateAvailable && (
              <span className="rp-update-pill">v{updateResult.latestVersion} available</span>
            )}
          </div>
          <p className="rp-status-sub">
            {updateResult
              ? updateResult.updateAvailable
                ? `A newer ${config.updates.channel} build (v${updateResult.latestVersion}) is live on GitHub Releases.`
                : `You are on the latest ${config.updates.channel} release (v${updateResult.latestVersion}).`
              : 'Free & open-source (MIT). Checks live releases from github.com/emrisca/RatPoison.'}
          </p>
          <div className="rp-update-meta">
            <span>Source: github.com/emrisca/RatPoison/releases</span>
            {publishedLabel && <span>· Published {publishedLabel}</span>}
            {updateResult?.assetName && <span>· {updateResult.assetName}</span>}
          </div>
        </div>
        <div className="rp-status-actions">
          {updateResult?.updateAvailable && (
            <button
              type="button"
              className="rp-btn rp-btn--primary"
              onClick={() => void openExternalUrl(downloadUrl)}
              title={
                updateResult.assetName
                  ? `Download ${updateResult.assetName}`
                  : `Open v${updateResult.latestVersion} on GitHub Releases`
              }
            >
              <Download size={14} />
              Download v{updateResult.latestVersion}
            </button>
          )}
          <button
            type="button"
            className={`rp-btn ${updateResult?.updateAvailable ? 'rp-btn--secondary' : 'rp-btn--primary'}`}
            disabled={busyAction !== null}
            onClick={() => void checkForUpdates()}
          >
            <RefreshCw size={14} className={busyAction === 'updates' ? 'rp-spin' : undefined} />
            {busyAction === 'updates' ? 'Checking…' : 'Check for Updates'}
          </button>
          <button
            type="button"
            className="rp-btn rp-btn--secondary"
            onClick={() => void openExternalUrl(releaseUrl)}
            title="Open RatPoison Releases on GitHub"
          >
            <ExternalLink size={14} />
            Releases
          </button>
        </div>
      </div>

      {updateResult?.releaseNotes && (
        <section className="rp-section-card">
          <div className="rp-section-header">
            <h3 className="rp-section-title">
              Release Notes — v{updateResult.latestVersion}
            </h3>
            <button
              type="button"
              className="rp-btn rp-btn--secondary"
              onClick={() => void openExternalUrl(releaseUrl)}
            >
              <ExternalLink size={13} />
              View on GitHub
            </button>
          </div>
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
