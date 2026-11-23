import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  Download,
  Grid3X3,
  Keyboard,
  Laptop,
  Monitor,
  MousePointer2,
  Palette,
  RotateCcw,
  Search,
  Settings,
  ShieldAlert,
  Tag,
  X,
} from 'lucide-react';
import {
  SETTINGS_TABS,
  SETTING_REGISTRY,
  getModifiedPathsForTab,
  scoreSettingMatch,
  type SettingDescriptor,
} from '../../config/defaults';
import { isTauriRuntime, subscribeConfigUpdated } from '../../ipc/tauriBridge';
import { useSettingsStore } from '../../store/useSettingsStore';
import type { PlatformOs, SettingsTabId } from '../../types/ipc';
import { LiveOverlayPreview } from './LiveOverlayPreview';
import { RatBrandIcon } from './RatBrandIcon';
import { RestoreContextMenu } from './RestoreContextMenu';
import { SettingControlByPath } from './SettingControls';
import { UpdatesPanel } from './UpdatesPanel';

const TAB_ICONS: Record<SettingsTabId, ReactNode> = {
  general: <Settings size={16} />,
  grid: <Grid3X3 size={16} />,
  freeMode: <MousePointer2 size={16} />,
  labelMode: <Tag size={16} />,
  keybindings: <Keyboard size={16} />,
  appearance: <Palette size={16} />,
  updates: <Download size={16} />,
};

const TAB_LABEL_MAP: Record<SettingsTabId, string> = Object.fromEntries(
  SETTINGS_TABS.map((t) => [t.id, t.label]),
) as Record<SettingsTabId, string>;

function groupBySection(items: SettingDescriptor[]): Array<[string, SettingDescriptor[]]> {
  const map = new Map<string, SettingDescriptor[]>();
  for (const item of items) {
    const list = map.get(item.section);
    if (list) list.push(item);
    else map.set(item.section, [item]);
  }
  return Array.from(map.entries());
}

/**
 * Searchable, tabbed Settings window (`?view=settings`).
 */
export function SettingsView() {
  const platform = useSettingsStore((s) => s.platform);
  const permissions = useSettingsStore((s) => s.permissions);
  const defaultConfig = useSettingsStore((s) => s.defaultConfig);
  const config = useSettingsStore((s) => s.config);
  const monitors = useSettingsStore((s) => s.monitors);
  const updateResult = useSettingsStore((s) => s.updateResult);
  const activeTab = useSettingsStore((s) => s.activeTab);
  const searchQuery = useSettingsStore((s) => s.searchQuery);
  const loading = useSettingsStore((s) => s.loading);
  const errorMessage = useSettingsStore((s) => s.errorMessage);
  const toastMessage = useSettingsStore((s) => s.toastMessage);

  const loadInitialData = useSettingsStore((s) => s.loadInitialData);
  const applyExternalConfig = useSettingsStore((s) => s.applyExternalConfig);
  const setActiveTab = useSettingsStore((s) => s.setActiveTab);
  const setSearchQuery = useSettingsStore((s) => s.setSearchQuery);
  const resetTabToDefaults = useSettingsStore((s) => s.resetTabToDefaults);
  const resetAllDefaults = useSettingsStore((s) => s.resetAllDefaults);
  const applyOsKeybindPreset = useSettingsStore((s) => s.applyOsKeybindPreset);
  const refreshPermissions = useSettingsStore((s) => s.refreshPermissions);
  const requestPermissions = useSettingsStore((s) => s.requestPermissions);
  const openAccessibilitySettings = useSettingsStore((s) => s.openAccessibilitySettings);
  const checkForUpdates = useSettingsStore((s) => s.checkForUpdates);
  const clearFeedback = useSettingsStore((s) => s.clearFeedback);

  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const autoCheckedUpdatesRef = useRef(false);
  const [confirmResetAll, setConfirmResetAll] = useState(false);

  // Load platform info, config, monitors + listen for external config updates
  useEffect(() => {
    void loadInitialData();
    let active = true;
    let unlisten: (() => void) | null = null;

    void subscribeConfigUpdated((nextCfg) => {
      if (active) applyExternalConfig(nextCfg);
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [loadInitialData, applyExternalConfig]);

  // Poll permission status every 2s while global hook is waiting on OS Accessibility trust
  useEffect(() => {
    if (!isTauriRuntime() || !permissions || permissions.hookActive) return undefined;
    const id = window.setInterval(() => {
      void refreshPermissions();
    }, 2000);
    return () => window.clearInterval(id);
  }, [permissions, refreshPermissions]);

  // Automatically check GitHub Releases once on startup when enabled
  useEffect(() => {
    if (loading || autoCheckedUpdatesRef.current) return;
    autoCheckedUpdatesRef.current = true;
    if (config.updates.autoCheck) {
      void checkForUpdates({ silent: true });
    }
  }, [loading, config.updates.autoCheck, checkForUpdates]);

  // Apply Dark / Light / System theme token attribute
  useEffect(() => {
    const mode = config.appearance.theme;
    const apply = (resolved: 'dark' | 'light') => {
      document.documentElement.dataset.theme = resolved;
    };

    if (mode === 'system') {
      const mq = window.matchMedia('(prefers-color-scheme: dark)');
      apply(mq.matches ? 'dark' : 'light');
      const onChange = (e: MediaQueryListEvent) => apply(e.matches ? 'dark' : 'light');
      mq.addEventListener('change', onChange);
      return () => mq.removeEventListener('change', onChange);
    }

    apply(mode);
    return undefined;
  }, [config.appearance.theme]);

  // Keyboard shortcut: Cmd/Ctrl+K or '/' focuses live search
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const isInput =
        target &&
        (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT');

      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
      } else if (e.key === '/' && !isInput) {
        e.preventDefault();
        searchInputRef.current?.focus();
      } else if (e.key === 'Escape' && document.activeElement === searchInputRef.current) {
        setSearchQuery('');
        searchInputRef.current?.blur();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [setSearchQuery]);

  // Live fuzzy + substring search across all tabs
  const searchResults = useMemo(() => {
    const q = searchQuery.trim();
    if (!q) return [];
    const scored: Array<{ item: SettingDescriptor; score: number }> = [];
    for (const item of SETTING_REGISTRY) {
      const score = scoreSettingMatch(item, q);
      if (score !== null) {
        scored.push({ item, score });
      }
    }
    scored.sort((a, b) => b.score - a.score);
    return scored.map((s) => s.item);
  }, [searchQuery]);

  // Count matches per tab when searching
  const matchCountByTab = useMemo(() => {
    const counts: Partial<Record<SettingsTabId, number>> = {};
    for (const item of searchResults) {
      counts[item.tab] = (counts[item.tab] ?? 0) + 1;
    }
    return counts;
  }, [searchResults]);

  const activeTabMeta = SETTINGS_TABS.find((t) => t.id === activeTab) ?? SETTINGS_TABS[0];
  const activeTabSections = useMemo(() => {
    const items = SETTING_REGISTRY.filter((item) => item.tab === activeTab);
    return groupBySection(items);
  }, [activeTab]);

  const activeTabModifiedCount = getModifiedPathsForTab(config, activeTab, defaultConfig).length;
  const totalModifiedCount = useMemo(
    () =>
      SETTINGS_TABS.reduce(
        (acc, t) => acc + getModifiedPathsForTab(config, t.id, defaultConfig).length,
        0,
      ),
    [config, defaultConfig],
  );

  const isSearching = searchQuery.trim().length > 0;

  return (
    <div className="rp-settings-shell">
      {/* Left Navigation Sidebar */}
      <aside className="rp-sidebar">
        <div className="rp-brand">
          <div className="rp-brand-mark">
            <RatBrandIcon size={26} />
          </div>
          <div>
            <div className="rp-brand-name">RatPoison</div>
            <div className="rp-brand-version">
              v{updateResult?.currentVersion ?? '0.1.0'} · MIT FOSS
            </div>
          </div>
        </div>

        {/* Live Global Search Bar */}
        <div className="rp-search-box">
          <Search size={14} className="rp-search-icon" />
          <input
            ref={searchInputRef}
            type="text"
            className="rp-search-input"
            placeholder={platform.os === 'macos' ? 'Search settings… (⌘K)' : 'Search settings… (Ctrl+K)'}
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            aria-label="Search all settings"
          />
          {searchQuery && (
            <button
              type="button"
              className="rp-search-clear"
              onClick={() => setSearchQuery('')}
              aria-label="Clear search"
            >
              <X size={13} />
            </button>
          )}
        </div>

        {/* Tab Navigation */}
        <nav className="rp-nav" aria-label="Settings Categories">
          {SETTINGS_TABS.map((tab) => {
            const modifiedCount = getModifiedPathsForTab(config, tab.id, defaultConfig).length;
            const searchCount = isSearching ? matchCountByTab[tab.id] ?? 0 : 0;
            const isActive = !isSearching && activeTab === tab.id;

            return (
              <button
                key={tab.id}
                type="button"
                className={`rp-nav-item${isActive ? ' is-active' : ''}`}
                onClick={() => {
                  setActiveTab(tab.id);
                  if (isSearching) setSearchQuery('');
                }}
              >
                <span className="rp-nav-icon">{TAB_ICONS[tab.id]}</span>
                <span className="rp-nav-label">{tab.label}</span>
                {isSearching ? (
                  searchCount > 0 && <span className="rp-nav-badge">{searchCount}</span>
                ) : tab.id === 'updates' && updateResult?.updateAvailable ? (
                  <span
                    className="rp-nav-update-badge"
                    title={`Version v${updateResult.latestVersion} available on GitHub`}
                  >
                    NEW
                  </span>
                ) : (
                  modifiedCount > 0 && (
                    <span
                      className="rp-nav-mod-badge"
                      title={`${modifiedCount} modified setting${modifiedCount === 1 ? '' : 's'}`}
                    >
                      {modifiedCount}
                    </span>
                  )
                )}
              </button>
            );
          })}
        </nav>

        {/* Sidebar Footer: Platform, Monitor Info & Factory Reset */}
        <div className="rp-sidebar-footer">
          <div className="rp-monitor-chip" title={`Detected host OS: ${platform.displayName}`}>
            <Laptop size={13} />
            <span>{platform.displayName}</span>
          </div>
          <div className="rp-monitor-chip" title={monitors.map((m) => `${m.name} (${m.scaleFactor}x)`).join('\n')}>
            <Monitor size={13} />
            <span>
              {monitors.length || 1} display{monitors.length === 1 ? '' : 's'} detected
            </span>
          </div>

          {!confirmResetAll ? (
            <button
              type="button"
              className="rp-reset-all-btn"
              disabled={totalModifiedCount === 0}
              onClick={() => setConfirmResetAll(true)}
            >
              <RotateCcw size={13} />
              <span>Reset All Defaults ({totalModifiedCount})</span>
            </button>
          ) : (
            <div className="rp-confirm-reset">
              <span>Restore all?</span>
              <button
                type="button"
                className="rp-mini-danger"
                onClick={() => {
                  setConfirmResetAll(false);
                  void resetAllDefaults();
                }}
              >
                Confirm
              </button>
              <button
                type="button"
                className="rp-mini-cancel"
                onClick={() => setConfirmResetAll(false)}
              >
                Cancel
              </button>
            </div>
          )}
        </div>
      </aside>

      {/* Main Content Area */}
      <main className="rp-content">
        <header className="rp-content-header">
          <div>
            <h1 className="rp-content-title">
              {isSearching ? `Search Results for “${searchQuery.trim()}”` : activeTabMeta.label}
            </h1>
            <p className="rp-content-subtitle">
              {isSearching
                ? `${searchResults.length} matching setting${searchResults.length === 1 ? '' : 's'} across all tabs — right-click any row to Restore Default.`
                : activeTabMeta.shortDescription}
            </p>
          </div>

          {!isSearching && activeTabModifiedCount > 0 && (
            <button
              type="button"
              className="rp-btn rp-btn--secondary"
              onClick={() => void resetTabToDefaults(activeTab)}
            >
              <RotateCcw size={14} />
              Reset Tab ({activeTabModifiedCount})
            </button>
          )}
        </header>

        {isTauriRuntime() && permissions && !permissions.hookActive && (
          <div className="rp-banner rp-banner--warning" role="status">
            <div className="rp-banner-content">
              <ShieldAlert size={16} />
              <span>
                <strong>Global hotkeys paused ({config.keybindings.toggleGrid}):</strong>{' '}
                {permissions.guidance ??
                  'Grant Accessibility & Input Monitoring permissions so RatPoison can listen for hotkeys.'}
              </span>
            </div>
            <div className="rp-banner-actions">
              <button
                type="button"
                className="rp-btn rp-btn--primary"
                onClick={() => void requestPermissions()}
              >
                Grant Permission
              </button>
              {platform.os === 'macos' && (
                <button
                  type="button"
                  className="rp-btn rp-btn--secondary"
                  onClick={() => void openAccessibilitySettings()}
                >
                  Open Accessibility
                </button>
              )}
              <button
                type="button"
                className="rp-btn rp-btn--secondary"
                onClick={() => void refreshPermissions()}
              >
                Re-check
              </button>
            </div>
          </div>
        )}

        {errorMessage && (
          <div className="rp-banner rp-banner--error" role="alert">
            <span>{errorMessage}</span>
            <button type="button" className="rp-banner-close" onClick={clearFeedback}>
              <X size={14} />
            </button>
          </div>
        )}

        <div className="rp-content-scroll">
          {loading ? (
            <div className="rp-empty-state">Loading RatPoison configuration…</div>
          ) : isSearching ? (
            searchResults.length === 0 ? (
              <div className="rp-empty-state">
                <p>No settings matched “{searchQuery.trim()}”.</p>
                <button
                  type="button"
                  className="rp-btn rp-btn--secondary"
                  onClick={() => setSearchQuery('')}
                >
                  Clear Search
                </button>
              </div>
            ) : (
              <section className="rp-section-card">
                <div className="rp-search-hint-bar">
                  Live-editable matches — right-click any row or click its amber dot to Restore Default.
                </div>
                {searchResults.map((item) => (
                  <SettingControlByPath
                    key={item.path}
                    path={item.path}
                    highlightQuery={searchQuery}
                    badgeText={TAB_LABEL_MAP[item.tab]}
                  />
                ))}
              </section>
            )
          ) : activeTab === 'updates' ? (
            <UpdatesPanel />
          ) : (
            <div className="rp-panel-stack">
              {/* Real-time Mini Overlay Preview on Appearance Tab */}
              {activeTab === 'appearance' && <LiveOverlayPreview />}

              {/* OS-Aware Native Keybinding Preset Bar on Keybindings Tab */}
              {activeTab === 'keybindings' && (
                <div className="rp-status-card rp-status-card--updates">
                  <div className="rp-status-icon">
                    <Laptop size={20} />
                  </div>
                  <div className="rp-status-body">
                    <div className="rp-status-headline">
                      Native {platform.displayName} Keybindings
                    </div>
                    <p className="rp-status-sub">
                      RatPoison automatically configures conflict-free defaults for your detected OS.
                      Apply any OS preset below or click any shortcut to rebind it.
                    </p>
                  </div>
                  <div className="rp-os-preset-actions">
                    {(['macos', 'windows', 'linux'] as PlatformOs[]).map((osOption) => {
                      const label =
                        osOption === 'macos' ? 'macOS' : osOption === 'windows' ? 'Windows' : 'Linux';
                      const isNative = osOption === platform.os;
                      return (
                        <button
                          key={osOption}
                          type="button"
                          className={`rp-btn ${isNative ? 'rp-btn--primary' : 'rp-btn--secondary'}`}
                          onClick={() => void applyOsKeybindPreset(osOption)}
                          title={isNative ? `Detected OS (${platform.displayName})` : `Apply ${label} keybindings`}
                        >
                          {label}
                          {isNative ? ' (Auto)' : ''}
                        </button>
                      );
                    })}
                  </div>
                </div>
              )}

              {activeTabSections.map(([sectionName, items]) => (
                <section key={sectionName} className="rp-section-card">
                  <div className="rp-section-header">
                    <h3 className="rp-section-title">{sectionName}</h3>
                    <span className="rp-section-hint">Right-click any row to restore default</span>
                  </div>
                  {items.map((item) => (
                    <SettingControlByPath key={item.path} path={item.path} />
                  ))}
                </section>
              ))}
            </div>
          )}
        </div>

        {/* Floating Toast Feedback */}
        {toastMessage && (
          <div className="rp-toast" role="status">
            {toastMessage}
          </div>
        )}
      </main>

      {/* Right-Click "Restore Default" Context Menu */}
      <RestoreContextMenu />
    </div>
  );
}
