import { Download, ExternalLink, RefreshCw, Sparkles } from 'lucide-react';
import { GITHUB_RELEASES_URL } from '../../ipc/tauriBridge';
import { useSettingsStore } from '../../store/useSettingsStore';
import { SettingControlByPath } from './SettingControls';
import '../../styles/updates.css';

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

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  const mb = kb / 1024;
  return `${mb.toFixed(1)} MB`;
}

/**
 * Updates tab view combining signed automatic in-app updates with release notes and preferences.
 */
export function UpdatesPanel() {
  const config = useSettingsStore((s) => s.config);
  const updateResult = useSettingsStore((s) => s.updateResult);
  const updateStatus = useSettingsStore((s) => s.updateStatus);
  const busyAction = useSettingsStore((s) => s.busyAction);
  const checkForUpdates = useSettingsStore((s) => s.checkForUpdates);
  const installUpdate = useSettingsStore((s) => s.installUpdate);
  const openExternalUrl = useSettingsStore((s) => s.openExternalUrl);

  const currentVersion = updateStatus?.currentVersion ?? updateResult?.currentVersion ?? '0.1.2';
  const latestVersion =
    (updateStatus?.latestVersion && updateStatus.latestVersion !== currentVersion
      ? updateStatus.latestVersion
      : undefined) ??
    (updateResult?.updateAvailable ? updateResult.latestVersion : undefined) ??
    updateResult?.latestVersion ??
    currentVersion;

  const isInstalling =
    updateStatus?.state === 'downloading' ||
    updateStatus?.state === 'installing' ||
    updateStatus?.state === 'ready' ||
    busyAction === 'install-update';

  const isChecking = updateStatus?.state === 'checking' || busyAction === 'updates';

  const updateAvailable =
    updateStatus?.state === 'available' ||
    updateStatus?.state === 'downloading' ||
    updateStatus?.state === 'installing' ||
    updateStatus?.state === 'ready' ||
    Boolean(updateResult?.updateAvailable);

  const releaseUrl = updateResult?.releaseUrl ?? GITHUB_RELEASES_URL;
  const publishedLabel = formatPublishedDate(updateStatus?.publishedAt ?? updateResult?.publishedAt);
  const releaseNotes = updateResult?.releaseNotes || updateStatus?.releaseNotes;

  const statusHeadline = (() => {
    if (updateStatus?.state === 'downloading') {
      return updateStatus.message ?? `Downloading RatPoison v${latestVersion}…`;
    }
    if (updateStatus?.state === 'installing' || updateStatus?.state === 'ready') {
      return updateStatus.message ?? `Installing RatPoison v${latestVersion} and restarting…`;
    }
    if (updateAvailable) {
      return config.updates.autoDownload
        ? `RatPoison v${latestVersion} is ready — updates install automatically in-app and restart cleanly.`
        : `RatPoison v${latestVersion} (${config.updates.channel}) is ready to install in-app without downloading an installer.`;
    }
    if (updateStatus?.state === 'upToDate' || updateResult) {
      return `You are running the latest ${config.updates.channel} release (v${latestVersion}).`;
    }
    return config.updates.autoDownload
      ? 'Automatic signed in-app updates are enabled. New releases install and restart automatically.'
      : 'Signed in-app updates via GitHub Releases — install new builds with one click.';
  })();

  return (
    <div className="rp-panel-stack">
      <div className="rp-status-card rp-status-card--updates rp-status-card--stacked">
        <div className="rp-status-icon">
          <Sparkles size={22} />
        </div>
        <div className="rp-status-body">
          <div className="rp-status-headline">
            <span>RatPoison v{currentVersion}</span>
            {updateAvailable && !isInstalling && (
              <span className="rp-update-pill">v{latestVersion} ready</span>
            )}
            {isInstalling && (
              <span className="rp-update-pill">
                {updateStatus?.state === 'downloading'
                  ? updateStatus.percent !== undefined
                    ? `Downloading ${updateStatus.percent}%`
                    : 'Downloading…'
                  : 'Installing…'}
              </span>
            )}
            {!updateAvailable && config.updates.autoDownload && (
              <span className="rp-update-pill rp-update-pill--auto">Auto-update on</span>
            )}
          </div>

          <p className="rp-status-sub">{statusHeadline}</p>

          {isInstalling && (
            <div className="rp-update-progress" role="progressbar" aria-valuenow={updateStatus?.percent}>
              <div className="rp-update-progress-track">
                <div
                  className={`rp-update-progress-fill${
                    updateStatus?.percent === undefined ? ' rp-update-progress-fill--indeterminate' : ''
                  }`}
                  style={
                    updateStatus?.percent !== undefined
                      ? { width: `${Math.max(4, Math.min(100, updateStatus.percent))}%` }
                      : undefined
                  }
                />
              </div>
              <div className="rp-update-progress-meta">
                <span>
                  {updateStatus?.state === 'downloading'
                    ? updateStatus.totalBytes
                      ? `${formatBytes(updateStatus.downloadedBytes)} / ${formatBytes(updateStatus.totalBytes)}`
                      : updateStatus.downloadedBytes > 0
                        ? `${formatBytes(updateStatus.downloadedBytes)} downloaded`
                        : 'Connecting to signed release…'
                    : 'Verifying signature & replacing app…'}
                </span>
                {updateStatus?.percent !== undefined && <span>{updateStatus.percent}%</span>}
              </div>
            </div>
          )}

          {updateStatus?.state === 'error' && updateStatus.message && (
            <p className="rp-update-error-note">{updateStatus.message}</p>
          )}

          <div className="rp-update-meta">
            <span>Signed in-app updater</span>
            <span aria-hidden="true">·</span>
            <span>github.com/emrisca/RatPoison</span>
            {publishedLabel && (
              <>
                <span aria-hidden="true">·</span>
                <span>Published {publishedLabel}</span>
              </>
            )}
          </div>

          <div className="rp-status-actions">
            {updateAvailable && (
              <button
                type="button"
                className="rp-btn rp-btn--primary"
                disabled={isInstalling}
                onClick={() => void installUpdate()}
                title={`Download, verify, install v${latestVersion}, and restart RatPoison`}
              >
                <Download size={14} />
                {isInstalling ? 'Installing…' : `Install & Restart v${latestVersion}`}
              </button>
            )}
            <button
              type="button"
              className={`rp-btn ${updateAvailable ? 'rp-btn--secondary' : 'rp-btn--primary'}`}
              disabled={isChecking || isInstalling}
              onClick={() => void checkForUpdates()}
            >
              <RefreshCw size={14} className={isChecking ? 'rp-spin' : undefined} />
              {isChecking ? 'Checking…' : 'Check for Updates'}
            </button>
            <button
              type="button"
              className="rp-btn rp-btn--secondary"
              onClick={() => void openExternalUrl(releaseUrl)}
              title="View release history on GitHub"
            >
              <ExternalLink size={14} />
              Releases
            </button>
          </div>
        </div>
      </div>

      {releaseNotes && (
        <section className="rp-section-card">
          <div className="rp-section-header">
            <h3 className="rp-section-title">
              Release Notes — v{latestVersion}
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
          <pre className="rp-code-block">{releaseNotes}</pre>
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
