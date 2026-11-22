import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  Crosshair,
  Download,
  Grid3X3,
  KeyRound,
  Keyboard,
  Monitor,
  MousePointer2,
  Palette,
  RotateCcw,
  Search,
  Settings,
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
import { subscribeConfigUpdated } from '../../ipc/tauriBridge';
import { useSettingsStore } from '../../store/useSettingsStore';
import type { SettingsTabId } from '../../types/ipc';
import { LicensingPanel } from './LicensingPanel';
import { LiveOverlayPreview } from './LiveOverlayPreview';
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
  licensing: <KeyRound size={16} />,
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
  const config = useSettingsStore((s) => s.config);
  const monitors = useSettingsStore((s) => s.monitors);
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
  const clearFeedback = useSettingsStore((s) => s.clearFeedback);

  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const [confirmResetAll, setConfirmResetAll] = useState(false);

  // Load config, monitors, license status + listen for external config updates
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

  // Live fuzzy + substring search across all 8 tabs
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

  const activeTabModifiedCount = getModifiedPathsForTab(config, activeTab).length;
  const totalModifiedCount = useMemo(
    () => SETTINGS_TABS.reduce((acc, t) => acc + getModifiedPathsForTab(config, t.id).length, 0),
    [config],
  );

  const isSearching = searchQuery.trim().length > 0;

  return (
    <div className="rp-settings-shell">
      {/* Left Navigation Sidebar */}
      <aside className="rp-sidebar">
        <div className="rp-brand">
          <div className="rp-brand-mark">
            <Crosshair size={18} />
          </div>
          <div>
            <div className="rp-brand-name">RatPoison</div>
            <div className="rp-brand-version">v0.1.0 · Rust + Tauri v2</div>
          </div>
        </div>

        {/* Live Global Search Bar */}
        <div className="rp-search-box">
          <Search size={14} className="rp-search-icon" />
          <input
            ref={searchInputRef}
            type="text"
            className="rp-search-input"
            placeholder="Search settings… (⌘K)"
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
            const modifiedCount = getModifiedPathsForTab(config, tab.id).length;
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

        {/* Sidebar Footer: Monitor Info & Factory Reset */}
        <div className="rp-sidebar-footer">
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
          ) : activeTab === 'licensing' ? (
            <LicensingPanel />
          ) : activeTab === 'updates' ? (
            <UpdatesPanel />
          ) : (
            <div className="rp-panel-stack">
              {/* Real-time Mini Overlay Preview on Appearance Tab */}
              {activeTab === 'appearance' && <LiveOverlayPreview />}

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
